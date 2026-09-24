use std::collections::BTreeSet;

use rayc_ir::{
    address::Address,
    cfg::{Instruction, Point},
    dataflow::DataflowProblem,
    ir_expr::IRExprKind,
    ir_function::{FunctionID, IRFunctionMap},
};
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;

use crate::{
    Diagnostic, PlaceState, PossibleStates, StackState, StackStateProblem,
    diagnostic::{
        MoveOutOfHandlerCapture, UseAfterMove, UseAfterPartialMove, UseBeforeInitialization,
    },
    drop_elaboration::DropElaborator,
    stack_state::tracked_local,
};

/// Checks every reachable load and drops every stack value that would
/// otherwise outlive its owner.
///
/// Drops are inserted into `functions` as a forced move out of the value
/// followed by a `Drop.drop` call on it:
///
/// - on each control-flow edge into a merge, for values initialized on that
///   edge but not on every edge into the merge, so all paths agree on the stack
///   at the merge;
/// - before each scope exit, for values still initialized when their scope
///   ends;
/// - before each store, for the previous value of a place which is still
///   initialized when it is reassigned.
///
/// Returns the diagnostics found.
pub async fn analyze(
    engine: &TrackedEngine,
    def_id: GlobalSymbolID,
    functions: &mut IRFunctionMap,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    // Nested functions use the enclosing definition's predicates and target,
    // but each function owns an independent CFG and stack state.
    let function_ids =
        functions.functions().map(|(function_id, _)| function_id).collect::<Vec<_>>();
    for function_id in function_ids {
        let mut elaborator = DropElaborator::default();
        analyze_function(engine, def_id, functions, function_id, &mut elaborator, &mut diagnostics)
            .await;
        elaborator.insert_drops(engine, functions, function_id).await;
    }

    diagnostics
}

/// Reports loads of possibly uninitialized places, and moves out of captures
/// which the function only borrows, and selects every drop the function needs.
///
/// Everything is decided from one solution of the IR as written, so an
/// inserted drop is never reported as the site of a move. The solution also
/// describes the IR once merges are balanced: a place which may be
/// uninitialized on some edge into a merge is already uninitialized in the
/// joined state, which is exactly what dropping it on the other edges
/// produces. Later scope exits and reassignments therefore see every place
/// initialized on all paths or on none.
async fn analyze_function(
    engine: &TrackedEngine,
    def_id: GlobalSymbolID,
    functions: &IRFunctionMap,
    function_id: FunctionID,
    elaborator: &mut DropElaborator,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let function = functions.get_function(function_id);
    let captures = functions.captures_for_function(function_id);
    let mut problem =
        StackStateProblem::new(Solver::new(engine.clone(), def_id).await, function, captures);
    let solution = function.solve_dataflow(&mut problem).await.unwrap();

    // Replay from stable block-entry facts. Reporting during fixpoint
    // iteration could emit the same error repeatedly from provisional facts.
    for block_id in solution.reachable_blocks() {
        let mut state = solution.block_entry(block_id).unwrap().clone();

        for (instruction_idx, instruction) in
            function.block_instructions(block_id).iter().enumerate()
        {
            let point =
                Point::builder().block_id(block_id).instruction_idx(instruction_idx).build();

            match instruction {
                Instruction::Expression(expression_id) => {
                    let expression = function.get_expression(*expression_id);
                    match expression.kind() {
                        IRExprKind::Load(load) => {
                            // A load through a dereference reads the pointer
                            // it dereferences, without moving it.
                            let read = load
                                .address()
                                .deref_base(engine)
                                .unwrap_or_else(|| load.address().clone());
                            check_read(expression.span(), &read, &state, function, diagnostics);

                            // A borrowed capture is shared by every call, so
                            // nothing may be moved out of it, even briefly.
                            //
                            // REVIEW: Is this a bug? it should've covered the
                            // "fields/projections" of a capture too, not just
                            // the root capture itself.
                            if problem.is_borrowed_capture(load.address())
                                && problem.load_moves(load, expression.ty().clone()).await
                            {
                                let root = tracked_local(load.address())
                                    .expect("a capture is a stack root");
                                let capture_span = problem.binding_span(root).await;
                                diagnostics.push(
                                    MoveOutOfHandlerCapture::new(expression.span(), capture_span)
                                        .into(),
                                );
                            }
                        }

                        // Borrowing a place behind a dereference reads the
                        // pointer it dereferences.
                        IRExprKind::RefOf(ref_of) => {
                            if let Some(base) = ref_of.address().deref_base(engine) {
                                check_read(expression.span(), &base, &state, function, diagnostics);
                            }
                        }

                        IRExprKind::Error
                        | IRExprKind::Literal(_)
                        | IRExprKind::Binary(_)
                        | IRExprKind::Call(_)
                        | IRExprKind::Phi(_)
                        | IRExprKind::Perform(_)
                        | IRExprKind::Tuple(_)
                        | IRExprKind::Closure(_)
                        | IRExprKind::Handle(_)
                        | IRExprKind::StructInitialization(_) => {}
                    }
                }

                // Values still initialized when their scope ends are dropped
                // just before the scope pops.
                Instruction::ScopePop(scope_id) => {
                    elaborator
                        .scope_drops(point, *scope_id, &state, &mut problem, diagnostics)
                        .await;
                }

                // The previous value of a reassigned place is dropped after
                // the new value is computed, just before it is stored. A new
                // value which moved the old one out leaves nothing to drop.
                //
                // A store through a dereference writes memory the frame does
                // not own. It drops nothing, and only reads the pointer.
                Instruction::Store(store) => {
                    if let Some(base) = store.address().deref_base(engine) {
                        check_read(store.span(), &base, &state, function, diagnostics);
                    }

                    let ty = function.get_expression(store.expression()).ty().clone();
                    elaborator
                        .reassignment_drops(
                            point,
                            store.address(),
                            ty,
                            &state,
                            &mut problem,
                            diagnostics,
                        )
                        .await;
                }

                Instruction::ScopePush(_) | Instruction::ExprDiscard(_) => {}
            }

            problem.transfer_instruction(point, instruction, &mut state).await.unwrap();
        }
    }

    // Balance the stack on each edge into a merge. The analysis is not
    // edge-sensitive, so an edge carries its source's exit state unchanged.
    let drop_order = problem.roots_in_drop_order().await;
    for edge in solution.edges() {
        let (Some(exit), Some(entry)) =
            (solution.block_exit(edge.source()), solution.block_entry(edge.target()))
        else {
            continue;
        };

        elaborator.merge_drops(*edge, exit, entry, &drop_order, &mut problem, diagnostics).await;
    }
}

/// Reports a read at `span` of the place `address` when that place may be
/// uninitialized in `state`. Untracked places are never reported.
fn check_read(
    span: rayc_lexical::tree::RelativeSpan,
    address: &Address,
    state: &StackState,
    function: &rayc_ir::ir_function::IRFunction,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(place_state) = state.place_state(address)
        && !place_state.is_initialized()
    {
        diagnostics_for_load(span, place_state, function, diagnostics);
    }
}

/// Returns the spans of the most recent moves which may have left `state`
/// uninitialized.
fn diagnostics_for_load(
    use_span: rayc_lexical::tree::RelativeSpan,
    state: &PlaceState,
    function: &rayc_ir::ir_function::IRFunction,
    diags: &mut Vec<Diagnostic>,
) {
    let mut move_points = BTreeSet::new();
    let mut may_be_uninitialized_without_move = false;
    state.visit_uninitialized(&mut |history| {
        move_points.extend(history.points());
        may_be_uninitialized_without_move |= history.may_be_uninitialized_without_move();
    });

    if may_be_uninitialized_without_move {
        diags.push(UseBeforeInitialization::new(use_span).into());
    }

    if move_points.is_empty() {
        return;
    }

    let move_spans = move_points.into_iter().map(|point| move_span(function, point)).collect();
    diags.push(match state {
        PlaceState::Partial(_) => UseAfterPartialMove::new(use_span, move_spans).into(),
        PlaceState::Uniform(PossibleStates::Uninitialized(_)) => {
            UseAfterMove::new(use_span, move_spans).into()
        }
        PlaceState::Uniform(PossibleStates::Initialized) => {
            unreachable!("initialized loads are filtered before diagnostic construction")
        }
    });
}

fn move_span(
    function: &rayc_ir::ir_function::IRFunction,
    point: Point,
) -> rayc_lexical::tree::RelativeSpan {
    let Instruction::Expression(expression_id) =
        &function.block_instructions(point.block_id())[point.instruction_idx()]
    else {
        unreachable!("only load instructions record move points")
    };
    function.get_expression(*expression_id).span()
}

use std::collections::{BTreeMap, BTreeSet};

use rayc_ir::{
    cfg::{ControlFlowEdge, Instruction, Point, Terminator},
    dataflow::DataflowProblem,
    ir_expr::IRExprKind,
    ir_function::{FunctionID, IRFunctionMap},
};
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;

use crate::{
    Diagnostic, PlaceState, PossibleStates, StackRoot, StackStateProblem,
    diagnostic::{
        MoveOutOfHandlerCapture, UseAfterMove, UseAfterPartialMove, UseBeforeInitialization,
    },
    drop_elaboration::{DropElaborator, PendingDrop, materialize_drops},
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
        // REVIEW: I think let's DropElaborator holds all the additional drop
        // insertions for the entire function and materializes them all at once
        // after `check_function` and `collect_place_drops` are done. This way
        // we can avoid having to allocate small Vecs for each drop insertion
        // and just to aggregate them all at once.
        let mut elaborator = DropElaborator::default();

        // Moves are checked against the IR as written, so an inserted drop is
        // never reported as the site of a move.
        let merge_drops = check_function(
            engine,
            def_id,
            functions,
            function_id,
            &mut elaborator,
            &mut diagnostics,
        )
        .await;

        insert_merge_drops(engine, functions, function_id, merge_drops).await;

        // Scope exits and reassignments are resolved against the balanced IR,
        // where every place is initialized on all paths or on none.
        let place_drops = collect_place_drops(
            engine,
            def_id,
            functions,
            function_id,
            &mut elaborator,
            &mut diagnostics,
        )
        .await;
        let mut insertions = BTreeMap::new();
        for (point, drops) in place_drops {
            insertions
                .insert(point, materialize_drops(engine, functions, function_id, drops).await);
        }
        functions.insert_instructions_before(function_id, insertions);
    }

    diagnostics
}

/// Reports loads of possibly uninitialized places, and moves out of captures
/// which the function only borrows, and returns the drops which balance the
/// stack on each control-flow edge into a merge.
async fn check_function(
    engine: &TrackedEngine,
    def_id: GlobalSymbolID,
    functions: &IRFunctionMap,
    function_id: FunctionID,
    elaborator: &mut DropElaborator,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<(ControlFlowEdge, Vec<PendingDrop>)> {
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
                            if let Some(place_state) = state.place_state(load.address())
                                && !place_state.is_initialized()
                            {
                                diagnostics_for_load(
                                    expression.span(),
                                    place_state,
                                    function,
                                    diagnostics,
                                );
                            }

                            // A borrowed capture is shared by every call, so
                            // nothing may be moved out of it, even briefly.
                            if problem.is_borrowed_capture(load.address())
                                && problem.load_moves(load, expression.ty().clone()).await
                            {
                                let root = StackRoot::from_address_root(load.address().root())
                                    .expect("a capture is a stack root");
                                let capture_span = problem.binding_span(root).await;
                                diagnostics.push(
                                    MoveOutOfHandlerCapture::new(expression.span(), capture_span)
                                        .into(),
                                );
                            }
                        }

                        IRExprKind::Error
                        | IRExprKind::Literal(_)
                        | IRExprKind::Binary(_)
                        | IRExprKind::Call(_)
                        | IRExprKind::RefOf(_)
                        | IRExprKind::Phi(_)
                        | IRExprKind::Perform(_)
                        | IRExprKind::Tuple(_)
                        | IRExprKind::Closure(_)
                        | IRExprKind::Handle(_)
                        | IRExprKind::StructInitialization(_) => {}
                    }
                }

                Instruction::ScopePush(_)
                | Instruction::ScopePop(_)
                | Instruction::ExprDiscard(_)
                | Instruction::Store(_) => {}
            }

            problem.transfer_instruction(point, instruction, &mut state).await.unwrap();
        }
    }

    // The analysis is not edge-sensitive, so an edge carries its source's
    // exit state unchanged.
    let mut merge_drops = Vec::new();
    for edge in solution.edges() {
        let (Some(exit), Some(entry)) =
            (solution.block_exit(edge.source()), solution.block_entry(edge.target()))
        else {
            continue;
        };

        let drops = elaborator.merge_drops(exit, entry, &mut problem, diagnostics).await;
        if !drops.is_empty() {
            merge_drops.push((*edge, drops));
        }
    }

    merge_drops
}

/// Places each edge's balancing drops where they run only when control
/// follows that edge.
async fn insert_merge_drops(
    engine: &TrackedEngine,
    functions: &mut IRFunctionMap,
    function_id: FunctionID,
    merge_drops: Vec<(ControlFlowEdge, Vec<PendingDrop>)>,
) {
    let mut insertions = BTreeMap::new();
    for (edge, drops) in merge_drops {
        let instructions = materialize_drops(engine, functions, function_id, drops).await;

        // A jump is the source's only edge, so the drops can end the source
        // block. A conditional edge is critical, since its target merges
        // several edges, so it gets a block of its own.
        let terminator = functions.get_function(function_id).block_terminator(edge.source());
        let block_id = match terminator {
            Some(Terminator::Jump(_)) => edge.source(),
            // this wouldn't be needed if we split every critical edge before dataflow analysis
            Some(Terminator::Conditional(_)) => functions.split_edge(function_id, edge),
            Some(Terminator::Return(_)) | None => {
                unreachable!("a control-flow edge leaves through a jump or a conditional")
            }
        };

        let instruction_idx =
            functions.get_function(function_id).block_instructions(block_id).len();
        let point = Point::builder().block_id(block_id).instruction_idx(instruction_idx).build();
        insertions.insert(point, instructions);
    }

    functions.insert_instructions_before(function_id, insertions);
}

/// Returns the drops which run before each scope exit and each reassignment,
/// keyed by the point of the `ScopePop` or `Store` they precede.
async fn collect_place_drops(
    engine: &TrackedEngine,
    def_id: GlobalSymbolID,
    functions: &IRFunctionMap,
    function_id: FunctionID,
    elaborator: &mut DropElaborator,
    diagnostics: &mut Vec<Diagnostic>,
) -> BTreeMap<Point, Vec<PendingDrop>> {
    let function = functions.get_function(function_id);
    let captures = functions.captures_for_function(function_id);
    let mut problem =
        StackStateProblem::new(Solver::new(engine.clone(), def_id).await, function, captures);

    // REVIEW: Woah! it seems that we have to solve the dataflow problem twice,
    // which is quite expensive. Why don't we reuse the dataflow solution for
    // both `check_function` and `collect_place_drops`? I assume that the
    // reason is because of the borrow checker? if that's the case, perhaps we
    // should aggregate all the drop insertions and then insert them all at once
    // after `check_function` and `collect_place_drops` are done, so that we don't
    // have to solve the dataflow problem twice. This is something we should
    // investigate.
    let solution = function.solve_dataflow(&mut problem).await.unwrap();

    let mut place_drops = BTreeMap::new();
    for block_id in solution.reachable_blocks() {
        let mut state = solution.block_entry(block_id).unwrap().clone();

        for (instruction_idx, instruction) in
            function.block_instructions(block_id).iter().enumerate()
        {
            let point =
                Point::builder().block_id(block_id).instruction_idx(instruction_idx).build();

            let drops = match instruction {
                // Values still initialized when their scope ends are dropped
                // just before the scope pops.
                Instruction::ScopePop(scope_id) => {
                    elaborator.scope_drops(*scope_id, &state, &mut problem, diagnostics).await
                }

                // The previous value of a reassigned place is dropped after
                // the new value is computed, just before it is stored. A new
                // value which moved the old one out leaves nothing to drop.
                Instruction::Store(store) => {
                    let ty = function.get_expression(store.expression()).ty().clone();
                    elaborator
                        .reassignment_drops(store.address(), ty, &state, &mut problem, diagnostics)
                        .await
                }

                Instruction::ScopePush(_)
                | Instruction::Expression(_)
                | Instruction::ExprDiscard(_) => Vec::new(),
            };
            if !drops.is_empty() {
                place_drops.insert(point, drops);
            }

            problem.transfer_instruction(point, instruction, &mut state).await.unwrap();
        }
    }
    place_drops
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

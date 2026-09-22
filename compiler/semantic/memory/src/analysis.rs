use std::collections::BTreeSet;

use rayc_ir::{
    cfg::{Instruction, Point},
    dataflow::DataflowProblem,
    ir_expr::IRExprKind,
    ir_function::IRFunctionMap,
};
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;

use crate::{
    Diagnostic, PlaceState, PossibleStates, StackStateProblem,
    diagnostic::{UseAfterMove, UseAfterPartialMove, UseBeforeInitialization},
};

/// Checks every reachable load without changing the supplied IR.
pub async fn analyze(
    engine: &TrackedEngine,
    def_id: GlobalSymbolID,
    functions: &IRFunctionMap,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    // Nested functions use the enclosing definition's predicates and target,
    // but each function owns an independent CFG and stack state.
    for (function_id, function) in functions.functions() {
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

                if let Instruction::Expression(expression_id) = instruction {
                    let expression = function.get_expression(*expression_id);
                    if let IRExprKind::Load(load) = expression.kind()
                        && let Some(place_state) = state.place_state(load.address())
                        && !place_state.is_initialized()
                    {
                        diagnostics_for_load(
                            expression.span(),
                            place_state,
                            function,
                            &mut diagnostics,
                        );
                    }
                }

                problem.transfer_instruction(point, instruction, &mut state).await.unwrap();
            }
        }
    }

    diagnostics
}

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

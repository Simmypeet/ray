use rayc_ir::{
    address::{Address, AddressRoot, Projection as IRProjection},
    cfg::{BlockID as IRBlockID, Instruction as IRInstruction, Terminator as IRTerminator},
    ir_expr::IRExprKind,
    ir_function::IRFunction,
};
use rayc_mono_ir::{
    MonoIR,
    cfg::{BlockID, Branch, Terminator},
    function::{Local, LocalKind},
    instruction::{Assign, Instruction},
    operand::{Constant, Operand},
    place::{FieldIndex, Place},
    rvalue::Rvalue,
};

use super::{Builder, FunctionState};

impl Builder {
    pub(super) fn lower_address(address: &Address, state: &FunctionState) -> Place {
        let mut place = match address.root() {
            AddressRoot::Error => {
                panic!("compiler-internal invariant violation: error address reached MonoIR")
            }
            AddressRoot::Variable(variable) => state.variable_place(variable),
            AddressRoot::Parameter(parameter) => state.parameter_place(parameter),
            AddressRoot::LambdaParameter(parameter) => state.lambda_parameter_place(parameter),
            AddressRoot::OperationHandlerParameter(parameter) => {
                state.operation_parameter_place(parameter)
            }
            AddressRoot::Capture(capture) => state.capture_place(capture),
            AddressRoot::Deref(expression) => state.expression_place(expression).dereference(),
        };
        for projection in address.projections() {
            match projection {
                IRProjection::Tuple(index) => {
                    place =
                        place.project_tuple_field(FieldIndex::new((*index).try_into().unwrap()));
                }
            }
        }
        place
    }

    pub(super) fn lower_terminator(
        source_block: IRBlockID,
        target_block: BlockID,
        terminator: &IRTerminator,
        source: &IRFunction,
        state: &FunctionState,
        output: &mut MonoIR,
    ) {
        let terminator = match terminator {
            IRTerminator::Jump(successor) => {
                Terminator::Goto(Self::lower_edge(source_block, *successor, source, state, output))
            }
            IRTerminator::Conditional(conditional) => Terminator::Branch(Branch::new(
                state.expression_operand(conditional.condition()),
                Self::lower_edge(source_block, conditional.then_block(), source, state, output),
                Self::lower_edge(source_block, conditional.else_block(), source, state, output),
            )),
            IRTerminator::Return(value) => Terminator::Return(Some(value.map_or_else(
                || Operand::Constant(Constant::Unit),
                |value| state.expression_operand(value),
            ))),
        };
        output.set_terminator(state.target_id, target_block, terminator);
    }

    /// Splits an incoming edge when its successor contains phi expressions.
    ///
    /// All incoming values are first copied to temporaries, then written to
    /// their phi destinations. This preserves parallel-copy semantics when phi
    /// inputs and destinations overlap.
    fn lower_edge(
        predecessor: IRBlockID,
        successor: IRBlockID,
        source: &IRFunction,
        state: &FunctionState,
        output: &mut MonoIR,
    ) -> BlockID {
        let phis = source
            .block_instructions(successor)
            .iter()
            .filter_map(|instruction| {
                let IRInstruction::Expression(expression_id) = instruction else {
                    return None;
                };
                let IRExprKind::Phi(phi) = source.get_expression(*expression_id).kind() else {
                    return None;
                };
                Some((*expression_id, phi))
            })
            .collect::<Vec<_>>();
        if phis.is_empty() {
            return state.block(successor);
        }

        let edge = output.create_block(state.target_id);
        let mut copies = Vec::with_capacity(phis.len());
        for (phi_id, phi) in &phis {
            let incoming = phi.value_from(predecessor).unwrap_or_else(|| {
                panic!("phi {phi_id:?} has no input for predecessor {predecessor:?}")
            });
            let destination = state.expression_place(*phi_id);
            let ty =
                output.get_function(state.target_id).get_local(destination.local()).ty().clone();
            let temporary =
                output.insert_local(state.target_id, Local::new(ty, LocalKind::Temporary));
            output.push_instruction(
                state.target_id,
                edge,
                Instruction::Assign(Assign::new(
                    Place::new(temporary),
                    Rvalue::Use(state.expression_operand(incoming)),
                )),
            );
            copies.push((destination, temporary));
        }
        for (destination, temporary) in copies {
            output.push_instruction(
                state.target_id,
                edge,
                Instruction::Assign(Assign::new(
                    destination,
                    Rvalue::Use(Operand::Copy(Place::new(temporary))),
                )),
            );
        }
        output.set_terminator(state.target_id, edge, Terminator::Goto(state.block(successor)));
        edge
    }
}

use rayc_ir::{
    address::{Address, AddressRoot, Projection as IRProjection},
    cfg::{BlockID as IRBlockID, Instruction as IRInstruction, Terminator as IRTerminator},
    ir_expr::IRExprKind,
    ir_function::IRFunction,
};
use rayc_mono_ir::{
    cfg::{BlockID, Branch, Terminator},
    function::{Local, LocalKind},
    instruction::{Assign, Instruction},
    operand::{Constant, Operand},
    place::{FieldIndex, Place},
    rvalue::Rvalue,
};

use super::Builder;
use crate::context::Context;

impl Context {
    pub(super) fn lower_address(address: &Address, builder: &Builder<'_>) -> Place {
        let mut place = match address.root() {
            AddressRoot::Error => {
                panic!("compiler-internal invariant violation: error address reached MonoIR")
            }
            AddressRoot::Variable(variable) => builder.variable_place(variable),
            AddressRoot::Parameter(parameter) => builder.parameter_place(parameter),
            AddressRoot::LambdaParameter(parameter) => builder.lambda_parameter_place(parameter),
            AddressRoot::OperationHandlerParameter(parameter) => {
                builder.operation_parameter_place(parameter)
            }
            AddressRoot::Capture(capture) => builder.capture_place(capture),
            AddressRoot::Deref(expression) => builder.expression_place(expression).dereference(),
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
        builder: &mut Builder<'_>,
    ) {
        let terminator = match terminator {
            IRTerminator::Jump(successor) => {
                Terminator::Goto(Self::lower_edge(source_block, *successor, source, builder))
            }
            IRTerminator::Conditional(conditional) => Terminator::Branch(Branch::new(
                builder.expression_operand(conditional.condition()),
                Self::lower_edge(source_block, conditional.then_block(), source, builder),
                Self::lower_edge(source_block, conditional.else_block(), source, builder),
            )),
            IRTerminator::Return(value) => Terminator::Return(Some(value.map_or_else(
                || Operand::Constant(Constant::Unit),
                |value| builder.expression_operand(value),
            ))),
        };
        builder.set_terminator(target_block, terminator);
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
        builder: &mut Builder<'_>,
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
            return builder.block(successor);
        }

        let edge = builder.create_block();
        let mut copies = Vec::with_capacity(phis.len());
        for (phi_id, phi) in &phis {
            let incoming = phi.value_from(predecessor).unwrap_or_else(|| {
                panic!("phi {phi_id:?} has no input for predecessor {predecessor:?}")
            });
            let destination = builder.expression_place(*phi_id);
            let ty = builder.local_type(destination.local());
            let temporary = builder.insert_local(Local::new(ty, LocalKind::Temporary));
            builder.push_instruction(
                edge,
                Instruction::Assign(Assign::new(
                    Place::new(temporary),
                    Rvalue::Use(builder.expression_operand(incoming)),
                )),
            );
            copies.push((destination, temporary));
        }
        for (destination, temporary) in copies {
            builder.push_instruction(
                edge,
                Instruction::Assign(Assign::new(
                    destination,
                    Rvalue::Use(Operand::Copy(Place::new(temporary))),
                )),
            );
        }
        builder.set_terminator(edge, Terminator::Goto(builder.block(successor)));
        edge
    }
}

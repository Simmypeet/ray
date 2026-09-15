use rayc_ir::{
    address::{Address, AddressRoot, Projection as IRProjection},
    cfg::{BlockID as IRBlockID, Instruction as IRInstruction, Terminator as IRTerminator},
    ir_expr::IRExprKind,
    ir_function::IRFunction,
};
use rayc_mono_ir::{
    cfg::{BlockID, Branch, Terminator},
    instruction::{Assign, Instruction},
    operand::{Constant, Operand},
    place::{FieldIndex, Place},
    rvalue::Rvalue,
};

use crate::builder::Builder;

impl Builder<'_> {
    pub(super) fn lower_address(&self, address: &Address) -> Place {
        let mut place = match address.root() {
            AddressRoot::Error => {
                panic!("compiler-internal invariant violation: error address reached MonoIR")
            }
            AddressRoot::Variable(variable) => self.variable_place(variable),
            AddressRoot::Parameter(parameter) => self.parameter_place(parameter),
            AddressRoot::LambdaParameter(parameter) => self.lambda_parameter_place(parameter),
            AddressRoot::OperationHandlerParameter(parameter) => {
                self.operation_parameter_place(parameter)
            }
            AddressRoot::Capture(capture) => self.capture_place(capture),
            AddressRoot::Deref(expression) => self.expression_place(expression).dereference(),
        };
        for projection in address.projections() {
            match projection {
                IRProjection::Tuple(index) => {
                    place =
                        place.project_tuple_field(FieldIndex::new((*index).try_into().unwrap()));
                }
                IRProjection::Field(_) => todo!("lower struct field access addresses"),
            }
        }
        place
    }

    pub(super) fn lower_terminator(
        &mut self,
        source_block: IRBlockID,
        terminator: &IRTerminator,
        source: &IRFunction,
    ) {
        let terminator = match terminator {
            IRTerminator::Jump(successor) => {
                Terminator::Goto(self.lower_edge(source_block, *successor, source))
            }
            IRTerminator::Conditional(conditional) => Terminator::Branch(Branch::new(
                self.expression_operand(conditional.condition()),
                self.lower_edge(source_block, conditional.then_block(), source),
                self.lower_edge(source_block, conditional.else_block(), source),
            )),
            IRTerminator::Return(value) => Terminator::Return(Some(value.map_or_else(
                || Operand::Constant(Constant::Unit),
                |value| self.expression_operand(value),
            ))),
        };
        self.set_terminator(terminator);
    }

    /// Splits an incoming edge when its successor contains phi expressions.
    ///
    /// All incoming values are first copied to temporaries, then written to
    /// their phi destinations. This preserves parallel-copy semantics when phi
    /// inputs and destinations overlap.
    fn lower_edge(
        &mut self,
        predecessor: IRBlockID,
        successor: IRBlockID,
        source: &IRFunction,
    ) -> BlockID {
        let mut phis = source.block_instructions(successor).iter().filter_map(|instruction| {
            let IRInstruction::Expression(expression_id) = instruction else {
                return None;
            };
            let IRExprKind::Phi(phi) = source.get_expression(*expression_id).kind() else {
                return None;
            };
            Some((*expression_id, phi))
        });

        let first = phis.next();
        if first.is_none() {
            return self.block(successor);
        }

        // create a temporary block between the predecessor and successor to perform the
        // phi copies
        let edge = self.create_block();
        self.select_block(edge);

        // place all incoming phi values into their destination locals.
        for (phi_id, phi) in first.into_iter().chain(phis) {
            let incoming = phi.value_from(predecessor).unwrap_or_else(|| {
                panic!("phi {phi_id:?} has no input for predecessor {predecessor:?}")
            });

            let destination = self.expression_place(phi_id);
            self.push_instruction(Instruction::Assign(Assign::new(
                destination,
                Rvalue::Use(self.expression_operand(incoming)),
            )));
        }

        self.set_terminator(Terminator::Goto(self.block(successor)));
        self.select_block(self.block(predecessor));
        edge
    }
}

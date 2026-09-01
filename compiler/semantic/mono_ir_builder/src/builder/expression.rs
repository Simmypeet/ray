use qbice::storage::intern::Interned;
use rayc_ir::{
    ir_expr::{
        IRExprID, IRExprKind, binary::BinaryOp as IRBinaryOperator, literal::Literal, tuple::Tuple,
    },
    ir_function::IRFunction,
};
use rayc_mono_ir::{
    cfg::BlockID,
    instruction::{Assign, Instruction},
    operand::{Constant, Operand},
    place::Place,
    rvalue::{AddressOf, AggregateValue, Binary, BinaryOperator, Rvalue},
    ty::MonoType,
};

use super::{Builder, BuilderState};

impl Builder {
    pub(super) async fn lower_expression(
        &mut self,
        expression_id: IRExprID,
        block: BlockID,
        source: &IRFunction,
        builder_state: &mut BuilderState<'_>,
    ) {
        let expression = source.get_expression(expression_id);
        let destination = builder_state.expression_place(expression_id);
        match expression.kind() {
            IRExprKind::Error => {
                panic!("compiler-internal invariant violation: error expression reached MonoIR")
            }
            IRExprKind::Literal(literal) => {
                let ty = builder_state.local_type(destination.local());
                let constant = lower_literal(literal, &ty);
                Self::assign(
                    block,
                    destination,
                    Rvalue::Use(Operand::Constant(constant)),
                    builder_state,
                );
            }
            IRExprKind::RefOf(reference) => {
                let ty = builder_state.local_type(destination.local());
                let MonoType::Pointer(pointer) = &*ty else {
                    panic!("RefOf should produce a pointer type")
                };
                Self::assign(
                    block,
                    destination,
                    Rvalue::AddressOf(AddressOf::new(
                        Self::lower_address(reference.address(), builder_state),
                        pointer.mutability(),
                    )),
                    builder_state,
                );
            }
            IRExprKind::Load(load) => {
                Self::assign(
                    block,
                    destination,
                    Rvalue::Use(Operand::Copy(Self::lower_address(load.address(), builder_state))),
                    builder_state,
                );
            }
            IRExprKind::Phi(_) => {}
            IRExprKind::Binary(binary) => {
                let operator = match binary.operator() {
                    IRBinaryOperator::Equal => BinaryOperator::Equal,
                    IRBinaryOperator::NotEqual => BinaryOperator::NotEqual,
                    IRBinaryOperator::Plus => BinaryOperator::Add,
                    IRBinaryOperator::Minus => BinaryOperator::Subtract,
                    IRBinaryOperator::Multiply => BinaryOperator::Multiply,
                    IRBinaryOperator::Divide => BinaryOperator::Divide,
                };
                Self::assign(
                    block,
                    destination,
                    Rvalue::Binary(Binary::new(
                        builder_state.expression_operand(binary.left()),
                        operator,
                        builder_state.expression_operand(binary.right()),
                    )),
                    builder_state,
                );
            }
            IRExprKind::Call(call) => {
                self.lower_call(call, expression_id, block, source, builder_state).await;
            }
            IRExprKind::Perform(perform) => {
                self.lower_perform(perform, expression_id, block, builder_state).await;
            }
            IRExprKind::Handle(handle) => {
                self.lower_handle(handle, expression_id, block, builder_state).await;
            }
            IRExprKind::Tuple(tuple) => {
                Self::lower_tuple(tuple, block, destination, builder_state);
            }
            IRExprKind::MakeLambda(lambda) => {
                self.lower_make_lambda(lambda, expression_id, block, builder_state);
            }
        }
    }

    fn lower_tuple(
        tuple: &Tuple,
        block: BlockID,
        destination: Place,
        builder_state: &mut BuilderState<'_>,
    ) {
        let ty = builder_state.local_type(destination.local());
        let value = if matches!(&*ty, MonoType::Unit) {
            Rvalue::Use(Operand::Constant(Constant::Unit))
        } else {
            let fields = tuple
                .elements()
                .iter()
                .map(|element| builder_state.expression_operand(*element))
                .collect();
            Rvalue::Aggregate(AggregateValue::new(ty, fields))
        };
        Self::assign(block, destination, value, builder_state);
    }

    pub(super) fn assign(
        block: BlockID,
        destination: Place,
        value: Rvalue,
        builder_state: &mut BuilderState<'_>,
    ) {
        builder_state.push_instruction(block, Instruction::Assign(Assign::new(destination, value)));
    }
}

#[allow(clippy::cast_precision_loss)]
fn lower_literal(literal: &Literal, ty: &Interned<MonoType>) -> Constant {
    match literal {
        Literal::Numeric(value) => match &**ty {
            MonoType::Int32 => Constant::Int32((*value).try_into().unwrap()),
            MonoType::Float32 => Constant::new_float32(*value as f32),
            MonoType::CInt => Constant::CInt((*value).try_into().unwrap()),
            MonoType::Unit
            | MonoType::Bool
            | MonoType::CStr
            | MonoType::OpaquePointer(_)
            | MonoType::Pointer(_)
            | MonoType::Aggregate(_)
            | MonoType::FunctionPointer(_) => {
                panic!("numeric literal has a non-numeric MonoIR type")
            }
        },
        Literal::Bool(value) => Constant::Bool(*value),
        Literal::String(value) => Constant::CStr(value.clone()),
    }
}

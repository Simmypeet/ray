use qbice::storage::intern::Interned;
use rayc_ir::{
    ir_expr::{
        IRExprID, IRExprKind, binary::BinaryOp as IRBinaryOperator, literal::Literal,
        struct_initialization::StructInitialization, tuple::Tuple,
    },
    ir_function::IRFunction,
};
use rayc_mono_ir::{
    instruction::{Assign, Instruction},
    operand::{Constant, Operand},
    place::Place,
    rvalue::{AddressOf, Binary, BinaryOperator, Rvalue},
    ty::MonoType,
};

use crate::{builder::Builder, context::Context};

impl Builder<'_> {
    pub(super) async fn lower_expression(
        &mut self,
        context: &Context,
        expression_id: IRExprID,
        source: &IRFunction,
    ) {
        let expression = source.get_expression(expression_id);
        let destination = self.expression_place(expression_id);

        match expression.kind() {
            IRExprKind::Error => {
                panic!("compiler-internal invariant violation: error expression reached MonoIR")
            }
            IRExprKind::Literal(literal) => {
                let ty = self.local_type(destination.local());
                let constant = lower_literal(literal, ty);
                self.assign(destination, Rvalue::Use(Operand::Constant(constant)));
            }
            IRExprKind::RefOf(reference) => {
                let ty = self.local_type(destination.local());
                let MonoType::Pointer(pointer) = &**ty else {
                    panic!("RefOf should produce a pointer type")
                };
                self.assign(
                    destination,
                    Rvalue::AddressOf(AddressOf::new(
                        self.lower_address(reference.address()),
                        pointer.mutability(),
                    )),
                );
            }
            // References and raw pointers share a representation, so the
            // coercion only forwards the address.
            IRExprKind::RefToPointer(coercion) => {
                self.assign(
                    destination,
                    Rvalue::Use(self.expression_operand(coercion.reference())),
                );
            }
            IRExprKind::Load(load) => {
                self.assign(
                    destination,
                    Rvalue::Use(Operand::Copy(self.lower_address(load.address()))),
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
                self.assign(
                    destination,
                    Rvalue::Binary(Binary::new(
                        self.expression_operand(binary.left()),
                        operator,
                        self.expression_operand(binary.right()),
                    )),
                );
            }
            IRExprKind::Call(call) => {
                self.lower_call(context, call, expression_id).await;
            }
            IRExprKind::Perform(perform) => {
                self.lower_perform(context, perform, expression_id).await;
            }
            IRExprKind::Handle(handle) => {
                self.lower_handle(context, handle, expression_id).await;
            }
            IRExprKind::Tuple(tuple) => {
                self.lower_tuple(tuple, destination);
            }

            IRExprKind::StructInitialization(st) => {
                self.lower_struct(st, destination);
            }

            IRExprKind::Closure(lambda) => self.lower_closure(context, lambda, expression_id),
        }
    }

    fn lower_struct(&mut self, st: &StructInitialization, destination: Place) {
        let ty = self.local_type(destination.local());
        let ty = ty.assert_as_struct();

        let fields = st
            .initializers()
            .iter()
            .map(|(field_id, field)| (*field_id, self.expression_operand(*field)))
            .collect();

        let value = Rvalue::new_struct(ty.clone(), fields);
        self.assign(destination, value);
    }

    fn lower_tuple(&mut self, tuple: &Tuple, destination: Place) {
        let ty = self.local_type(destination.local());
        let ty = ty.assert_as_tuple();

        let value = if ty.is_empty() {
            Rvalue::Use(Operand::Constant(Constant::Unit))
        } else {
            let fields =
                tuple.elements().iter().map(|element| self.expression_operand(*element)).collect();

            Rvalue::new_tuple(ty.clone(), fields)
        };

        self.assign(destination, value);
    }

    pub(super) fn assign(&mut self, destination: Place, value: Rvalue) {
        self.push_instruction(Instruction::Assign(Assign::new(destination, value)));
    }
}

#[allow(clippy::cast_precision_loss)]
fn lower_literal(literal: &Literal, ty: &Interned<MonoType>) -> Constant {
    match literal {
        Literal::Numeric(value) => match &**ty {
            MonoType::Int32 => Constant::Int32((*value).try_into().unwrap()),
            MonoType::Float32 => Constant::new_float32(*value as f32),
            MonoType::CInt => Constant::CInt((*value).try_into().unwrap()),
            MonoType::Bool
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

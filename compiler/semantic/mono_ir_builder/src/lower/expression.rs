use qbice::storage::intern::Interned;
use rayc_ir::ir_expr::{
    IRExprID, IRExprKind,
    binary::{Binary as IRBinary, BinaryOp as IRBinaryOperator},
    literal::Literal,
    struct_initialization::StructInitialization,
    tuple::Tuple,
};
use rayc_mono_ir::{
    operand::{Constant, Operand},
    place::Place,
    rvalue::{AddressOf, Binary, BinaryOperator, Rvalue},
    ty::{MonoType, Tuple as TupleTy},
};

use crate::function_lowering::FunctionLowering;

impl FunctionLowering<'_> {
    pub(super) async fn lower_expression(&mut self, expression_id: IRExprID) {
        let destination = self.expression_place(expression_id);

        match self.source_expression(expression_id).kind() {
            IRExprKind::Error => {
                panic!("compiler-internal invariant violation: error expression reached MonoIR")
            }
            IRExprKind::Literal(literal) => {
                let constant = lower_literal(literal, self.expression_type(expression_id));
                self.store_constant(destination, constant);
            }
            IRExprKind::RefOf(reference) => {
                let ty = self.expression_type(expression_id).assert_as_pointer();
                let address = self.lower_address(reference.address());
                let value = Rvalue::AddressOf(AddressOf::new(address, ty.mutability()));
                self.assign(destination, value);
            }
            // References and raw pointers share a representation, so the
            // coercion only forwards the address.
            IRExprKind::RefToPointer(coercion) => {
                self.store(destination, self.expression_operand(coercion.reference()));
            }
            IRExprKind::Load(load) => {
                self.store(destination, Operand::Copy(self.lower_address(load.address())));
            }
            IRExprKind::Phi(_) => {}
            IRExprKind::Binary(binary) => self.lower_binary(binary, destination),
            IRExprKind::Call(call) => self.lower_call_in_callback_style(call, expression_id).await,
            IRExprKind::Perform(perform) => self.lower_perform(perform, expression_id).await,
            IRExprKind::RunWith(run_with) => self.lower_run_with(run_with, expression_id).await,
            IRExprKind::Tuple(tuple) => self.lower_tuple(tuple, expression_id),
            IRExprKind::StructInitialization(st) => self.lower_struct(st, expression_id).await,
            IRExprKind::Closure(closure) => self.lower_closure(closure, expression_id),
            IRExprKind::Coroutine(coroutine) => {
                self.lower_coro(coroutine, expression_id).await;
            }

            // No instruction evaluates them: a `Handle` terminator defines
            // them on its outgoing edges.
            IRExprKind::Handle(_) | IRExprKind::Yielded(_) => {
                panic!("a `Handle` terminator should evaluate its expressions")
            }
        }
    }

    fn lower_binary(&mut self, binary: &IRBinary, destination: Place) {
        let value = Binary::new(
            self.expression_operand(binary.left()),
            lower_binary_operator(binary.operator()),
            self.expression_operand(binary.right()),
        );
        self.assign(destination, Rvalue::Binary(value));
    }

    async fn lower_struct(&mut self, st: &StructInitialization, expression_id: IRExprID) {
        let ty = self.expression_type(expression_id).assert_as_struct();
        let layout = self.resolver().struct_layout(ty).await;

        let fields = st
            .initializers()
            .iter()
            .map(|(field_id, field)| (*field_id, self.expression_operand(*field)))
            .collect();

        let value = Rvalue::new_struct(&layout, fields);
        self.assign(self.expression_place(expression_id), value);
    }

    fn lower_tuple(&mut self, tuple: &Tuple, expression_id: IRExprID) {
        let ty = self.expression_type(expression_id).assert_as_tuple();
        let fields =
            tuple.elements().iter().map(|element| self.expression_operand(*element)).collect();

        let value = tuple_value(ty, fields);
        self.assign(self.expression_place(expression_id), value);
    }
}

/// The value of a tuple of type `ty`. The empty tuple is the unit constant.
pub(super) fn tuple_value(ty: &TupleTy, fields: Vec<Operand>) -> Rvalue {
    if ty.is_empty() {
        Rvalue::Use(Operand::Constant(Constant::Unit))
    } else {
        Rvalue::new_tuple(ty.clone(), fields)
    }
}

const fn lower_binary_operator(operator: IRBinaryOperator) -> BinaryOperator {
    match operator {
        IRBinaryOperator::Equal => BinaryOperator::Equal,
        IRBinaryOperator::NotEqual => BinaryOperator::NotEqual,
        IRBinaryOperator::Plus => BinaryOperator::Add,
        IRBinaryOperator::Minus => BinaryOperator::Subtract,
        IRBinaryOperator::Multiply => BinaryOperator::Multiply,
        IRBinaryOperator::Divide => BinaryOperator::Divide,
    }
}

#[allow(clippy::cast_precision_loss)]
fn lower_literal(literal: &Literal, ty: &Interned<MonoType>) -> Constant {
    match literal {
        Literal::Numeric(value) => match &**ty {
            MonoType::Int8 => Constant::Int8(fit(*value)),
            MonoType::Int16 => Constant::Int16(fit(*value)),
            MonoType::Int32 => Constant::Int32(fit(*value)),
            MonoType::Int64 => Constant::Int64(fit(*value)),
            MonoType::Isize => Constant::Isize(fit(*value)),
            MonoType::Uint8 => Constant::Uint8(fit(*value)),
            MonoType::Uint16 => Constant::Uint16(fit(*value)),
            MonoType::Uint32 => Constant::Uint32(fit(*value)),
            MonoType::Uint64 => Constant::Uint64(fit(*value)),
            MonoType::Usize => Constant::Usize(fit(*value)),
            MonoType::Float32 => Constant::new_float32(*value as f32),
            MonoType::CInt => Constant::CInt(fit(*value)),
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

/// Converts a numeric literal to the representation of its type. The typed AST
/// builder rejects every literal that does not fit in its type.
fn fit<T: TryFrom<u128>>(value: u128) -> T {
    T::try_from(value)
        .unwrap_or_else(|_| panic!("numeric literal `{value}` should fit in its type"))
}

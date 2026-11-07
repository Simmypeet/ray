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
    place::{Place, Projection},
    rvalue::{AddressOf, Binary, BinaryOperator, Rvalue},
    ty::MonoType,
};

use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    pub(super) async fn lower_expression(&mut self, expression_id: IRExprID, ctx: &Context<'_>) {
        let destination = ctx.expression_place(expression_id);

        match ctx.source_expression(expression_id).kind() {
            IRExprKind::Error => {
                panic!("compiler-internal invariant violation: error expression reached MonoIR")
            }
            IRExprKind::Literal(literal) => {
                let constant = lower_literal(literal, ctx.expression_type(expression_id));
                self.store_constant(destination, constant);
            }
            IRExprKind::RefOf(reference) => {
                let ty = ctx.expression_type(expression_id).assert_as_pointer();
                let address = self.lower_address(reference.address(), ctx);
                let value = Rvalue::AddressOf(AddressOf::new(address, ty.mutability()));
                self.assign(destination, value);
            }
            // References and raw pointers share a representation, so the
            // coercion only forwards the address.
            IRExprKind::RefToPointer(coercion) => {
                self.store(destination, ctx.expression_operand(coercion.reference()));
            }
            IRExprKind::Load(load) => {
                self.store(destination, Operand::Copy(self.lower_address(load.address(), ctx)));
            }
            IRExprKind::Phi(_) => {}
            IRExprKind::Binary(binary) => self.lower_binary(binary, destination, ctx),
            IRExprKind::Call(call) => {
                self.lower_call_in_callback_style(call, expression_id, ctx).await;
            }
            IRExprKind::Perform(perform) => {
                self.lower_callback_perform(perform, expression_id, ctx).await;
            }
            IRExprKind::RunWith(run_with) => {
                self.lower_callback_run_with(run_with, expression_id, ctx).await;
            }
            IRExprKind::Tuple(tuple) => self.lower_tuple(tuple, expression_id, ctx),
            IRExprKind::StructInitialization(st) => self.lower_struct(st, expression_id, ctx).await,
            IRExprKind::Closure(closure) => self.lower_closure(closure, expression_id, ctx),
            IRExprKind::Coroutine(coroutine) => {
                self.lower_coro(coroutine, expression_id, ctx).await;
            }

            // No instruction evaluates them: a `Handle` terminator defines
            // them on its outgoing edges.
            IRExprKind::Handle(_) | IRExprKind::Yielded(_) => {
                panic!("a `Handle` terminator should evaluate its expressions")
            }
        }
    }

    fn lower_binary(&mut self, binary: &IRBinary, destination: Place, ctx: &Context<'_>) {
        let value = Binary::new(
            ctx.expression_operand(binary.left()),
            lower_binary_operator(binary.operator()),
            ctx.expression_operand(binary.right()),
        );
        self.assign(destination, Rvalue::Binary(value));
    }

    async fn lower_struct(
        &mut self,
        st: &StructInitialization,
        expression_id: IRExprID,
        ctx: &Context<'_>,
    ) {
        let ty = ctx.expression_type(expression_id).assert_as_struct();
        let layout = self.resolver().struct_layout(ty).await;

        assert_eq!(layout.fields().len(), st.initializers().len());

        let destination = ctx.expression_place(expression_id);
        self.initialize_aggregate(
            &destination,
            st.initializers().iter().map(|(field_id, field)| {
                (Projection::StructFieldIndex(*field_id), ctx.expression_operand(*field))
            }),
        );
    }

    fn lower_tuple(&mut self, tuple: &Tuple, expression_id: IRExprID, ctx: &Context<'_>) {
        let destination = ctx.expression_place(expression_id);

        self.initialize_tuple(
            &destination,
            tuple.elements().iter().map(|element| ctx.expression_operand(*element)),
        );
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

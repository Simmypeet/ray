use qbice::storage::intern::Interned;
use rayc_ir::{
    cfg::{Conditional, Terminator},
    ir_expr::{
        IRExpr, IRExprKind,
        binary::{Binary as IrBinary, BinaryOp as IrBinaryOp},
        literal::Literal,
        phi::Phi,
    },
};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;
use rayc_typed_ast::typed_expr::binary::{Binary, BinaryOp};

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a Binary>> for Builder {
    fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Binary>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let binary = expression.node();
        match binary.operator() {
            BinaryOp::Assign => lower_assignment(self, context, binary),
            BinaryOp::Equal => lower_binary(self, context, binary, IrBinaryOp::Equal, span, ty),
            BinaryOp::NotEqual => {
                lower_binary(self, context, binary, IrBinaryOp::NotEqual, span, ty)
            }
            BinaryOp::Plus => lower_binary(self, context, binary, IrBinaryOp::Plus, span, ty),
            BinaryOp::Minus => lower_binary(self, context, binary, IrBinaryOp::Minus, span, ty),
            BinaryOp::Multiply => {
                lower_binary(self, context, binary, IrBinaryOp::Multiply, span, ty)
            }
            BinaryOp::Divide => lower_binary(self, context, binary, IrBinaryOp::Divide, span, ty),
            BinaryOp::And => lower_logical(self, context, binary, false, span, ty),
            BinaryOp::Or => lower_logical(self, context, binary, true, span, ty),
        }
    }
}

fn lower_assignment(
    builder: &mut Builder,
    context: &LoweringContext<'_>,
    binary: &Binary,
) -> LoweredExpression {
    let address = builder.lower_lvalue_by_id(context, binary.left());
    let value = builder.lower_rvalue_by_id(context, binary.right());
    builder.emit_store(address, value);
    LoweredExpression::RValue(value)
}

fn lower_binary(
    builder: &mut Builder,
    context: &LoweringContext<'_>,
    binary: &Binary,
    operator: IrBinaryOp,
    span: RelativeSpan,
    ty: Interned<Ty>,
) -> LoweredExpression {
    let left = builder.lower_rvalue_by_id(context, binary.left());
    let right = builder.lower_rvalue_by_id(context, binary.right());
    LoweredExpression::RValue(builder.emit_expression(IRExpr::new(
        IRExprKind::Binary(IrBinary::new(left, operator, right)),
        span,
        ty,
    )))
}

fn lower_logical(
    builder: &mut Builder,
    context: &LoweringContext<'_>,
    binary: &Binary,
    short_circuit_value: bool,
    span: RelativeSpan,
    ty: Interned<Ty>,
) -> LoweredExpression {
    let left = builder.lower_rvalue_by_id(context, binary.left());
    let rhs_block = builder.create_block();
    let short_circuit_block = builder.create_block();
    let merge_block = builder.create_block();

    let (then_block, else_block) = if short_circuit_value {
        (short_circuit_block, rhs_block)
    } else {
        (rhs_block, short_circuit_block)
    };
    builder.terminate(Terminator::Conditional(Conditional::new(left, then_block, else_block)));

    builder.select_block(short_circuit_block);
    let constant = builder.emit_expression(IRExpr::new(
        IRExprKind::Literal(Literal::Bool(short_circuit_value)),
        span,
        ty.clone(),
    ));
    let short_circuit_predecessor = builder.jump_to(merge_block);

    builder.select_block(rhs_block);
    let rhs = builder.lower_rvalue_by_id(context, binary.right());
    let rhs_predecessor = builder.jump_to(merge_block);

    builder.select_block(merge_block);
    let incoming =
        [(short_circuit_predecessor, constant), (rhs_predecessor, rhs)].into_iter().collect();
    LoweredExpression::RValue(builder.emit_expression(IRExpr::new(
        IRExprKind::Phi(Phi::new(incoming)),
        span,
        ty,
    )))
}

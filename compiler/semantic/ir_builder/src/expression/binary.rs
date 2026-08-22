use qbice::storage::intern::Interned;
use rayc_ir::{
    cfg::{Conditional, Terminator},
    expression::{
        Expression, ExpressionID, ExpressionKind,
        binary::{Binary as IrBinary, BinaryOp as IrBinaryOp},
        literal::Literal,
        phi::Phi,
    },
};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    function::Function as TypedFunction,
    typed_expr::binary::{Binary, BinaryOp},
};

use crate::{
    builder::Builder,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a Binary>> for Builder {
    fn lower_expression(
        &mut self,
        expression: TypedExprWithID<&'a Binary>,
        typed_function: &TypedFunction,
    ) -> ExpressionID {
        let typed_expression = typed_function.get_expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let binary = expression.node();
        match binary.operator() {
            BinaryOp::Assign => lower_assignment(self, typed_function, binary),
            BinaryOp::Plus => {
                lower_arithmetic(self, typed_function, binary, IrBinaryOp::Plus, span, ty)
            }
            BinaryOp::Minus => {
                lower_arithmetic(self, typed_function, binary, IrBinaryOp::Minus, span, ty)
            }
            BinaryOp::Multiply => {
                lower_arithmetic(self, typed_function, binary, IrBinaryOp::Multiply, span, ty)
            }
            BinaryOp::Divide => {
                lower_arithmetic(self, typed_function, binary, IrBinaryOp::Divide, span, ty)
            }
            BinaryOp::And => lower_logical(self, typed_function, binary, false, span, ty),
            BinaryOp::Or => lower_logical(self, typed_function, binary, true, span, ty),
        }
    }
}

fn lower_assignment(
    builder: &mut Builder,
    typed_function: &TypedFunction,
    binary: &Binary,
) -> ExpressionID {
    let address = builder.lower_address_by_id(typed_function, binary.left());
    let value = builder.lower_expression_by_id(typed_function, binary.right());
    builder.emit_store(address, value);
    value
}

fn lower_arithmetic(
    builder: &mut Builder,
    typed_function: &TypedFunction,
    binary: &Binary,
    operator: IrBinaryOp,
    span: RelativeSpan,
    ty: Interned<Ty>,
) -> ExpressionID {
    let left = builder.lower_expression_by_id(typed_function, binary.left());
    let right = builder.lower_expression_by_id(typed_function, binary.right());
    builder.emit_expression(Expression::new(
        ExpressionKind::Binary(IrBinary::new(left, operator, right)),
        span,
        ty,
    ))
}

fn lower_logical(
    builder: &mut Builder,
    typed_function: &TypedFunction,
    binary: &Binary,
    short_circuit_value: bool,
    span: RelativeSpan,
    ty: Interned<Ty>,
) -> ExpressionID {
    let left = builder.lower_expression_by_id(typed_function, binary.left());
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
    let constant = builder.emit_expression(Expression::new(
        ExpressionKind::Literal(Literal::Bool(short_circuit_value)),
        span,
        ty.clone(),
    ));
    let short_circuit_predecessor = builder.jump_to(merge_block);

    builder.select_block(rhs_block);
    let rhs = builder.lower_expression_by_id(typed_function, binary.right());
    let rhs_predecessor = builder.jump_to(merge_block);

    builder.select_block(merge_block);
    let incoming =
        [(short_circuit_predecessor, constant), (rhs_predecessor, rhs)].into_iter().collect();
    builder.emit_expression(Expression::new(ExpressionKind::Phi(Phi::new(incoming)), span, ty))
}

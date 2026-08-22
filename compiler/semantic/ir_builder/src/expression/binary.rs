use qbice::storage::intern::Interned;
use rayc_ir::{
    cfg::{Conditional, Terminator},
    expression::{
        Expression, ExpressionID, ExpressionKind,
        binary::{Binary as IrBinary, BinaryOp as IrBinaryOp},
        literal::Literal,
        load::Load,
    },
};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    function::Function as TypedFunction,
    typed_expr::{
        TypedExprID,
        binary::{Binary, BinaryOp},
    },
};

use crate::{builder::Builder, expression::LowerExpression};

impl LowerExpression<Binary> for Builder {
    fn lower_expression(
        &mut self,
        typed_function: &TypedFunction,
        _expression_id: TypedExprID,
        binary: &Binary,
        span: RelativeSpan,
        ty: Interned<Ty>,
    ) -> ExpressionID {
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
    let result = builder.create_temporary(ty.clone(), span);
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
    builder.emit_store(builder.variable_address(result), constant);
    builder.terminate(Terminator::Jump(merge_block));

    builder.select_block(rhs_block);
    let rhs = builder.lower_expression_by_id(typed_function, binary.right());
    builder.emit_store(builder.variable_address(result), rhs);
    builder.terminate(Terminator::Jump(merge_block));

    builder.select_block(merge_block);
    builder.emit_expression(Expression::new(
        ExpressionKind::Load(Load::new(builder.variable_address(result))),
        span,
        ty,
    ))
}

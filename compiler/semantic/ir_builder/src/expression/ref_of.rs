use qbice::storage::intern::Interned;
use rayc_ir::expression::{Expression, ExpressionID, ExpressionKind, ref_of::RefOf as IrRefOf};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    function::Function as TypedFunction,
    typed_expr::{TypedExprID, ref_of::RefOf},
};

use crate::{builder::Builder, expression::LowerExpression};

impl LowerExpression<RefOf> for Builder {
    fn lower_expression(
        &mut self,
        typed_function: &TypedFunction,
        _expression_id: TypedExprID,
        reference: &RefOf,
        span: RelativeSpan,
        ty: Interned<Ty>,
    ) -> ExpressionID {
        let address = self.lower_address_by_id(typed_function, reference.pointee());
        self.emit_expression(Expression::new(
            ExpressionKind::RefOf(IrRefOf::new(address)),
            span,
            ty,
        ))
    }
}

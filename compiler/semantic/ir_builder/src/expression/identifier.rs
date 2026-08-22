use qbice::storage::intern::Interned;
use rayc_ir::expression::{Expression, ExpressionID, ExpressionKind, load::Load};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    function::Function as TypedFunction,
    typed_expr::{TypedExprID, identifier::Identifier},
};

use crate::{builder::Builder, expression::LowerExpression};

impl LowerExpression<Identifier> for Builder {
    fn lower_expression(
        &mut self,
        typed_function: &TypedFunction,
        expression_id: TypedExprID,
        _identifier: &Identifier,
        span: RelativeSpan,
        ty: Interned<Ty>,
    ) -> ExpressionID {
        let address = self.lower_address_by_id(typed_function, expression_id);
        self.emit_expression(Expression::new(ExpressionKind::Load(Load::new(address)), span, ty))
    }
}

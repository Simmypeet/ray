use qbice::storage::intern::Interned;
use rayc_ir::expression::{Expression, ExpressionID, ExpressionKind, load::Load};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;
use rayc_typed_ast::typed_expr::{LvalueClassification, TypedExprID, tuple_index::TupleIndex};

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a TupleIndex>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a TupleIndex>,
    ) -> ExpressionID {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let tuple_index = expression.node();
        let expression_id = expression.id();
        match context.classify_lvalue(expression_id) {
            LvalueClassification::Lvalue(_) => emit_load(self, context, expression_id, span, ty),
            LvalueClassification::NotLvalue => {
                let operand_ty = context.expression_ty(tuple_index.operand()).clone();
                let operand_value = self.lower_expression_by_id(context, tuple_index.operand());
                let temporary = self.create_temporary(operand_ty, span);
                let mut address = self.variable_address(temporary);
                self.emit_store(address.clone(), operand_value);
                self.project_tuple(&mut address, tuple_index.index());
                self.emit_expression(Expression::new(
                    ExpressionKind::Load(Load::new(address)),
                    span,
                    ty,
                ))
            }
            LvalueClassification::Errored => self.emit_expression(Expression::new_error(span, ty)),
        }
    }
}

fn emit_load(
    builder: &mut Builder,
    context: &LoweringContext<'_>,
    expression_id: TypedExprID,
    span: RelativeSpan,
    ty: Interned<Ty>,
) -> ExpressionID {
    let address = builder.lower_address_by_id(context, expression_id);
    builder.emit_expression(Expression::new(ExpressionKind::Load(Load::new(address)), span, ty))
}

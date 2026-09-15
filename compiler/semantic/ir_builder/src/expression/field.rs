use rayc_ir::ir_expr::{IRExpr, IRExprID, IRExprKind, load::Load};
use rayc_typed_ast::typed_expr::{LvalueClassification, field_access::FieldAccess};

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a FieldAccess>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a FieldAccess>,
    ) -> IRExprID {
        match context.classify_lvalue(expression.id()) {
            LvalueClassification::Lvalue(_) => {
                self.lower_address_and_load(context, expression.id())
            }

            LvalueClassification::NotLvalue => {
                // store the operand in a temporary variable and then project the field from
                // that temporary variable
                let mut temporary_address =
                    self.create_temporary_and_lower_store(expression.node().operand(), context);

                self.project_field(&mut temporary_address, expression.node().field());

                self.emit_expression(IRExpr::new(
                    IRExprKind::Load(Load::new(temporary_address)),
                    context.expression_span(expression.id()),
                    context.expression_ty(expression.id()).clone(),
                ))
            }
            LvalueClassification::Errored => self.emit_expression(IRExpr::new_error(
                context.expression_span(expression.id()),
                context.expression_ty(expression.id()).clone(),
            )),
        }
    }
}

use rayc_ir::ir_expr::{IRExpr, IRExprID, IRExprKind, make_lambda::MakeLambda};
use rayc_type::ty::{Ty, application::View as ApplicationView};
use rayc_typed_ast::typed_expr::lambda::Lambda;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a Lambda>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Lambda>,
    ) -> IRExprID {
        let typed_expression = context.expression(expression.id());
        let lambda = expression.node();
        let return_ty = match &**typed_expression.ty() {
            Ty::Application(application) => match application.view() {
                ApplicationView::Lambda(lambda) => lambda.return_type().clone(),
                ApplicationView::Primitive(_)
                | ApplicationView::Tuple(_)
                | ApplicationView::Pointer(_)
                | ApplicationView::Error => {
                    panic!(
                        "TypedAST lambda expression should have a solved lambda type, found {:?}",
                        typed_expression.ty()
                    )
                }
            },
            Ty::Inference(_) | Ty::PolyVar(_) => {
                panic!(
                    "TypedAST lambda expression should have a solved lambda type, found {:?}",
                    typed_expression.ty()
                )
            }
            Ty::EffectRow(_) => {
                todo!("lower a TypedAST lambda expression with an effect-row type")
            }
        };
        let function_id = self.lower_lambda_function(
            context,
            lambda.function_id(),
            return_ty,
            typed_expression.span(),
        );
        let captures = self.lower_capture_operands(context, lambda.function_id());

        self.emit_expression(IRExpr::new(
            IRExprKind::MakeLambda(MakeLambda::new(function_id, captures)),
            typed_expression.span(),
            typed_expression.ty().clone(),
        ))
    }
}

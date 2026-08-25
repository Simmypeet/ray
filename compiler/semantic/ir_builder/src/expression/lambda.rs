use rayc_ir::ir_expr::{
    IRExpr, IRExprID, IRExprKind, make_lambda::MakeLambda, ref_of::RefOf as IrRefOf,
};
use rayc_type::ty::{Ty, TyApplicationView};
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
                TyApplicationView::Lambda(lambda) => lambda.return_type().clone(),
                TyApplicationView::Primitive(_)
                | TyApplicationView::Tuple(_)
                | TyApplicationView::Pointer(_)
                | TyApplicationView::Error => {
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
        };
        let function_id = self.lower_lambda_function(context, lambda.function_id(), return_ty);
        let captures = context
            .capture_plan(lambda.function_id())
            .captures()
            .map(|(_, requirement)| {
                let address = self.source_address(requirement.source());
                let ty =
                    self.pointer_ty(requirement.pointee_ty().clone(), requirement.mutability());
                self.emit_expression(IRExpr::new(
                    IRExprKind::RefOf(IrRefOf::new(address)),
                    requirement.span(),
                    ty,
                ))
            })
            .collect();

        self.emit_expression(IRExpr::new(
            IRExprKind::MakeLambda(MakeLambda::new(function_id, captures)),
            typed_expression.span(),
            typed_expression.ty().clone(),
        ))
    }
}

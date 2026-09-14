use rayc_source_file::SourceElement;
use rayc_syntax::expression::While as WhileSyntax;
use rayc_type::ty::{Primitive, Ty};
use rayc_typed_ast::typed_expr::{TypedExprID, while_loop::While};

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl Bind<WhileSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: WhileSyntax) -> TypedExprID {
        // Bind the condition outside the body's lexical scope.
        let Some(condition) = syn.condition() else {
            return self.push_error_expression(syn.span()).await;
        };
        let condition = Box::pin(self.bind(condition)).await;
        let bool_ty = Ty::new_primitive(Primitive::Bool, self.engine());
        self.push_while_condition_constraint(&bool_ty, condition).await;

        // A while body is a nested statement scope in which break and continue
        // are valid.
        self.enter_statement_block(true);
        if let Some(block) = syn.block() {
            for statement in block.statements() {
                Box::pin(self.bind_statement(&statement)).await;
            }
        }
        let body = self.exit_statement_block(true);

        self.insert_expression(While::new(condition, body), syn.span(), Ty::new_unit(self.engine()))
            .await
    }
}

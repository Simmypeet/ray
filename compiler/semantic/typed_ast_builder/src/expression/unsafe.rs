use rayc_source_file::SourceElement;
use rayc_syntax::expression::{IfElseArm as ArmSyntax, Unsafe as UnsafeSyntax};
use rayc_type::ty::Ty;
use rayc_typed_ast::typed_expr::{TypedExprID, statement_block::StatementBlock};

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl Bind<UnsafeSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: UnsafeSyntax) -> TypedExprID {
        let Some(arm) = syn.arm() else {
            return self.push_error_expression(syn.span()).await;
        };

        // `unsafe` only allows raw pointer dereferences inside it, so it needs
        // no node of its own: the expression form is its inner expression, and
        // the block form is a plain statement block.
        self.enter_unsafe();
        let expression = match arm {
            ArmSyntax::Expression(expression_arm) => {
                if let Some(expression) = expression_arm.expression() {
                    Box::pin(self.bind(expression)).await
                } else {
                    self.push_error_expression(syn.span()).await
                }
            }
            ArmSyntax::Block(block) => {
                self.enter_statement_block(false);
                for statement in block.statements().filter_map(rayc_syntax::Passable::into_option) {
                    Box::pin(self.bind_statement(&statement)).await;
                }
                let statements = self.exit_statement_block(false);
                self.insert_expression(
                    StatementBlock::new(statements),
                    syn.span(),
                    Ty::new_unit(self.engine()),
                )
                .await
            }
        };
        self.exit_unsafe();

        expression
    }
}

use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_source_file::SourceElement;
use rayc_syntax::expression::{
    IfElse as IfElseSyntax, IfElseArm as IfElseArmSyntax, IfElseCondition,
};
use rayc_type::ty::{Primitive, Ty};
use rayc_typed_ast::typed_expr::{
    TypedExprID,
    errored::ErroredChild,
    if_else::{Arm, ConditionalArm, IfElse},
};

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl Bind<IfElseSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: IfElseSyntax) -> TypedExprID {
        // Bind every condition and arm in source order so names introduced by a
        // statement block remain local to that block.
        let mut conditional_arms = Vec::new();
        let mut arm_spans = Vec::new();

        let condition = self.bind_if_condition(syn.condition()).await;
        let then_arm = self.bind_if_arm(syn.then_arm()).await;
        if let (Some(condition), Some((arm, span))) = (condition, then_arm) {
            conditional_arms.push(ConditionalArm::new(condition, arm));
            arm_spans.push(span);
        }

        for elif in syn.elif_arms() {
            let condition = self.bind_if_condition(elif.condition()).await;
            let arm = self.bind_if_arm(elif.arm()).await;
            if let (Some(condition), Some((arm, span))) = (condition, arm) {
                conditional_arms.push(ConditionalArm::new(condition, arm));
                arm_spans.push(span);
            }
        }

        let else_arm = if let Some(else_syntax) = syn.else_arm() {
            self.bind_if_arm(else_syntax.arm()).await
        } else {
            None
        };

        // If somehow, we don't have any conditional arms or the presence of an else arm
        // doesn't match the syntax, then we have a malformed if-else expression. In
        // that case, we want to return an error expression.
        if conditional_arms.is_empty() || syn.else_arm().is_some() != else_arm.is_some() {
            let mut children = Vec::new();
            for conditional_arm in &conditional_arms {
                children.push(conditional_arm.condition().into());
                append_errored_arm_children(conditional_arm.arm(), &mut children);
            }
            if let Some((else_arm, _)) = &else_arm {
                append_errored_arm_children(else_arm, &mut children);
            }

            return self.push_error_expression_with_children(syn.span(), children).await;
        }

        // An omitted else supplies an implicit unit path, making the complete
        // expression unit and requiring every explicit arm to agree.
        let ty = if else_arm.is_some() {
            self.new_type_inference()
        } else {
            Ty::new_unit(self.engine())
        };
        for (conditional_arm, span) in conditional_arms.iter().zip(arm_spans) {
            self.push_if_arm_constraint(&ty, conditional_arm.arm(), span).await;
        }
        if let Some((else_arm, span)) = &else_arm {
            self.push_if_arm_constraint(&ty, else_arm, *span).await;
        }

        self.insert_expression(
            IfElse::new(conditional_arms, else_arm.map(|(arm, _)| arm)),
            syn.span(),
            ty,
        )
        .await
    }
}

fn append_errored_arm_children(arm: &Arm, children: &mut Vec<ErroredChild>) {
    match arm {
        Arm::Expression(expression) => children.push((*expression).into()),
        Arm::Block(statements) => {
            children.extend(statements.iter().copied().map(ErroredChild::from));
        }
    }
}

impl TAstBuilder {
    async fn bind_if_condition(
        &mut self,
        condition: Option<IfElseCondition>,
    ) -> Option<TypedExprID> {
        let expression = condition.and_then(|condition| condition.expression())?;
        let expression = Box::pin(self.bind(expression)).await;

        let bool_ty = Ty::new_primitive(Primitive::Bool, self.engine());
        self.push_if_condition_constraint(&bool_ty, expression).await;

        Some(expression)
    }

    async fn bind_if_arm(&mut self, arm: Option<IfElseArmSyntax>) -> Option<(Arm, RelativeSpan)> {
        let arm = arm?;
        let span = arm.span();
        match arm {
            IfElseArmSyntax::Expression(expression_arm) => {
                let expression = expression_arm.expression()?;
                Some((Arm::Expression(Box::pin(self.bind(expression)).await), span))
            }
            IfElseArmSyntax::Block(block) => {
                self.enter_statement_block(false);
                for statement in block.statements() {
                    Box::pin(self.bind_statement(&statement)).await;
                }
                Some((Arm::Block(self.exit_statement_block(false)), span))
            }
        }
    }

    async fn push_if_arm_constraint(
        &mut self,
        expected_ty: &Interned<Ty>,
        arm: &Arm,
        span: RelativeSpan,
    ) {
        match arm {
            Arm::Expression(expression) => {
                self.push_if_branch_constraint(expected_ty, *expression).await;
            }
            Arm::Block(_) => {
                self.push_if_unit_branch_constraint(expected_ty, span).await;
            }
        }
    }
}

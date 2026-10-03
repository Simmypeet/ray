use rayc_lexical::tree::RelativeSpan;
use rayc_source_file::SourceElement;
use rayc_syntax::expression::{Leaf, Literal, Prefix, PrefixOperator};
use rayc_typed_ast::typed_expr::{
    TypedExprID, TypedExprKind,
    unary::{Unary, UnaryOp},
};

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl Bind<Prefix> for TAstBuilder {
    async fn bind(&mut self, syn: Prefix) -> TypedExprID {
        let Some(postfix) = syn.postfix() else {
            return self.push_error_expression(syn.span()).await;
        };

        // The operators apply from the innermost one outwards, so in `- -x`,
        // the last `-` applies first.
        let operators = syn.operators().collect::<Vec<_>>();

        // A negated numeric literal such as `-128i8` may be one past the
        // largest value of its type, so the literal is told about the
        // negation it is the operand of.
        let negated_literal = if let Some(PrefixOperator::Negate(_)) = operators.last()
            && let Some(Leaf::Literal(Literal::Numeric(literal))) = postfix.leaf()
            && postfix.postfixes().next().is_none()
        {
            Some(literal)
        } else {
            None
        };
        let mut operand = match negated_literal {
            Some(literal) => self.bind_numeric_literal(&literal, postfix.span(), true).await,
            None => self.bind(postfix).await,
        };

        for operator in operators.iter().rev() {
            let span = operator.span().join(&self.span_of_expression(operand));
            operand = match operator {
                PrefixOperator::Negate(_) => self.build_negation(operand, span).await,
            };
        }

        operand
    }
}

impl TAstBuilder {
    /// Negates the operand, which must have a signed numeric type. The
    /// negation has the operand's type.
    async fn build_negation(&mut self, operand: TypedExprID, span: RelativeSpan) -> TypedExprID {
        let ty = self.new_signed_numeric_type_inference();
        self.push_negation_operand_constraint(&ty, operand).await;

        self.insert_expression(TypedExprKind::Unary(Unary::new(UnaryOp::Negate, operand)), span, ty)
            .await
    }
}

use rayc_lexical::tree::RelativeSpan;
use rayc_source_file::SourceElement;
use rayc_syntax::expression::{Leaf, Literal, Prefix, PrefixOperator};
use rayc_typed_ast::typed_expr::{
    TypedExprID, TypedExprKind,
    r#move::Move,
    unary::{Unary, UnaryOp},
};

use crate::{
    bind::Bind,
    tast_builder::{TAstBuilder, constraint_solver::NumericOperation},
};

impl Bind<Prefix> for TAstBuilder {
    async fn bind(&mut self, syn: Prefix) -> TypedExprID {
        let Some(postfix) = syn.postfix() else {
            return self.push_error_expression(syn.span()).await;
        };

        // The operators apply from the innermost one outwards, so in `- -x`,
        // the last `-` applies first.
        let mut operators = syn.operators().collect::<Vec<_>>();

        // A negated numeric literal such as `-128i8` is bound as a single
        // negative literal, since its magnitude may not fit in its type, e.g.
        // `128` in `int8`.
        let mut operand = if let Some(PrefixOperator::Negate(minus)) = operators.last()
            && let Some(Leaf::Literal(Literal::Numeric(literal))) = postfix.leaf()
            && postfix.postfixes().next().is_none()
        {
            let span = minus.span().join(&postfix.span());
            operators.pop();
            self.bind_numeric_literal(&literal, span, true).await
        } else {
            self.bind(postfix).await
        };

        for operator in operators.iter().rev() {
            let span = operator.span().join(&self.span_of_expression(operand));
            operand = match operator {
                PrefixOperator::Negate(_) => self.build_negation(operand, span).await,
                PrefixOperator::Move(_) => self.build_move(operand, span).await,
            };
        }

        operand
    }
}

impl TAstBuilder {
    /// Negates the operand, which must have a signed numeric type. The
    /// negation has the operand's type.
    async fn build_negation(&mut self, operand: TypedExprID, span: RelativeSpan) -> TypedExprID {
        self.push_numeric_operand_constraint(NumericOperation::Negation, operand).await;

        let ty = self.type_of_expression(operand);
        self.insert_expression(TypedExprKind::Unary(Unary::new(UnaryOp::Negate, operand)), span, ty)
            .await
    }

    /// Moves out of the operand. The moved value keeps the operand's type.
    async fn build_move(&mut self, operand: TypedExprID, span: RelativeSpan) -> TypedExprID {
        let ty = self.type_of_expression(operand);
        self.insert_expression(TypedExprKind::Move(Move::new(operand)), span, ty).await
    }
}

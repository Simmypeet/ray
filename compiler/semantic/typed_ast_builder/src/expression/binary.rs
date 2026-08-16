use rayc_source_file::SourceElement;
use rayc_syntax::expression::{Binary as BinarySyntax, BinaryOperator as BinaryOperatorSyntax};
use rayc_type::ty::{Primitive, Ty};
use rayc_typed_ast::typed_expr::{
    TypedExpr, TypedExprID, TypedExprKind,
    binary::{Binary, BinaryOp},
};

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl Bind<BinarySyntax> for TAstBuilder {
    async fn bind(&mut self, syn: BinarySyntax) -> TypedExprID {
        let span = syn.span();
        let Some(first) = syn.postfix() else {
            return self.push_error_expression(span);
        };

        let first = self.bind(first).await;
        let mut bound_operands = vec![first];
        let mut subsequent = Vec::new();
        let mut is_malformed = false;

        for next in syn.subsequent() {
            let operator = next.operator().as_ref().map(map_operator);
            let right = if let Some(postfix) = next.postfix() {
                let right = self.bind(postfix).await;
                bound_operands.push(right);
                Some(right)
            } else {
                None
            };

            if let (Some(operator), Some(right)) = (operator, right) {
                subsequent.push((operator, right));
            } else {
                is_malformed = true;
            }
        }

        if is_malformed {
            return self.push_error_expression_with_children(span, bound_operands);
        }

        self.reduce_with_precedence(first, subsequent)
    }
}

impl TAstBuilder {
    /// Converts a flattened operand/operator sequence into a
    /// precedence-aware binary tree.
    ///
    /// `operands` stores the roots of the typed-AST subtrees built so far,
    /// while `operators` stores the operators that are still waiting for their
    /// right-hand subtree. Before pushing an incoming operator, every pending
    /// operator with higher or equal precedence is reduced into one operand
    /// subtree. Using `>=` is what makes operators at the same precedence
    /// left-associative: `a - b + c` first becomes `(a - b)`, which is then
    /// used as the left operand of `+`.
    ///
    /// After all flat pairs have been visited, draining the operator stack
    /// joins the remaining subtrees into one typed-AST expression.
    fn reduce_with_precedence(
        &mut self,
        first: TypedExprID,
        subsequent: impl IntoIterator<Item = (BinaryOp, TypedExprID)>,
    ) -> TypedExprID {
        let mut operands = vec![first];
        let mut operators = Vec::new();

        for (operator, operand) in subsequent {
            while operators.last().is_some_and(|last| should_reduce(*last, operator)) {
                self.reduce_last(&mut operands, &mut operators);
            }

            operators.push(operator);
            operands.push(operand);
        }

        // No more incoming operators can affect precedence, so reduce from the
        // top of the operator stack until only the complete tree root remains.
        while !operators.is_empty() {
            self.reduce_last(&mut operands, &mut operators);
        }

        operands.pop().expect("a binary expression should contain its first operand")
    }

    /// Replaces the top operator and its two operand subtrees with their
    /// typed-AST parent.
    fn reduce_last(&mut self, operands: &mut Vec<TypedExprID>, operators: &mut Vec<BinaryOp>) {
        let right = operands.pop().expect("an operator should have a right operand");
        let left = operands.pop().expect("an operator should have a left operand");
        let operator = operators.pop().expect("an operand reduction should have an operator");

        operands.push(self.build_binary(left, operator, right));
    }

    fn build_binary(
        &mut self,
        left: TypedExprID,
        operator: BinaryOp,
        right: TypedExprID,
    ) -> TypedExprID {
        let ty = match operator {
            BinaryOp::Plus | BinaryOp::Minus | BinaryOp::Multiply | BinaryOp::Divide => {
                self.new_numeric_type_inference()
            }
            BinaryOp::And | BinaryOp::Or => Ty::new_primitive(Primitive::Bool, self.engine()),
        };

        self.push_binary_operator_constraint(&ty, left);
        self.push_binary_operator_constraint(&ty, right);

        let span = self.span_of_expression(left).join(&self.span_of_expression(right));

        self.insert_expression(TypedExpr::new(
            TypedExprKind::Binary(Binary::new(left, operator, right)),
            span,
            ty,
        ))
    }
}

const fn map_operator(operator: &BinaryOperatorSyntax) -> BinaryOp {
    match operator {
        BinaryOperatorSyntax::Plus(_) => BinaryOp::Plus,
        BinaryOperatorSyntax::Minus(_) => BinaryOp::Minus,
        BinaryOperatorSyntax::Multiply(_) => BinaryOp::Multiply,
        BinaryOperatorSyntax::Divide(_) => BinaryOp::Divide,
        BinaryOperatorSyntax::And(_) => BinaryOp::And,
        BinaryOperatorSyntax::Or(_) => BinaryOp::Or,
    }
}

const fn precedence(operator: BinaryOp) -> u8 {
    match operator {
        BinaryOp::Or => 0,
        BinaryOp::And => 1,
        BinaryOp::Plus | BinaryOp::Minus => 2,
        BinaryOp::Multiply | BinaryOp::Divide => 3,
    }
}

const fn should_reduce(pending: BinaryOp, incoming: BinaryOp) -> bool {
    precedence(pending) >= precedence(incoming)
}

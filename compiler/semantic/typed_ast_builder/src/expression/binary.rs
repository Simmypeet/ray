use rayc_source_file::SourceElement;
use rayc_syntax::expression::{Binary as BinarySyntax, BinaryOperator as BinaryOperatorSyntax};
use rayc_type::ty::{Primitive, Ty};
use rayc_typed_ast::typed_expr::{
    TypedExprID, TypedExprKind,
    binary::{Associativity, Binary, BinaryOp},
};

use crate::{bind::Bind, diagnostic::LvalueOperation, tast_builder::TAstBuilder};

impl Bind<BinarySyntax> for TAstBuilder {
    async fn bind(&mut self, syn: BinarySyntax) -> TypedExprID {
        let span = syn.span();
        let Some(first) = syn.cast() else {
            return self.push_error_expression(span).await;
        };

        let first = self.bind(first).await;
        let mut bound_operands = vec![first];
        let mut subsequent = Vec::new();
        let mut is_malformed = false;

        for next in syn.subsequent() {
            let operator = next.operator().as_ref().map(map_operator);
            let right = if let Some(cast) = next.cast() {
                let right = self.bind(cast).await;
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
            return self.push_error_expression_with_expression_children(span, bound_operands).await;
        }

        self.reduce_with_precedence(first, subsequent).await
    }
}

impl TAstBuilder {
    /// Converts a flattened operand/operator sequence into a
    /// precedence-aware binary tree.
    ///
    /// `operands` stores the roots of the typed-AST subtrees built so far,
    /// while `operators` stores the operators that are still waiting for their
    /// right-hand subtree. Before pushing an incoming operator, every pending
    /// operator with higher precedence is reduced into one operand subtree.
    /// Equal precedence is reduced only when the incoming operator is
    /// left-associative: `a - b + c` becomes `(a - b) + c`, while assignment
    /// remains queued so `a = b = c` becomes `a = (b = c)`.
    ///
    /// After all flat pairs have been visited, draining the operator stack
    /// joins the remaining subtrees into one typed-AST expression.
    async fn reduce_with_precedence(
        &mut self,
        first: TypedExprID,
        subsequent: impl IntoIterator<Item = (BinaryOp, TypedExprID)>,
    ) -> TypedExprID {
        let mut operands = vec![first];
        let mut operators = Vec::new();

        for (operator, operand) in subsequent {
            while operators.last().is_some_and(|last| should_reduce(*last, operator)) {
                self.reduce_last(&mut operands, &mut operators).await;
            }

            operators.push(operator);
            operands.push(operand);
        }

        // No more incoming operators can affect precedence, so reduce from the
        // top of the operator stack until only the complete tree root remains.
        while !operators.is_empty() {
            self.reduce_last(&mut operands, &mut operators).await;
        }

        operands.pop().expect("a binary expression should contain its first operand")
    }

    /// Replaces the top operator and its two operand subtrees with their
    /// typed-AST parent.
    async fn reduce_last(
        &mut self,
        operands: &mut Vec<TypedExprID>,
        operators: &mut Vec<BinaryOp>,
    ) {
        let right = operands.pop().expect("an operator should have a right operand");
        let left = operands.pop().expect("an operator should have a left operand");
        let operator = operators.pop().expect("an operand reduction should have an operator");

        operands.push(self.build_binary(left, operator, right).await);
    }

    async fn build_binary(
        &mut self,
        left: TypedExprID,
        operator: BinaryOp,
        mut right: TypedExprID,
    ) -> TypedExprID {
        let ty = match operator {
            // The assigned value moves into the place, so the assignment
            // itself has no value to give.
            BinaryOp::Assign => {
                let ty = self.type_of_expression(left);
                right = self.coerce(right, &ty).await;
                self.push_variable_assignment_constraint(&ty, right).await;
                self.require_lvalue(left, true, LvalueOperation::Assignment);
                Ty::new_unit(self.engine())
            }
            BinaryOp::Equal | BinaryOp::NotEqual => {
                let operand_ty = self.new_equality_comparable_type_inference();
                self.push_binary_operator_constraint(&operand_ty, left).await;
                self.push_binary_operator_constraint(&operand_ty, right).await;
                Ty::new_primitive(Primitive::Bool, self.engine())
            }
            BinaryOp::Plus | BinaryOp::Minus | BinaryOp::Multiply | BinaryOp::Divide => {
                let ty = self.new_numeric_type_inference();
                self.push_binary_operator_constraint(&ty, left).await;
                self.push_binary_operator_constraint(&ty, right).await;
                ty
            }
            BinaryOp::And | BinaryOp::Or => {
                let ty = Ty::new_primitive(Primitive::Bool, self.engine());
                self.push_binary_operator_constraint(&ty, left).await;
                self.push_binary_operator_constraint(&ty, right).await;
                ty
            }
        };

        let span = self.span_of_expression(left).join(&self.span_of_expression(right));

        self.insert_expression(TypedExprKind::Binary(Binary::new(left, operator, right)), span, ty)
            .await
    }
}

const fn map_operator(operator: &BinaryOperatorSyntax) -> BinaryOp {
    match operator {
        BinaryOperatorSyntax::Equal(_) => BinaryOp::Equal,
        BinaryOperatorSyntax::NotEqual(_) => BinaryOp::NotEqual,
        BinaryOperatorSyntax::Assign(_) => BinaryOp::Assign,
        BinaryOperatorSyntax::Plus(_) => BinaryOp::Plus,
        BinaryOperatorSyntax::Minus(_) => BinaryOp::Minus,
        BinaryOperatorSyntax::Multiply(_) => BinaryOp::Multiply,
        BinaryOperatorSyntax::Divide(_) => BinaryOp::Divide,
        BinaryOperatorSyntax::And(_) => BinaryOp::And,
        BinaryOperatorSyntax::Or(_) => BinaryOp::Or,
    }
}

const fn should_reduce(pending: BinaryOp, incoming: BinaryOp) -> bool {
    pending.precedence() > incoming.precedence()
        || (pending.precedence() == incoming.precedence()
            && matches!(incoming.associativity(), Associativity::Left))
}

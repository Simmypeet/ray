use rayc_source_file::SourceElement;
use rayc_syntax::expression::{
    Deref as DerefSyntax, Leaf, Postfix, PostfixOperator, RefOf as RefOfSyntax,
    TupleIndex as TupleIndexSyntax,
};
use rayc_type::ty::{Mutability, Ty, application::View as ApplicationView};
use rayc_typed_ast::typed_expr::{
    TypedExprID, TypedExprKind, deref::Deref, ref_of::RefOf, tuple_index::TupleIndex,
};

use crate::{
    bind::Bind,
    diagnostic::{
        Diagnostic, ExpectedPointerType, ExpectedTupleType, LvalueOperation, OutOfBoundsTupleIndex,
        TypeMustBeKnownAtThisPoint,
    },
    tast_builder::TAstBuilder,
};

impl Bind<Postfix> for TAstBuilder {
    async fn bind(&mut self, syn: Postfix) -> TypedExprID {
        let Some(leaf) = syn.leaf() else {
            // very malformed expressionc
            return self.push_error_expression(syn.span());
        };

        let postfixes = syn.postfixes().collect::<Vec<_>>();
        let (mut bound, postfix_start) = match (&leaf, postfixes.first()) {
            (Leaf::Identifier(identifier), Some(PostfixOperator::Call(call))) => {
                (self.build_bare_identifier_call(identifier.clone(), call).await, 1)
            }
            (
                Leaf::Identifier(_),
                Some(
                    PostfixOperator::RefOf(_)
                    | PostfixOperator::Deref(_)
                    | PostfixOperator::TupleIndex(_),
                )
                | None,
            )
            | (Leaf::Literal(_) | Leaf::Parenthesized(_), _) => (self.bind(leaf).await, 0),
        };

        for postfix in postfixes.into_iter().skip(postfix_start) {
            let val = match postfix {
                PostfixOperator::Call(call) => Some(self.build_lambda_call(bound, &call).await),
                PostfixOperator::RefOf(ref_of) => Some(self.build_ref_of(bound, &ref_of)),

                PostfixOperator::Deref(deref) => Some(self.build_deref(bound, &deref)),

                PostfixOperator::TupleIndex(index) => self.build_tuple_index(bound, &index),
            };

            bound = val.unwrap_or_else(|| {
                self.push_error_expression_with_children(syn.span(), vec![bound])
            });
        }

        bound
    }
}

impl TAstBuilder {
    fn build_ref_of(&mut self, bound: TypedExprID, ref_of: &RefOfSyntax) -> TypedExprID {
        let span = self.span_of_expression(bound);
        let ty = self.type_of_expression(bound);
        let mutability = if ref_of.mut_keyword().is_some() {
            Mutability::Mutable
        } else {
            Mutability::Immutable
        };

        let pointer_ty = Ty::new_pointer(ty, mutability, self.engine());

        self.require_lvalue(
            bound,
            mutability == Mutability::Mutable,
            if mutability == Mutability::Mutable {
                LvalueOperation::MutableReference
            } else {
                LvalueOperation::Reference
            },
        );

        self.insert_expression(
            TypedExprKind::RefOf(RefOf::new(bound, mutability)),
            span.join(&ref_of.span()),
            pointer_ty,
        )
    }

    fn build_tuple_index(
        &mut self,
        bound: TypedExprID,
        tuple_index: &TupleIndexSyntax,
    ) -> Option<TypedExprID> {
        let index = tuple_index.numeric()?;
        let span = self.span_of_expression(bound);

        let ty = self.latest_type(&self.type_of_expression(bound));

        // expect a tuple type
        let tuple = match &*ty {
            Ty::Application(ty_application) => {
                if let ApplicationView::Tuple(tuple) = ty_application.view() {
                    tuple
                } else {
                    self.push_diagnostic(Diagnostic::ExpectedTupleType(
                        ExpectedTupleType::builder().span(span).ty(ty.clone()).build(),
                    ));
                    return None;
                }
            }

            Ty::PolyVar(_) => {
                self.push_diagnostic(Diagnostic::ExpectedTupleType(
                    ExpectedTupleType::builder().span(span).ty(ty.clone()).build(),
                ));
                return None;
            }

            // the type must be known at this point
            //
            // NOTE: it would be nice to have a constraint system that doesn't
            // require the type to be known at this point, but that would
            // require a more sophisticated tuple type representation
            Ty::Inference(_) => {
                self.push_diagnostic(Diagnostic::TypeMustBeKnownAtThisPoint(
                    TypeMustBeKnownAtThisPoint::builder().span(span).build(),
                ));

                return None;
            }
            Ty::EffectRow(_) => todo!("type-check tuple indexing on an effect-row type"),
        };

        let index = index.kind.parse::<usize>().expect("TODO: handle over flow error");

        if index >= tuple.args().len() {
            self.push_diagnostic(Diagnostic::OutOfBoundsTupleIndex(
                OutOfBoundsTupleIndex::builder()
                    .span(span)
                    .index(index)
                    .tuple_len(tuple.args().len())
                    .build(),
            ));
            return None;
        }

        Some(self.insert_expression(
            TypedExprKind::TupleIndex(TupleIndex::new(bound, index)),
            span,
            tuple.args()[index].clone(),
        ))
    }

    fn build_deref(&mut self, expr_id: TypedExprID, deref: &DerefSyntax) -> TypedExprID {
        let span = self.span_of_expression(expr_id);
        let ty = self.latest_type(&self.type_of_expression(expr_id));
        let pointee = match &*ty {
            Ty::Application(application) => match application.view() {
                ApplicationView::Pointer(pointer) => pointer.pointee().clone(),
                ApplicationView::Error => Ty::new_star_error(self.engine()),
                ApplicationView::Primitive(_)
                | ApplicationView::Tuple(_)
                | ApplicationView::Lambda(_) => {
                    self.push_diagnostic(Diagnostic::ExpectedPointerType(
                        ExpectedPointerType::builder().ty(ty).span(span).build(),
                    ));
                    Ty::new_star_error(self.engine())
                }
            },
            Ty::PolyVar(_) => {
                self.push_diagnostic(Diagnostic::ExpectedPointerType(
                    ExpectedPointerType::builder().ty(ty).span(span).build(),
                ));
                Ty::new_star_error(self.engine())
            }
            Ty::Inference(_) => {
                self.push_diagnostic(Diagnostic::TypeMustBeKnownAtThisPoint(
                    TypeMustBeKnownAtThisPoint::builder().span(span).build(),
                ));
                Ty::new_star_error(self.engine())
            }
            Ty::EffectRow(_) => todo!("type-check dereferencing an effect-row type"),
        };

        self.insert_expression(
            TypedExprKind::Deref(Deref::new(expr_id)),
            span.join(&deref.span()),
            pointee,
        )
    }
}

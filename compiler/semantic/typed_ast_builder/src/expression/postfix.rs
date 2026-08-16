use rayc_source_file::SourceElement;
use rayc_syntax::expression::{
    Deref as DerefSyntax, Postfix, PostfixOperator, RefOf as RefOfSyntax,
    TupleIndex as TupleIndexSyntax,
};
use rayc_type::ty::{Ty, TyApplicationView};
use rayc_typed_ast::typed_expr::{
    TypedExpr, TypedExprID, TypedExprKind, deref::Deref, ref_of::RefOf, tuple_index::TupleIndex,
};

use crate::{
    bind::Bind,
    diagnostic::{
        Diagnostic, ExpectedTupleType, OutOfBoundsTupleIndex, TypeMustBeKnownAtThisPoint,
    },
    tast_builder::TAstBuilder,
};

impl Bind<Postfix> for TAstBuilder {
    async fn bind(&mut self, syn: Postfix) -> TypedExprID {
        let Some(leaf) = syn.leaf() else {
            // very malformed expressionc
            return self.push_error_expression(syn.span());
        };

        let mut bound = self.bind(leaf).await;

        for postfix in syn.postfixes() {
            let val = match postfix {
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

        let pointer_ty = Ty::new_pointer(ty, self.engine());

        self.insert_expression(TypedExpr::new(
            TypedExprKind::RefOf(RefOf::new(bound)),
            span.join(&ref_of.span()),
            pointer_ty,
        ))
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
                if let TyApplicationView::Tuple(tuple) = ty_application.view() {
                    tuple
                } else {
                    self.push_diagnostic(Diagnostic::ExpectedTupleType(
                        ExpectedTupleType::builder().span(span).ty(ty.clone()).build(),
                    ));
                    return None;
                }
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

        let new_expr = TypedExpr::new(
            TypedExprKind::TupleIndex(TupleIndex::new(bound, index)),
            span,
            tuple.args()[index].clone(),
        );

        Some(self.insert_expression(new_expr))
    }

    fn build_deref(&mut self, expr_id: TypedExprID, deref: &DerefSyntax) -> TypedExprID {
        let span = self.span_of_expression(expr_id);
        let pointee = self.new_type_inference();

        let expected_pointer = Ty::new_pointer(pointee.clone(), self.engine());

        self.push_deref_constarint(&expected_pointer, expr_id);

        self.insert_expression(TypedExpr::new(
            TypedExprKind::Deref(Deref::new(expr_id)),
            span.join(&deref.span()),
            pointee,
        ))
    }
}

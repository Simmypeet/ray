use rayc_semantic_element::struct_body::get_struct_body;
use rayc_source_file::SourceElement;
use rayc_syntax::expression::{
    Deref as DerefSyntax, FieldAccess as FieldAccessSyntax, Postfix, PostfixOperator,
    RefOf as RefOfSyntax, TupleIndex as TupleIndexSyntax,
};
use rayc_type::{
    subst::Substitutable,
    ty::{Mutability, Ty, application::View as ApplicationView, lifetime::Lifetime},
};
use rayc_typed_ast::typed_expr::{
    TypedExprID, TypedExprKind,
    deref::{Deref, DerefKind},
    field_access::FieldAccess,
    ref_of::RefOf,
    tuple_index::TupleIndex,
};

use crate::{
    bind::Bind,
    diagnostic::{
        Diagnostic, ExpectedPointerType, ExpectedStructType, ExpectedTupleType, LvalueOperation,
        OutOfBoundsTupleIndex, RawPointerDerefOutsideUnsafe, TypeMustBeKnownAtThisPoint,
        UnknownStructField,
    },
    tast_builder::TAstBuilder,
};

impl Bind<Postfix> for TAstBuilder {
    async fn bind(&mut self, syn: Postfix) -> TypedExprID {
        let Some(leaf) = syn.leaf() else {
            // very malformed expressionc
            return self.push_error_expression(syn.span()).await;
        };

        let mut bound = self.bind(leaf).await;
        for postfix in syn.postfixes() {
            let val = match postfix {
                PostfixOperator::Call(call) => Some(self.build_lambda_call(bound, &call).await),
                PostfixOperator::RefOf(ref_of) => Some(self.build_ref_of(bound, &ref_of).await),

                PostfixOperator::Deref(deref) => Some(self.build_deref(bound, &deref).await),

                PostfixOperator::TupleIndex(index) => self.build_tuple_index(bound, &index).await,
                PostfixOperator::FieldAccess(access) => {
                    self.build_field_access(bound, &access).await
                }
            };

            bound = if let Some(val) = val {
                val
            } else {
                // if the postfix operator failed to bind then we return an error expression
                // but we still want to keep the children of the expression so that we can
                // report errors on them as well
                self.push_error_expression_with_children(syn.span(), vec![bound.into()]).await
            };
        }

        bound
    }
}

impl TAstBuilder {
    async fn build_field_access(
        &mut self,
        bound: TypedExprID,
        field_access: &FieldAccessSyntax,
    ) -> Option<TypedExprID> {
        let name = field_access.name()?;
        let operand_span = self.span_of_expression(bound);
        let ty = self.latest_type(&self.type_of_expression(bound)).await;

        // Field lookup requires the concrete struct so its declaration and
        // generic arguments are both available.
        let Some(st) = ty.as_struct_view() else {
            if matches!(&*ty, Ty::Inference(_)) {
                self.push_diagnostic(Diagnostic::TypeMustBeKnownAtThisPoint(
                    TypeMustBeKnownAtThisPoint::builder().span(operand_span).build(),
                ));
            } else {
                self.push_diagnostic(Diagnostic::ExpectedStructType(
                    ExpectedStructType::builder().span(operand_span).ty(ty).build(),
                ));
            }
            return None;
        };

        // Resolve the source name to the stable field identity stored in the
        // typed AST.
        let body = self.engine().get_struct_body(st.symbol_id()).await;
        let Some((field_id, field)) = body.get_by_name(&name.kind) else {
            self.push_diagnostic(Diagnostic::UnknownStructField(
                UnknownStructField::builder()
                    .struct_id(st.symbol_id())
                    .name(name.kind.0)
                    .span(name.span)
                    .build(),
            ));
            return None;
        };

        let subst = st.create_subst(self.engine()).await;
        let field_ty = field.ty().apply_subst_or_clone(&subst, self.engine());

        Some(
            self.insert_expression(
                TypedExprKind::FieldAccess(FieldAccess::new(bound, field_id)),
                operand_span.join(&field_access.span()),
                field_ty,
            )
            .await,
        )
    }

    async fn build_ref_of(&mut self, bound: TypedExprID, ref_of: &RefOfSyntax) -> TypedExprID {
        let span = self.span_of_expression(bound);
        let ty = self.type_of_expression(bound);
        let mutability = if ref_of.mut_keyword().is_some() {
            Mutability::Mutable
        } else {
            Mutability::Immutable
        };

        // Type inference ignores lifetimes, so the borrow's lifetime is left
        // for the borrow checker.
        let reference_ty = Ty::new_reference(
            Ty::new_lifetime(Lifetime::Erased, self.engine()),
            ty,
            mutability,
            self.engine(),
        );

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
            reference_ty,
        )
        .await
    }

    async fn build_tuple_index(
        &mut self,
        bound: TypedExprID,
        tuple_index: &TupleIndexSyntax,
    ) -> Option<TypedExprID> {
        let index = tuple_index.numeric()?;
        let span = self.span_of_expression(bound);

        let ty = self.latest_type(&self.type_of_expression(bound)).await;

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

            Ty::PolyVar(_) | Ty::SelfInstance(_) => {
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
            Ty::Lifetime(_) => unreachable!("an expression cannot have a lifetime as its type"),
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

        Some(
            self.insert_expression(
                TypedExprKind::TupleIndex(TupleIndex::new(bound, index)),
                span,
                tuple.args()[index].clone(),
            )
            .await,
        )
    }

    async fn build_deref(&mut self, expr_id: TypedExprID, deref: &DerefSyntax) -> TypedExprID {
        let span = self.span_of_expression(expr_id);
        let ty = self.latest_type(&self.type_of_expression(expr_id)).await;

        #[allow(clippy::option_if_let_else)]
        let (pointee, kind) = if let Some(target) = ty.as_dereferenceable() {
            if target.is_raw() {
                // Only a raw pointer may be null or dangling, so only its
                // dereference needs `unsafe`.
                if !self.is_inside_unsafe() {
                    self.push_diagnostic(Diagnostic::RawPointerDerefOutsideUnsafe(
                        RawPointerDerefOutsideUnsafe::builder()
                            .span(span.join(&deref.span()))
                            .build(),
                    ));
                }
                (target.pointee().clone(), DerefKind::RawPointer)
            } else {
                (target.pointee().clone(), DerefKind::Reference)
            }
        } else {
            if let Ty::Inference(_) = &*ty {
                self.push_diagnostic(Diagnostic::TypeMustBeKnownAtThisPoint(
                    TypeMustBeKnownAtThisPoint::builder().span(span).build(),
                ));
            } else {
                self.push_diagnostic(Diagnostic::ExpectedPointerType(
                    ExpectedPointerType::builder().ty(ty).span(span).build(),
                ));
            }

            (Ty::new_star_error(self.engine()), DerefKind::RawPointer)
        };

        self.insert_expression(
            TypedExprKind::Deref(Deref::new(expr_id, kind)),
            span.join(&deref.span()),
            pointee,
        )
        .await
    }
}

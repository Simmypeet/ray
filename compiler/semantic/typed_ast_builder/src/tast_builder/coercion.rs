//! Reference coercions at coercion sites.
//!
//! As in Rust, a value of reference type is coerced when the type it is
//! expected to have is known to be a different kind of pointer:
//!
//! - `&mut t` to `&t`, as the reborrow `r.*.&`;
//! - `&mut t` to `&mut t`, as the implicit reborrow `r.*.&mut`, so that passing
//!   a unique reference along does not move it;
//! - `&t` to `*t`, and `&mut t` to `*t` or `*mut t`, as a [`RefToPointer`] of
//!   the reference, which is first reborrowed when it is unique so that the
//!   coercion does not move it.
//!
//! Coercion only looks at the outermost type constructors, and only at what
//! inference knows when the coercion site is bound. Everything else is left to
//! the type relation.

use qbice::storage::intern::Interned;
use rayc_type::ty::{Mutability, Ty, lifetime::Lifetime};
use rayc_typed_ast::typed_expr::{
    TypedExprID, TypedExprKind,
    deref::{Deref, DerefKind},
    ref_of::RefOf,
    ref_to_pointer::RefToPointer,
};

use crate::tast_builder::TAstBuilder;

/// The coercion applied to an expression of reference type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Coercion {
    /// Reborrows the referenced place as a reference of this mutability.
    Reborrow(Mutability),

    /// Turns the reference into a raw pointer of this mutability.
    RawPointer(Mutability),
}

impl TAstBuilder {
    /// Coerces `expression` to `expected` when a reference coercion applies,
    /// and returns the expression to use in its place.
    pub async fn coerce(
        &mut self,
        expression: TypedExprID,
        expected: &Interned<Ty>,
    ) -> TypedExprID {
        let actual = self.latest_type(&self.type_of_expression(expression)).await;
        let expected = self.latest_type(expected).await;

        // Only a reference coerces, and only to another pointer type.
        let (Some(actual), Some(expected)) =
            (actual.as_reference_view(), expected.as_dereferenceable())
        else {
            return expression;
        };

        // A coercion never turns a shared reference into a mutable pointer;
        // the type relation reports that mismatch.
        let coercion = match (actual.mutability(), expected.mutability(), expected.is_raw()) {
            // immutable to mutable is not allowed!
            (Mutability::Immutable, Mutability::Mutable, _)
            // nothing to do if everything is the same
            | (Mutability::Immutable, Mutability::Immutable, false) => return expression,
            // either reborrow to immutable or mutable
            (Mutability::Mutable, mutability, false) => Coercion::Reborrow(mutability),
            // coerce to a row pointer, either from a shared or unique reference
            (_, mutability, true) => Coercion::RawPointer(mutability),
        };

        match coercion {
            Coercion::Reborrow(mutability) => self.reborrow(expression, mutability).await,
            Coercion::RawPointer(mutability) => {
                // A unique reference is not `Copy`, so it is reborrowed to
                // keep the coercion from moving it.
                let reference = match actual.mutability() {
                    Mutability::Immutable => expression,
                    Mutability::Mutable => self.reborrow(expression, mutability).await,
                };

                let span = self.span_of_expression(expression);
                let ty = Ty::new_pointer(actual.pointee().clone(), mutability, self.engine());
                self.insert_expression(
                    TypedExprKind::RefToPointer(RefToPointer::new(reference)),
                    span,
                    ty,
                )
                .await
            }
        }
    }

    /// Borrows the place referenced by `reference` again, as the reference
    /// `reference.*.&` of the given mutability.
    ///
    /// # Panics
    ///
    /// Panics if `reference` is not of reference type.
    async fn reborrow(&mut self, reference: TypedExprID, mutability: Mutability) -> TypedExprID {
        let span = self.span_of_expression(reference);
        let ty = self.latest_type(&self.type_of_expression(reference)).await;
        let pointee =
            ty.as_reference_view().expect("only a reference can be reborrowed").pointee().clone();

        let place = self
            .insert_expression(
                TypedExprKind::Deref(Deref::new(reference, DerefKind::Reference)),
                span,
                pointee.clone(),
            )
            .await;
        let ty = Ty::new_reference(
            Ty::new_lifetime(Lifetime::Erased, self.engine()),
            pointee,
            mutability,
            self.engine(),
        );
        self.insert_expression(TypedExprKind::RefOf(RefOf::new(place, mutability)), span, ty).await
    }
}

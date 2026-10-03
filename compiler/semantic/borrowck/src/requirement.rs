//! What a nested function requires of the function creating it.
//!
//! The lifetimes in the interface of a nested function, a closure body, a
//! handled body or an operation handler, are external regions: universal
//! regions of the nested function, which its outlives environment relates to
//! nothing. The function creating it chooses them: each stands for a region of
//! the creator, where the creator creates the nested function.
//!
//! So a predicate about an external region that the body requires, and may not
//! assume, is not an error of the nested function. It is a requirement of the
//! nested function, as a where clause is one of a declared function:
//!
//! - `'a: 'b`, between two universal regions of which at least one is external,
//!   found by the [check of the universal regions](crate::universal_regions);
//! - `subject: 'a`, of a type parameter or a rigid projection, when `'a` is
//!   external or `subject` mentions an external region, found by the [check of
//!   the type tests](crate::type_test).
//!
//! The creator instantiates the external regions with its own regions, and
//! requires each predicate at the point where it creates the nested function;
//! see [`constraint`](crate::constraint). What it then requires of its own
//! universal regions is checked as any other of its constraints is, so a
//! requirement passes up one level of nesting at a time, until a function may
//! assume it or the definition function reports it.
//!
//! This is rustc's `ClosureRegionRequirements`.

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::ir_function::FunctionID;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    subst::{Subst, Substitutable},
    ty::Ty,
    where_clause::OutlivesPredicate,
};

/// A predicate `subject: 'bound` that a nested function requires of the
/// function creating it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalRequirement {
    /// The predicate, stated over the universal regions of the nested
    /// function: its external regions, the lifetime parameters of the
    /// definition and `'static`.
    predicate: OutlivesPredicate,

    /// The source that requires the predicate, in the nested function or in
    /// a function nested in it.
    span: RelativeSpan,
}

impl ExternalRequirement {
    /// Returns the required predicate stated over the regions of the creator
    /// of the nested function: with each external region replaced with the
    /// region that `instantiation` gives it.
    ///
    /// # Panics
    ///
    /// Panics if `instantiation` leaves an external region in place. The
    /// creator would take it for an external region of its own, and what is
    /// required of it would be lost.
    pub(crate) fn instantiate(
        &self,
        instantiation: &Subst,
        engine: &TrackedEngine,
    ) -> OutlivesPredicate {
        let predicate = self.predicate.apply_subst_or_clone(instantiation, engine);

        let is_instantiated = [predicate.lesser(), predicate.greater()]
            .into_iter()
            .all(|ty| !ty.recursive_iter().any(Ty::is_external_lifetime));
        assert!(
            is_instantiated,
            "every external region of a nested function should be instantiated"
        );

        predicate
    }

    /// Returns the source that requires the predicate, in the nested function
    /// or in a function nested in it.
    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }
}

/// The predicates that one nested function requires of the function creating
/// it, in the order they were found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExternalRequirements {
    requirements: Vec<ExternalRequirement>,
}

impl ExternalRequirements {
    /// Requires `subject: 'bound` of the creator, for the source at `span`.
    pub(crate) fn require(
        &mut self,
        subject: Interned<Ty>,
        bound: Interned<Ty>,
        span: RelativeSpan,
    ) {
        self.requirements
            .push(ExternalRequirement { predicate: OutlivesPredicate::new(subject, bound), span });
    }

    /// Iterates over the requirements, in the order they were found.
    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &ExternalRequirement> {
        self.requirements.iter()
    }

    /// Returns whether the nested function requires nothing of its creator.
    #[must_use]
    pub const fn is_empty(&self) -> bool { self.requirements.is_empty() }
}

/// The requirements of the nested functions of a definition that were checked
/// so far.
///
/// A function is checked after the nested functions it creates, so that their
/// requirements are known where it creates them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NestedRequirements {
    by_function: FxHashMap<FunctionID, ExternalRequirements>,
}

impl NestedRequirements {
    /// Records what the checked function `function_id` requires of its
    /// creator.
    pub(crate) fn record(&mut self, function_id: FunctionID, requirements: ExternalRequirements) {
        self.by_function.insert(function_id, requirements);
    }

    /// Iterates over what the nested function `function_id` requires of its
    /// creator, in the order it was found.
    ///
    /// # Panics
    ///
    /// Panics if `function_id` was not checked yet.
    pub(crate) fn of(
        &self,
        function_id: FunctionID,
    ) -> impl ExactSizeIterator<Item = &ExternalRequirement> {
        self.by_function
            .get(&function_id)
            .expect("a nested function should be checked before the function creating it")
            .iter()
    }
}

//! What a nested function requires of the function creating it: the predicates
//! about its external regions that its body requires but may not assume (rustc:
//! `ClosureRegionRequirements`).

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
    /// The predicate, stated over the universal regions of the nested function.
    predicate: OutlivesPredicate,

    /// The source that requires the predicate, in the nested function or in
    /// a function nested in it.
    span: RelativeSpan,
}

impl ExternalRequirement {
    /// Returns the predicate with each external region replaced by the region
    /// of the creator that `instantiation` gives it.
    pub(crate) fn instantiate(
        &self,
        instantiation: &Subst,
        engine: &TrackedEngine,
    ) -> OutlivesPredicate {
        let predicate = self.predicate.apply_subst_or_clone(instantiation, engine);

        // The creator would take a leftover external region for one of its
        // own, and lose what is required of it.
        let is_instantiated = [predicate.lesser(), predicate.greater()]
            .into_iter()
            .all(|ty| !ty.recursive_iter().any(Ty::is_external_lifetime));
        assert!(
            is_instantiated,
            "every external region of a nested function should be instantiated"
        );

        predicate
    }

    /// Returns the source that requires the predicate.
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

/// The requirements of the nested functions of a definition checked so far.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NestedRequirements {
    by_function: FxHashMap<FunctionID, ExternalRequirements>,
}

impl NestedRequirements {
    /// Records what the checked function `function_id` requires of its creator.
    pub(crate) fn record(&mut self, function_id: FunctionID, requirements: ExternalRequirements) {
        self.by_function.insert(function_id, requirements);
    }

    /// Iterates over what the nested function `function_id`, which must be
    /// checked already, requires of its creator.
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

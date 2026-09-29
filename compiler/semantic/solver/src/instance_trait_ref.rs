//! Entailment for explicit given arguments.

use rayc_type::{
    constraint::instance_trait_ref::InstanceTraitRef, trait_ref::InstanceTraitRefError,
    variance::Variance,
};

use crate::{
    Solver,
    ty_relate::{DerivedConstraint, Error, Step},
};

impl Solver {
    /// Checks trait identity and relates the instantiated trait arguments.
    pub async fn entail_instance_trait_ref(
        &mut self,
        check: &InstanceTraitRef,
    ) -> Result<Step, Error> {
        let engine = self.engine();
        // Errors have already been diagnosed during resolution.
        if check.instance().contains_error() || check.expected().contains_error() {
            return Ok(Step::Derived(Vec::new()));
        }
        let actual = match check.instance().instance_trait_ref(engine).await {
            Ok(actual) => actual,
            Err(InstanceTraitRefError::Inference) => return Ok(Step::NoProgress),
            Err(InstanceTraitRefError::NotInstance) => return Err(Error::Conflicted),
            // Missing heads are recovery from an invalid instance declaration.
            Err(InstanceTraitRefError::Unresolved) => return Ok(Step::Derived(Vec::new())),
        };

        if actual.trait_id() != check.expected().trait_id() {
            return Err(Error::Conflicted);
        }

        let pairs =
            actual.args().structural_match(check.expected().args()).ok_or(Error::Conflicted)?;

        Ok(Step::Derived(
            pairs
                .map(|(actual, expected)| {
                    // Trait arguments are invariant.
                    DerivedConstraint::new_type_application_matching(
                        actual.clone(),
                        expected.clone(),
                        Variance::Invariant,
                    )
                })
                .collect(),
        ))
    }
}

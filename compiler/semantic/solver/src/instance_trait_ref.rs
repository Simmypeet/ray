//! Entailment for explicit given arguments.

use rayc_semantic_element::instance_trait_ref::get_instance_trait_ref;
use rayc_type::{
    constraint::instance_trait_ref::InstanceTraitRef,
    poly_var::{build_subst_from_args, get_poly_var_map},
    subst::Substitutable,
    ty::{Ty, application::View},
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
        let actual = match &**check.instance() {
            Ty::Inference(_) => return Ok(Step::NoProgress),
            Ty::PolyVar(id) => {
                engine.get_poly_var_map(id.parent_id()).await.trait_ref_of(id.id()).cloned()
            }
            Ty::Application(application) => {
                let View::Instance(instance) = application.view() else {
                    return Err(Error::Conflicted);
                };

                let head = engine.get_instance_trait_ref(instance.symbol_id()).await;
                let subst =
                    engine.build_subst_from_args(instance.symbol_id(), instance.args()).await;

                head.map(|head| head.apply_subst_or_clone(&subst, engine))
            }
            Ty::EffectRow(_) => return Err(Error::Conflicted),
        };

        // Missing heads are recovery from an invalid instance declaration.
        let Some(actual) = actual else { return Ok(Step::Derived(Vec::new())) };

        if actual.trait_id() != check.expected().trait_id() {
            return Err(Error::Conflicted);
        }

        let pairs =
            actual.args().structural_match(check.expected().args()).ok_or(Error::Conflicted)?;

        Ok(Step::Derived(
            pairs
                .map(|(actual, expected)| {
                    DerivedConstraint::new_type_application_matching(
                        actual.clone(),
                        expected.clone(),
                    )
                })
                .collect(),
        ))
    }
}

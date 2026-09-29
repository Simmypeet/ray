//! Finds the trait member an instance member implements, pairs their
//! polymorphic variables, and checks that the implementation conforms.
//!
//! - `correspondence` pairs the members' variables.
//! - `conformance` checks given requirements, where clauses and signatures.
//! - `diagnostic` reports every way the two can disagree.

use rayc_handler::Storage;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    member::get_member_by_name,
    name::get_name,
    parent::get_parent_global,
    span::get_span,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_type::{
    instance_member::{InstanceMember, Key},
    subst::Subst,
    trait_ref::get_instance_trait_ref,
};

use crate::{
    build::{Build, Output},
    register_build,
};

mod conformance;
mod correspondence;
mod diagnostic;

pub use conformance::ConformanceKey;
use correspondence::poly_var_substitution;
use diagnostic::Compatibility;
pub use diagnostic::{Diagnostic, Mismatch};

impl Build for Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let diagnostics = Storage::new();

        let instance_id = engine
            .get_parent_global(symbol_id)
            .await
            .expect("an instance member symbol should have a parent instance");
        let Some(trait_ref) = engine.get_instance_trait_ref(instance_id).await else {
            return Output::new_with(None, [], [], engine);
        };
        let name = engine.get_name(symbol_id).await;
        let Some(trait_member_id) = engine.get_member_by_name(trait_ref.trait_id(), &name).await
        else {
            return Output::new_with(None, [], [], engine);
        };
        let trait_member_span = engine
            .get_span(trait_member_id)
            .await
            .expect("a trait member symbol should have a source span");

        // Only corresponding member kinds share a polymorphic-variable mapping.
        let instance_kind = engine.get_symbol_kind(symbol_id).await;
        let trait_kind = engine.get_symbol_kind(trait_member_id).await;
        let matching_kinds = matches!(
            (trait_kind, instance_kind),
            (SymbolKind::TraitDef, SymbolKind::InstanceDef)
                | (SymbolKind::TraitType, SymbolKind::InstanceType)
        );
        let compatibility = Compatibility::new(
            name,
            engine.get_span(symbol_id).await.expect("an instance member should have a span"),
            trait_member_span,
            &diagnostics,
        );
        let substitution = if matching_kinds {
            poly_var_substitution(engine, &trait_ref, trait_member_id, symbol_id, &compatibility)
                .await
        } else {
            compatibility
                .report(Mismatch::MemberKind { expected: trait_kind, actual: instance_kind });
            None
        };

        let definition = InstanceMember::new(
            trait_member_id,
            symbol_id,
            substitution.unwrap_or_else(Subst::new_empty),
        );
        Output::new_with(Some(engine.intern(definition)), diagnostics.into_vec(), [], engine)
    }
}

register_build!(Key);

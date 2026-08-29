use qbice::storage::intern::Interned;
use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
use rayc_qbice::TrackedEngine;
use rayc_source_file::GlobalSourceID;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    constraint::{Constraint, Error, subtype::Subtype},
    ty::{Primitive, Ty, args::Args},
};

use super::{
    Cause, PendingConstraint, RootCause, RootCauseOrigin, SubtypeConstraintOrigin, SubtypeSource,
};
use crate::tast_builder::TAstBuilder;

fn test_span() -> RelativeSpan {
    RelativeSpan {
        start: RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ROOT_BRANCH_ID },
        end: RelativeLocation { offset: 1, mode: OffsetMode::End, relative_to: ROOT_BRANCH_ID },
        source_id: GlobalSourceID::default(),
    }
}

fn pending_constraint(builder: &mut TAstBuilder, subtype: Subtype) -> PendingConstraint {
    let cause_id = builder.constraint_solver.causes.insert(Cause::Root(RootCause {
        origin: RootCauseOrigin::Subtype(SubtypeConstraintOrigin {
            original_subtype: subtype.clone(),
            source: SubtypeSource::IfBranch,
            span: test_span(),
        }),
    }));

    PendingConstraint { constraint: Constraint::Subtype(subtype), cause_id }
}

fn state_effect(
    argument: Interned<Ty>,
    effect_id: GlobalSymbolID,
    engine: &TrackedEngine,
) -> Interned<Ty> {
    let label = engine.intern(rayc_type::ty::effect_row::EffectLabel::new(
        effect_id,
        Args::new([argument], engine),
    ));
    Ty::new_effect_row([label], None, engine)
}

// input: {State[int32]} = e, {State[bool]} = e
// premise: both constraints are queued before either one is solved
// output: Conflicted
#[tokio::test]
async fn queued_constraints_observe_prior_substitutions() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut builder = TAstBuilder::new(engine.clone(), GlobalSymbolID::default());
    let shared_effect = builder.new_effect_inference();
    let effect_id = GlobalSymbolID::default();
    let state_int32 =
        state_effect(Ty::new_primitive(Primitive::Int32, &engine), effect_id, &engine);
    let state_bool = state_effect(Ty::new_primitive(Primitive::Bool, &engine), effect_id, &engine);
    let int32_constraint =
        pending_constraint(&mut builder, Subtype::new(state_int32, shared_effect.clone()));
    let bool_constraint = pending_constraint(&mut builder, Subtype::new(state_bool, shared_effect));

    builder.push_constraints(vec![int32_constraint, bool_constraint]);

    assert_eq!(builder.constraint_solver.errored_constraints.len(), 1);
    assert_eq!(builder.constraint_solver.errored_constraints[0].0, Error::Conflicted);
}

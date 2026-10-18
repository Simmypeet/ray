//! Lowers the compiler-generated `Drop.drop` body of a nominal type.
//!
//! The fragment is keyed by a concrete `NominalDropInstance` dictionary. Its
//! body follows the nominal type's `GeneratedDropPlan`: each field recipe is
//! instantiated with the nominal's type arguments and the dictionary's
//! external instances, then dropped exactly like a stored dictionary would be.
//! A recursive field produces a call to another (or the same) generated
//! fragment rather than inlining its recipe.

use rayc_mono_ir::{
    MonoIR, MonoNominalDropInstance,
    cfg::Terminator,
    function::{Local, LocalKind},
    operand::{Constant, Operand},
    place::Place,
    ty::ReturnType,
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::drop_plan::{DropPlan, get_drop_plan};
use rayc_solver::Solver;
use rayc_type::subst::Subst;

use crate::{builder::Builder, resolver::Resolver};

/// Lowers one generated nominal Drop instance into its own `MonoIR` fragment.
pub(crate) async fn lower(engine: &TrackedEngine, instance: MonoNominalDropInstance) -> MonoIR {
    // The dictionary key is concrete, so no owner substitution applies.
    let resolver = Resolver::new(Solver::without_givens(engine.clone()).await, Subst::new_empty());
    let view = instance.view();
    let nominal = view.nominal().clone();
    let external_instances = view.external_instances();
    let struct_ = nominal.as_struct_view().expect("a nominal Drop instance must name a struct");

    // The solver only builds a `NominalDropInstance` from a generated plan.
    let plan = engine.get_drop_plan(struct_.symbol_id()).await;
    let DropPlan::Generated(plan) = &*plan else {
        panic!("a nominal Drop instance requires a generated Drop plan")
    };
    assert_eq!(
        plan.requirements().len(),
        external_instances.len(),
        "a nominal Drop instance supplies one dictionary per plan requirement"
    );

    // Recipes are written in the nominal declaration's generic context.
    let substitution = struct_.create_subst(engine).await;

    // Build `drop(self: Nominal) -> unit` with a scratch local that receives
    // the unit result of every field drop call.
    let signature = resolver.nominal_drop_signature(&nominal).await;
    let ReturnType::Value(unit) = signature.return_type().clone() else {
        panic!("a generated Drop body returns a unit value")
    };

    let mut output = MonoIR::new(instance.clone(), signature);
    let root_id = output.root_id();
    let mut builder = Builder::new(&mut output, root_id);

    let value = Place::new(builder.parameter_ids()[0]);
    let destination = Place::new(builder.insert_local(Local::new(unit, LocalKind::Temporary)));

    // Drop the owned fields in reverse declaration order, matching scope
    // teardown and the tuple Drop instance.
    for field in plan.fields().iter().rev() {
        let dictionary = field.dictionary().instantiate(&substitution, external_instances, engine);
        let field_place = value.clone().project_struct_field(field.field_id());
        builder.lower_drop(&resolver, field_place, &dictionary, destination.clone()).await;
    }

    builder.set_terminator(Terminator::Return(Some(Operand::Constant(Constant::Unit))));
    output
}

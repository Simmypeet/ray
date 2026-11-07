//! Lowers the compiler-generated `Drop.drop` body of a nominal type.
//!
//! The fragment is keyed by a concrete `GeneratedAdtDropInstance` dictionary.
//! Its body follows the nominal type's `GeneratedDropPlan`: each field recipe
//! is instantiated with the nominal's type arguments and the dictionary's
//! external instances, then dropped exactly like a stored dictionary would be.
//! A recursive field produces a call to another (or the same) generated
//! fragment rather than inlining its recipe.

use rayc_mono_ir::{
    MonoGeneratedAdtDropInstance, MonoIR,
    cfg::Terminator,
    operand::{Constant, Operand},
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::drop_plan::{DropPlan, GeneratedDropBody, get_drop_plan};
use rayc_solver::Solver;
use rayc_type::subst::Subst;

use crate::{emitter::Emitter, resolver::Resolver};

/// Lowers one generated nominal Drop instance into its own `MonoIR` fragment.
pub(crate) async fn lower(
    engine: &TrackedEngine,
    instance: MonoGeneratedAdtDropInstance,
) -> MonoIR {
    // The dictionary key is concrete, so no owner substitution applies.
    let resolver = Resolver::new(Solver::without_givens(engine.clone()).await, Subst::new_empty());
    let view = instance.view();
    let nominal = view.nominal().clone();
    let external_instances = view.external_instances();
    let adt = nominal.as_adt_view().expect("a nominal Drop instance must name a struct or an enum");

    // The solver only builds a `GeneratedAdtDropInstance` from a generated
    // plan.
    let plan = engine.get_drop_plan(adt.symbol_id()).await;
    let DropPlan::Generated(plan) = &*plan else {
        panic!("a nominal Drop instance requires a generated Drop plan")
    };
    assert_eq!(
        plan.requirements().len(),
        external_instances.len(),
        "a nominal Drop instance supplies one dictionary per plan requirement"
    );

    // Recipes are written in the nominal declaration's generic context.
    let substitution = adt.create_subst(engine).await;

    // Build `drop(self: Nominal) -> unit` with a scratch local that receives
    // the unit result of every field drop call.
    let signature = resolver.nominal_drop_signature(&nominal).await;
    let mut output = MonoIR::new(instance.clone(), signature, engine);
    let root_id = output.root_id();
    let mut emit = Emitter::new(&resolver, &mut output, root_id);

    let value = emit.local_place(emit.parameter_ids().next().expect("should have self parameter"));
    let destination = emit.new_unit_place();

    // TODO: lower the drop of an enum's payload once enum values are lowered
    // to MonoIR.
    let fields = match plan.body() {
        GeneratedDropBody::Struct(fields) => fields,
        GeneratedDropBody::Enum(_) => panic!("enum Drop bodies are not lowered to MonoIR yet"),
    };

    // Drop the owned fields in reverse declaration order, matching scope
    // teardown and the tuple Drop instance.
    for field in fields.iter().rev() {
        let dictionary = field.dictionary().instantiate(&substitution, external_instances, engine);
        let field_place = emit.project(&value).struct_field(field.field_id());
        emit.lower_drop(field_place, &dictionary, destination.clone()).await;
    }

    emit.set_terminator(Terminator::Return(Some(Operand::Constant(Constant::Unit))));
    output
}

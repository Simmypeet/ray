//! Lowers the compiler-generated `Drop.drop` body of a nominal type.
//!
//! The function is keyed by a concrete `GeneratedAdtDropInstance` dictionary.
//! Its body follows the nominal type's `GeneratedDropPlan`: each field recipe
//! is instantiated with the nominal's type arguments and the dictionary's
//! external instances, then dropped exactly like a stored dictionary would be.
//! An enum drops the payload of the variant it holds, which its discriminant
//! tells. A recursive field produces a call to another (or the same) generated
//! function rather than inlining its recipe.

use qbice::storage::intern::Interned;
use rayc_mono_ir::{
    MonoGeneratedAdtDropInstance,
    cfg::Terminator,
    function::MonoFunction,
    layout::EnumLayout,
    operand::{Constant, Operand},
    place::Place,
    rvalue::Rvalue,
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::drop_plan::{
    DictionaryExpr, DropPlan, FieldDrop, GeneratedDropBody, VariantDrop, get_drop_plan,
};
use rayc_solver::Solver;
use rayc_type::{subst::Subst, ty::Ty};

use crate::{emitter::Emitter, resolver::Resolver};

/// Lowers one generated nominal Drop instance into its own `MonoFunction`.
pub(crate) async fn lower(
    engine: &TrackedEngine,
    instance: MonoGeneratedAdtDropInstance,
) -> MonoFunction {
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
    let mut output = MonoFunction::new(instance.clone(), signature, engine);
    let mut emit = Emitter::new(&resolver, &mut output);

    let value = emit.local_place(emit.parameter_ids().next().expect("should have self parameter"));
    let destination = emit.new_unit_place();
    let drops = PayloadDrops { substitution: &substitution, external_instances, destination };

    match plan.body() {
        GeneratedDropBody::Struct(fields) => drops.drop_fields(&mut emit, &value, fields).await,
        GeneratedDropBody::Enum(variants) => {
            let ty = resolver.lower_type(&nominal).await;
            let layout = resolver.enum_layout(ty.assert_as_enum()).await;
            drops.drop_variants(&mut emit, &value, &layout, variants).await;
        }
    }

    emit.set_terminator(Terminator::Return(Some(Operand::Constant(Constant::Unit))));
    output
}

/// What dropping each owned value of a nominal needs: the nominal's type
/// arguments and external instances, which instantiate the plan's recipes,
/// and where the unit result of every drop call goes.
struct PayloadDrops<'a> {
    substitution: &'a Subst,
    external_instances: &'a [Interned<Ty>],
    destination: Place,
}

impl PayloadDrops<'_> {
    /// Drops the owned fields of the struct at `value` in reverse declaration
    /// order, matching scope teardown and the tuple Drop instance.
    async fn drop_fields(&self, emit: &mut Emitter<'_>, value: &Place, fields: &[FieldDrop]) {
        for field in fields.iter().rev() {
            let field_place = emit.project(value).struct_field(field.field_id());
            self.drop_owned(emit, field_place, field.dictionary()).await;
        }
    }

    /// Drops the payload of whichever variant the enum at `value` holds.
    ///
    /// The function switches on the discriminant to a block per variant,
    /// which drops its payload, if it has one, and goes on to the block that
    /// returns, which is left selected. An enum without variants has no
    /// value, so the switch has no target: dropping one is unreachable.
    ///
    /// ```c
    ///     discriminant = discriminant(value);
    ///     switch (discriminant) { case 0: goto drop_0; ...; default: unreachable; }
    /// drop_0:
    ///     drop(value.variant0);
    ///     goto after;
    /// ...
    /// after:
    /// ```
    async fn drop_variants(
        &self,
        emit: &mut Emitter<'_>,
        value: &Place,
        layout: &EnumLayout,
        variants: &[VariantDrop],
    ) {
        let engine = emit.engine();
        let discriminant_type = layout.discriminant_type();
        let discriminant = emit.new_temporary_place(engine.intern(discriminant_type.mono_type()));
        emit.assign(discriminant.clone(), Rvalue::Discriminant(value.clone()));

        // a variant without a payload has nothing to drop, so it goes to the
        // block after them directly
        let switch_block = emit.selected_block();
        let after = emit.create_block();
        let mut targets = Vec::with_capacity(variants.len());
        for variant in variants {
            let index = layout.variant_index(variant.variant_id());
            let target = match variant.payload() {
                Some(dictionary) => {
                    let block = emit.create_block();
                    emit.select_block(block);
                    let payload = emit.project(value).variant_payload(index);
                    self.drop_owned(emit, payload, dictionary).await;
                    emit.set_terminator(Terminator::Goto(after));
                    block
                }
                None => after,
            };
            targets.push((discriminant_type.constant(u64::from(index.index())), target));
        }

        emit.select_block(switch_block);
        emit.set_switch_terminator(Operand::Copy(discriminant), targets);
        emit.select_block(after);
    }

    /// Drops the value at `place` with the plan's recipe `dictionary`.
    async fn drop_owned(&self, emit: &mut Emitter<'_>, place: Place, dictionary: &DictionaryExpr) {
        let engine = emit.engine();
        let dictionary =
            dictionary.instantiate(self.substitution, self.external_instances, engine).await;
        emit.lower_drop(place, &dictionary, self.destination.clone()).await;
    }
}

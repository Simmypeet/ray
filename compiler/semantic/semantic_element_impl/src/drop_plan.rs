//! Computes Drop recipes before any concrete nominal dictionary is selected.
//!
//! The target query owns the calculation because two structs or enums in one
//! target can refer to each other. It first identifies explicit instances, then
//! repeatedly derives the remaining plans until their external requirements
//! stabilize. `Evaluator` turns each struct field type and enum payload type
//! into a dictionary expression: for example,
//! a `t` field uses `External(0)`, while `Option[Node[t]]` can pass a
//! `Generated` application to an explicit `DropOption` instance. This does
//! not recursively expand the field recipe for `Node[t]`.

use linkme::distributed_slice;
use qbice::{executor, program::Registration, storage::intern::Interned};
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    drop_plan::{
        DictionaryArgument, DictionaryExpr, DropPlan, DropPlanError, FieldDrop, GeneratedDropBody,
        GeneratedDropPlan, NominalDropPlan, TargetDropPlans, VariantDrop, get_drop_plan,
        get_target_drop_plans,
    },
    struct_body::get_struct_body,
    variant_payload_ty::get_variant_payload_ty,
};
use rayc_solver::Solver;
use rayc_symbol::{
    GlobalSymbolID, SymbolID,
    core_item::{CoreItem, get_core_item},
    member::get_members,
    symbol_kind::{
        SymbolKind, get_all_enum_ids, get_all_instance_ids, get_all_nominal_type_ids,
        get_symbol_kind,
    },
    syntax::is_linear_type,
};
use rayc_target::TargetID;
use rayc_type::{
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    subst::{Subst, Substitutable},
    trait_ref::get_instance_trait_ref,
    ty::{
        Ty,
        application::{AdtView, View as ApplicationView},
    },
    where_clause::get_where_clause,
};

const MAX_REQUIREMENT_PASSES: usize = 64;

/// The public per-nominal query is only a lookup into the defining target's
/// completed map. Foreign nominal fields can therefore request their own
/// target's result without reentering the current target's fixed point.
#[executor(config = Config)]
async fn nominal_drop_plan_executor(
    &NominalDropPlan { symbol_id }: &NominalDropPlan,
    engine: &TrackedEngine,
) -> Interned<DropPlan> {
    engine
        .get_target_drop_plans(symbol_id.target_id)
        .await
        .get(&symbol_id.id)
        .cloned()
        .expect("invalid id")
}

#[distributed_slice(RAY_PROGRAM)]
static NOMINAL_DROP_PLAN_EXECUTOR: Registration<Config> =
    Registration::new::<NominalDropPlan, NominalDropPlanExecutor>();

#[executor(config = Config)]
async fn target_drop_plans_executor(
    &TargetDropPlans { target_id }: &TargetDropPlans,
    engine: &TrackedEngine,
) -> Interned<FxHashMap<SymbolID, Interned<DropPlan>>> {
    let mut plans = FxHashMap::default();
    let mut adt_ids = Vec::new();

    // Reserve a plan slot for every local struct and enum before inspecting
    // any fields or payloads. These empty generated plans are the starting
    // approximation.
    let struct_ids = engine.get_all_nominal_type_ids(target_id).await;
    let enum_ids = engine.get_all_enum_ids(target_id).await;
    for (id, body) in struct_ids
        .iter()
        .map(|&id| (id, GeneratedDropBody::Struct(Vec::new())))
        .chain(enum_ids.iter().map(|&id| (id, GeneratedDropBody::Enum(Vec::new()))))
    {
        adt_ids.push(id);
        plans.insert(
            id,
            engine.intern(DropPlan::Generated(GeneratedDropPlan::new(Vec::new(), body))),
        );
    }
    // In-place updates expose earlier results to later constructors. Fix the
    // traversal order so requirement positions do not depend on hash iteration.
    adt_ids.sort_unstable();

    let explicit = explicit_drop_instances(engine, target_id).await;

    // An explicit implementation replaces generation. Multiple or unsupported
    // implementations become plan errors, so field derivation cannot pick one
    // by accident.
    for (&id, instances) in &explicit {
        if !plans.contains_key(&id) {
            continue;
        }
        let plan = match instances.as_slice() {
            [instance_id]
                if valid_explicit(engine, *instance_id, target_id.make_global(id)).await =>
            {
                DropPlan::Explicit(*instance_id)
            }
            [instance_id] => {
                DropPlan::CannotDerive(DropPlanError::InvalidExplicitInstance(*instance_id))
            }
            [_, _, ..] => {
                DropPlan::CannotDerive(DropPlanError::MultipleInstances(instances.clone()))
            }
            [] => unreachable!("the map contains only nonempty instance lists"),
        };
        plans.insert(id, engine.intern(plan));
    }

    // A linear struct or enum never gets a Drop plan, not even from an
    // explicit instance (which is rejected where the instance is declared).
    // Fields and payloads of a linear type then fail generation for their
    // enclosing types.
    for &id in &adt_ids {
        if engine.is_linear_type(target_id.make_global(id)).await {
            plans.insert(id, engine.intern(DropPlan::Linear));
        }
    }

    // Update each plan after building its replacement. Later constructors in
    // the sweep can use that new result; earlier ones see it on the next sweep.
    // The current entry stays in the map during construction so references to
    // the same nominal constructor can still inspect its old requirements.
    // Requirements only grow, and field recipes are rebuilt until an entire
    // sweep makes no changes. The map remains private until the query returns.
    // A chain can need one pass per nominal constructor. Keep additional fuel
    // for requirements that propagate through several constructors per cycle.
    let pass_limit =
        adt_ids.len().saturating_mul(MAX_REQUIREMENT_PASSES).max(MAX_REQUIREMENT_PASSES);

    for _ in 0..pass_limit {
        let mut changed = false;

        for &id in &adt_ids {
            let old = plans.get(&id).unwrap();
            let DropPlan::Generated(old) = &**old else { continue };

            let nominal_id = target_id.make_global(id);
            let next = build_generated(engine, nominal_id, old, &plans).await;

            if plans.get(&id).map(|x| &**x) != Some(&next) {
                plans.insert(id, engine.intern(next));
                changed = true;
            }
        }

        if !changed {
            return engine.intern(plans);
        }
    }

    // A growing requirement sequence may have no finite plan (for example,
    // recursive generic instantiations that keep changing the required type).
    // Return a plan error instead of leaving this query running indefinitely.
    for id in adt_ids {
        let plan = plans.get(&id).unwrap();

        if matches!(&**plan, DropPlan::Generated(_)) {
            plans.insert(
                id,
                engine.intern(DropPlan::CannotDerive(DropPlanError::NonConvergentRequirements)),
            );
        }
    }
    engine.intern(plans)
}

#[distributed_slice(RAY_PROGRAM)]
static TARGET_DROP_PLANS_EXECUTOR: Registration<Config> =
    Registration::new::<TargetDropPlans, TargetDropPlansExecutor>();

/// Groups the source-written Drop instances of `target_id` by the struct or
/// enum in their head. The same-target rule is checked when resolving the
/// instance head.
async fn explicit_drop_instances(
    engine: &TrackedEngine,
    target_id: TargetID,
) -> FxHashMap<SymbolID, Vec<GlobalSymbolID>> {
    let drop_trait = engine.get_core_item(CoreItem::DropTrait).await;
    let mut explicit: FxHashMap<SymbolID, Vec<GlobalSymbolID>> = FxHashMap::default();

    for id in engine.get_all_instance_ids(target_id).await.iter().copied() {
        let instance_id = target_id.make_global(id);
        let Some(head) = engine.get_instance_trait_ref(instance_id).await else { continue };
        if head.trait_id() != drop_trait || head.args().len() != 1 {
            continue;
        }

        let Some(ty) = head.args().interned_iter().next() else { continue };
        let Some(adt_id) = ty.as_adt_id() else { continue };

        if adt_id.target_id == target_id {
            explicit.entry(adt_id.id).or_default().push(instance_id);
        }
    }

    explicit
}

/// Restricts an explicit Drop declaration to the first milestone's simple
/// form. This lets a generated outer plan instantiate its head by matching
/// each nominal type argument to exactly one instance type variable.
async fn valid_explicit(
    engine: &TrackedEngine,
    instance_id: GlobalSymbolID,
    nominal_id: GlobalSymbolID,
) -> bool {
    let Some(head) = engine.get_instance_trait_ref(instance_id).await else { return false };
    let Some(adt) = head.args().interned_iter().next().and_then(|x| x.as_adt_view()) else {
        return false;
    };

    if adt.symbol_id() != nominal_id {
        return false;
    }

    let instance_params = engine.get_poly_var_map(instance_id).await;
    let nominal_params = engine.get_poly_var_map(nominal_id).await;
    if adt.args().len() != nominal_params.len() {
        return false;
    }

    let mut seen = FxHashSet::default();
    let mut rename = Subst::new_empty();
    for (argument, (nominal_parameter, _)) in adt.args().iter().zip(nominal_params.iter()) {
        // should exactly be poly var
        let Ty::PolyVar(id) = &**argument else { return false };

        // it should obviously be a parameter of the instance
        if id.parent_id() != instance_id || !seen.insert(id.id()) {
            return false;
        }

        rename.insert(
            *id,
            engine.intern(Ty::PolyVar(GlobalPolyVarID::new(nominal_id, nominal_parameter))),
        );
    }

    let drop_trait = engine.get_core_item(CoreItem::DropTrait).await;

    // Generated callers can supply only Drop dictionaries for head variables
    // or associated projections determined by those variables. A projection
    // containing an unbound instance parameter cannot be reconstructed here.
    for (param_id, parameter) in instance_params.iter() {
        let Some(given) = parameter.trait_ref() else { continue };

        // if this dictionary has already been constrainted by the head, then
        // we don't need to check it again
        if !seen.insert(param_id) {
            continue;
        }

        // unconstrainted parameters should be a Drop dictionary.
        if given.trait_id() != drop_trait || given.args().len() != 1 {
            return false;
        }

        let Some(required) = given.args().interned_iter().next() else { return false };
        match &**required {
            Ty::PolyVar(id) if seen.contains(&id.id()) && id.parent_id() == instance_id => {}

            Ty::Application(application)
                if matches!(application.view(), ApplicationView::InstanceAssociated(_)) => {}

            Ty::Application(_)
            | Ty::Inference(_)
            | Ty::PolyVar(_)
            | Ty::SelfInstance(_)
            | Ty::EffectRow(_)
            | Ty::Lifetime(_) => return false,
        }
    }

    // if length differs, this implies that there are some parameters not
    // constrained by the head or any given, which is not allowed
    if seen.len() != instance_params.len() {
        return false;
    }

    // The nominal declaration must already guarantee every predicate added
    // by the explicit instance. This check guarantee there's no extraneous
    // predicates introduced by this drop instance; TODO: broader predicate
    // entailment belongs to a later milestone.
    let original = engine.get_where_clause(nominal_id).await;
    let implementation = engine.get_where_clause(instance_id).await;
    implementation.predicates().all(|predicate| {
        let renamed = predicate.kind().apply_subst_or_clone(&rename, engine);
        original.predicates().any(|nominal_predicate| nominal_predicate.kind() == &renamed)
    })
}

/// Rechecks every field or payload using the latest available plans.
/// Retaining old requirements keeps their `External` indices stable across
/// passes.
async fn build_generated(
    engine: &TrackedEngine,
    nominal_id: GlobalSymbolID,
    old: &GeneratedDropPlan,
    plans: &FxHashMap<SymbolID, Interned<DropPlan>>,
) -> DropPlan {
    let mut evaluator = Evaluator {
        engine,
        solver: Solver::with_givens(engine.clone(), nominal_id, []).await,
        current_nominal: nominal_id,
        plans,
        requirements: old.requirements().to_vec(),
    };

    let body = if engine.get_symbol_kind(nominal_id).await == SymbolKind::Enum {
        evaluator.enum_body(nominal_id).await
    } else {
        evaluator.struct_body(nominal_id).await
    };

    match body {
        Ok(body) => DropPlan::Generated(GeneratedDropPlan::new(evaluator.requirements, body)),
        Err(error) => DropPlan::CannotDerive(error),
    }
}

/// Resolves field Drop requirements in the generic context of one generated
/// nominal instance. `plans` contains the latest entries for the current
/// target; foreign nominal types use their completed per-symbol query instead.
struct Evaluator<'a> {
    engine: &'a TrackedEngine,
    solver: Solver,
    current_nominal: GlobalSymbolID,
    plans: &'a FxHashMap<SymbolID, Interned<DropPlan>>,
    requirements: Vec<Interned<Ty>>,
}

impl Evaluator<'_> {
    /// Selects a dictionary for every field of the struct `struct_id`, in
    /// declaration order.
    async fn struct_body(
        &mut self,
        struct_id: GlobalSymbolID,
    ) -> Result<GeneratedDropBody, DropPlanError> {
        let body = self.engine.get_struct_body(struct_id).await;

        let mut fields = Vec::with_capacity(body.len());
        for (field_id, field) in body.iter() {
            fields.push(FieldDrop::new(field_id, self.resolve(field.ty().clone()).await?));
        }

        Ok(GeneratedDropBody::Struct(fields))
    }

    /// Selects a dictionary for the payload of every variant of the enum
    /// `enum_id`. Variants are visited by symbol ID so that requirement
    /// indices do not depend on hash iteration.
    async fn enum_body(
        &mut self,
        enum_id: GlobalSymbolID,
    ) -> Result<GeneratedDropBody, DropPlanError> {
        let mut variant_ids = self.engine.get_members(enum_id).await.all_ids().collect::<Vec<_>>();
        variant_ids.sort_unstable();

        let mut variants = Vec::with_capacity(variant_ids.len());
        for variant_id in variant_ids {
            let variant_id = enum_id.target_id.make_global(variant_id);
            let payload = match self.engine.get_variant_payload_ty(variant_id).await {
                Some(payload) => Some(self.resolve(payload).await?),
                None => None,
            };

            variants.push(VariantDrop::new(variant_id, payload));
        }

        Ok(GeneratedDropBody::Enum(variants))
    }

    /// Constructs a finite expression for the dictionary needed by `ty`.
    /// `Box::pin` permits recursive async calls for tuple elements and the
    /// premises of other nominal or explicit instances.
    async fn resolve(&mut self, ty: Interned<Ty>) -> Result<DictionaryExpr, DropPlanError> {
        Box::pin(async {
            // Substitution can make an associated projection concrete. Reduce
            // it before classifying the type as an external requirement or a
            // nominal dictionary. Declaration predicates are deliberately not
            // visible here: only definitionally reducible types normalize.
            let ty = self.solver.normalize(&ty).await;

            match &*ty {
                Ty::PolyVar(_) => Ok(self.external(ty)),
                Ty::Application(application) => match application.view() {
                    ApplicationView::Primitive(_)
                    | ApplicationView::Pointer(_)
                    | ApplicationView::Reference(_) => Ok(DictionaryExpr::NoOp(ty)),
                    ApplicationView::Tuple(tuple) => {
                        let mut elements = Vec::with_capacity(tuple.args().len());
                        for element in tuple.args() {
                            elements.push(self.resolve(element.clone()).await?);
                        }
                        Ok(DictionaryExpr::Tuple { tuple: ty, elements })
                    }
                    ApplicationView::Closure(closure) => {
                        // Uninferred captures cannot occur in a declaration's
                        // field types, so the captured tuple is always known.
                        let Some(captured_tuple) = closure.captured_tuple().as_tuple_view() else {
                            return Err(DropPlanError::MissingFieldDictionary(ty));
                        };

                        let mut captures = Vec::with_capacity(captured_tuple.args().len());
                        for capture in captured_tuple.args() {
                            captures.push(self.resolve(capture.clone()).await?);
                        }
                        Ok(DictionaryExpr::Closure { closure: ty, captures })
                    }
                    ApplicationView::Struct(_) | ApplicationView::Enum(_) => {
                        let adt = ty.as_adt_view().expect("the type is a struct or an enum");
                        self.adt(&ty, adt).await
                    }
                    ApplicationView::InstanceAssociated(_) => Ok(self.external(ty)),
                    ApplicationView::Instance(_)
                    | ApplicationView::DefInstance(_)
                    | ApplicationView::NoOpDropInstance(_)
                    | ApplicationView::TupleDropInstance(_)
                    | ApplicationView::ClosureDropInstance(_)
                    | ApplicationView::DefCoroutine(_)
                    | ApplicationView::TraitDefCoroutine(_)
                    | ApplicationView::ContinuationDefInstance(_)
                    | ApplicationView::CoroutineInstance(_)
                    | ApplicationView::CoroutineDropInstance(_)
                    | ApplicationView::GeneratedAdtDropInstance(_)
                    | ApplicationView::Error => Err(DropPlanError::MissingFieldDictionary(ty)),
                },
                Ty::Inference(_) | Ty::SelfInstance(_) | Ty::EffectRow(_) | Ty::Lifetime(_) => {
                    Err(DropPlanError::MissingFieldDictionary(ty))
                }
            }
        })
        .await
    }

    /// Selects the dictionary of `ty`, the struct or enum `adt`, from its
    /// plan.
    async fn adt(
        &mut self,
        ty: &Interned<Ty>,
        adt: AdtView<'_>,
    ) -> Result<DictionaryExpr, DropPlanError> {
        let symbol_id = adt.symbol_id();

        // Same-target plans may still be changing in this sweep. A foreign
        // target cannot be in this local fixed point, so its completed query
        // result is safe to request.
        let same_target = symbol_id.target_id == self.current_nominal.target_id;
        let foreign_plan =
            if same_target { None } else { Some(self.engine.get_drop_plan(symbol_id).await) };

        let plan = if same_target {
            self.plans.get(&symbol_id.id).map(|x| &**x)
        } else {
            foreign_plan.as_deref()
        };

        match plan {
            Some(DropPlan::Generated(generated)) => {
                // Substitute the actual type arguments while borrowing the
                // nested plan. The resulting types outlive that borrow, letting
                // recursive resolution use `&mut self` without cloning the
                // nested plan's recipes.
                let subst = adt.create_subst(self.engine).await;

                let mut external = Vec::with_capacity(generated.requirements().len());

                for required in generated
                    .requirements()
                    .iter()
                    .map(|x| x.apply_subst_or_clone(&subst, self.engine))
                {
                    external.push(self.resolve(required).await?);
                }
                Ok(DictionaryExpr::Generated { nominal: ty.clone(), external })
            }
            Some(DropPlan::Explicit(instance_id)) => self.explicit(*instance_id, adt.args()).await,
            Some(DropPlan::Linear) => Err(DropPlanError::LinearField(ty.clone())),
            Some(DropPlan::CannotDerive(_)) | None => {
                Err(DropPlanError::MissingFieldDictionary(ty.clone()))
            }
        }
    }

    fn external(&mut self, ty: Interned<Ty>) -> DictionaryExpr {
        // Reuse an existing parameter for repeated requirements such as two
        // fields of type `t`; only new leaf requirements extend the plan.
        let index =
            self.requirements.iter().position(|requirement| requirement == &ty).unwrap_or_else(
                || {
                    let index = self.requirements.len();
                    self.requirements.push(ty);
                    index
                },
            );
        DictionaryExpr::External(index)
    }

    async fn explicit(
        &mut self,
        instance_id: GlobalSymbolID,
        actual_args: &[Interned<Ty>],
    ) -> Result<DictionaryExpr, DropPlanError> {
        // The earlier simple-head check guarantees that matching the nominal
        // arguments determines every ordinary instance type parameter.
        let head = self
            .engine
            .get_instance_trait_ref(instance_id)
            .await
            .expect("explicit plan has a head");
        let adt = head
            .args()
            .interned_iter()
            .next()
            .and_then(|x| x.as_adt_view())
            .expect("Drop has one argument");

        let mut subst = Subst::new_empty();
        for (head_arg, actual) in adt.args().iter().zip(actual_args) {
            let Ty::PolyVar(id) = &**head_arg else { unreachable!() };
            subst.insert(*id, actual.clone());
        }

        // Build arguments in the declaration's parameter order. A `given`
        // parameter recursively asks for its required Drop dictionary. A
        // recursive nominal reference applies its generated plan using the
        // current plan's external requirements.
        let params = self.engine.get_poly_var_map(instance_id).await;
        let mut arguments = Vec::with_capacity(params.len());
        for (id, parameter) in params.iter() {
            if let Some(given) = parameter.trait_ref() {
                let required = given.args().interned_iter().next().expect("Drop has one argument");
                let required = required.apply_subst_or_clone(&subst, self.engine);
                let dictionary = self.resolve(required).await?;
                arguments.push(DictionaryArgument::Dictionary(Box::new(dictionary)));
            } else {
                let arg = subst
                    .get(&GlobalPolyVarID::new(instance_id, id))
                    .expect("simple head binds every ordinary parameter");
                arguments.push(DictionaryArgument::Type(arg.clone()));
            }
        }
        Ok(DictionaryExpr::Explicit { instance_id, arguments })
    }
}

#[cfg(test)]
mod test;

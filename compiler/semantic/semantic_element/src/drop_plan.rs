//! Structural Drop recipes for nominal types.

use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_hash::FxHashMap;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    GlobalSymbolID, SymbolID,
    core_item::{CoreItem, get_core_item},
};
use rayc_target::TargetID;
use rayc_type::{
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{Ty, args::Args},
};

use crate::struct_body::FieldID;

/// A dictionary expression in the generic context of a generated Drop plan.
/// `External` indexes `GeneratedDropPlan::requirements`. A recursive nominal
/// reference uses `Generated` without expanding that nominal's field recipe.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub enum DictionaryExpr {
    External(usize),
    NoOp(Interned<Ty>),
    Tuple {
        tuple: Interned<Ty>,
        elements: Vec<Self>,
    },
    /// One dictionary per capture of the closure, in environment order.
    Closure {
        closure: Interned<Ty>,
        captures: Vec<Self>,
    },
    Explicit {
        instance_id: GlobalSymbolID,
        arguments: Vec<DictionaryArgument>,
    },
    Generated {
        nominal: Interned<Ty>,
        external: Vec<Self>,
    },
}

impl DictionaryExpr {
    /// Turns this plan expression into a concrete dictionary type.
    /// `substitution` maps the nominal declaration's polymorphic variables to
    /// the nominal's type arguments, and `external_instances` are the
    /// dictionaries chosen for the plan's requirements, in order.
    #[must_use]
    pub fn instantiate(
        &self,
        substitution: &Subst,
        external_instances: &[Interned<Ty>],
        engine: &TrackedEngine,
    ) -> Interned<Ty> {
        let ty = |ty: &Interned<Ty>| ty.apply_subst_or_clone(substitution, engine);
        let instantiate =
            |expression: &Self| expression.instantiate(substitution, external_instances, engine);

        match self {
            Self::External(index) => external_instances[*index].clone(),
            Self::NoOp(no_op) => Ty::new_no_op_drop_instance(ty(no_op), engine),
            Self::Tuple { tuple, elements } => {
                Ty::new_tuple_drop_instance(ty(tuple), elements.iter().map(instantiate), engine)
            }
            Self::Closure { closure, captures } => {
                Ty::new_closure_drop_instance(ty(closure), captures.iter().map(instantiate), engine)
            }

            // Explicit instance arguments are already in the instance's
            // polymorphic-variable order, which is how instance applications
            // store their arguments.
            Self::Explicit { instance_id, arguments } => {
                let arguments = arguments.iter().map(|argument| match argument {
                    DictionaryArgument::Type(argument) => ty(argument),
                    DictionaryArgument::Dictionary(dictionary) => instantiate(dictionary.as_ref()),
                });
                Ty::new_instance(*instance_id, Args::new(arguments, engine), engine)
            }

            // Recursive nominal references stay as dictionaries rather than
            // expanding their recipes.
            Self::Generated { nominal, external } => {
                Ty::new_nominal_drop_instance(ty(nominal), external.iter().map(instantiate), engine)
            }
        }
    }
}

/// An argument to a source-declared Drop instance, in parameter order.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub enum DictionaryArgument {
    Type(Interned<Ty>),
    Dictionary(Box<DictionaryExpr>),
}

/// One owned struct field and the dictionary selected to drop it.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct FieldDrop {
    field_id: FieldID,
    dictionary: DictionaryExpr,
}

impl FieldDrop {
    #[must_use]
    pub const fn new(field_id: FieldID, dictionary: DictionaryExpr) -> Self {
        Self { field_id, dictionary }
    }

    #[must_use]
    pub const fn field_id(&self) -> FieldID { self.field_id }

    #[must_use]
    pub const fn dictionary(&self) -> &DictionaryExpr { &self.dictionary }
}

/// A generic structural plan. Requirements are Drop implementor types in the
/// nominal declaration's polymorphic context, in dictionary argument order.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct GeneratedDropPlan {
    requirements: Vec<Interned<Ty>>,
    fields: Vec<FieldDrop>,
}

impl GeneratedDropPlan {
    #[must_use]
    pub const fn new(requirements: Vec<Interned<Ty>>, fields: Vec<FieldDrop>) -> Self {
        Self { requirements, fields }
    }

    #[must_use]
    pub fn requirements(&self) -> &[Interned<Ty>] { &self.requirements }

    #[must_use]
    pub fn fields(&self) -> &[FieldDrop] { &self.fields }
}

/// Why a nominal type cannot have a structural Drop plan.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub enum DropPlanError {
    MultipleInstances(Vec<GlobalSymbolID>),
    InvalidExplicitInstance(GlobalSymbolID),
    MissingFieldDictionary(Interned<Ty>),
    /// A field's type is a linear struct, which has no Drop instance.
    LinearField(Interned<Ty>),
    NonConvergentRequirements,
}

/// The Drop behavior selected for one nominal constructor.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub enum DropPlan {
    /// The struct is declared `@linear`: it has no Drop instance, so its
    /// values must always be consumed explicitly.
    Linear,
    Explicit(GlobalSymbolID),
    Generated(GeneratedDropPlan),
    CannotDerive(DropPlanError),
}

impl DropPlan {
    /// Returns whether the struct is dropped by a `Drop` instance declared
    /// for it in source, which may use anything a value of the struct can
    /// reach, rather than by dropping its fields one by one.
    #[must_use]
    pub const fn is_explicit(&self) -> bool {
        match self {
            Self::Explicit(_) => true,
            Self::Linear | Self::Generated(_) | Self::CannotDerive(_) => false,
        }
    }
}

/// Computes all nominal plans in a target together, allowing recursive
/// constructors to reach a common fixed point without recursive query calls.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<FxHashMap<SymbolID, Interned<DropPlan>>>)]
#[extend(by_val, name = get_target_drop_plans)]
pub struct TargetDropPlans {
    pub target_id: TargetID,
}

/// Looks up one nominal plan in its defining target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<DropPlan>)]
#[extend(by_val, name = get_drop_plan)]
pub struct NominalDropPlan {
    pub symbol_id: GlobalSymbolID,
}

/// Explains a missing `Drop` dictionary when the cause is a linear struct, for
/// the diagnostics that report such a failure. Returns `None` for any other
/// requirement or cause.
pub async fn linear_drop_help(engine: &TrackedEngine, trait_ref: &TraitRef) -> Option<String> {
    // Only a `Drop[SomeStruct[...]]` requirement can fail due to linearity.
    if trait_ref.trait_id() != engine.get_core_item(CoreItem::DropTrait).await {
        return None;
    }
    let ty = trait_ref.args().interned_iter().next()?;
    let struct_ = ty.as_struct_view()?;

    // Name the linear type: either the struct itself or the field type that
    // prevented generating its plan.
    match &*engine.get_drop_plan(struct_.symbol_id()).await {
        DropPlan::Linear => Some(format!(
            "`{}` is a linear type: it has no Drop instance and must be consumed explicitly",
            ty.display(engine).await
        )),
        DropPlan::CannotDerive(DropPlanError::LinearField(field)) => Some(format!(
            "`{}` has no generated Drop instance because it contains the linear type `{}`; \
             consume it explicitly or implement Drop for it by hand",
            ty.display(engine).await,
            field.display(engine).await
        )),
        DropPlan::Explicit(_) | DropPlan::Generated(_) | DropPlan::CannotDerive(_) => None,
    }
}

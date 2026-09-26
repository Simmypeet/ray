//! The variance of struct and `eff` parameters.
//!
//! The variance of a struct parameter is the join of the variances of its
//! occurrences in the struct's field types. The variance of an `eff` parameter
//! comes from its operation signatures: a `perform` calls the handler, so
//! parameter types start covariant and the return type starts contravariant.
//!
//! Structs can be recursive and mutually recursive, and structs and effects
//! can mention each other: an operation signature can name a struct, and a
//! struct can name an effect row inside a closure type. So every struct and
//! `eff` in a target is computed together as one fixed point that starts every
//! parameter at bivariant.

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    parameter::get_parameter_map, return_type::get_return_type, struct_body::get_struct_body,
};
use rayc_symbol::{
    GlobalSymbolID, SymbolID,
    member::get_members,
    source_map::to_absolute_span,
    symbol_kind::{SymbolKind, get_all_effect_ids, get_all_nominal_type_ids, get_symbol_kind},
};
use rayc_target::TargetID;
use rayc_type::{
    poly_var::get_poly_var_map,
    ty::Ty,
    variance::{Variance, VarianceKey, VarianceMap, get_variance},
};

#[cfg(test)]
mod test;

/// Retrieves the variances of every struct and `eff` in a target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<FxHashMap<SymbolID, Interned<VarianceMap>>>)]
struct VariancesKey {
    target_id: TargetID,
}

#[executor(config = Config)]
async fn variances_executor(
    &VariancesKey { target_id }: &VariancesKey,
    engine: &TrackedEngine,
) -> Interned<FxHashMap<SymbolID, Interned<VarianceMap>>> {
    // Start every parameter of every struct and effect at bivariant.
    let struct_ids = engine.get_all_nominal_type_ids(target_id).await;
    let effect_ids = engine.get_all_effect_ids(target_id).await;
    let owner_ids = struct_ids
        .iter()
        .map(|id| (*id, VarianceOwner::Struct))
        .chain(effect_ids.iter().map(|id| (*id, VarianceOwner::Effect)));

    let mut owners = Vec::with_capacity(struct_ids.len() + effect_ids.len());
    let mut variances = FxHashMap::default();
    for (owner_id, owner) in owner_ids {
        let owner_id = target_id.make_global(owner_id);
        let poly_vars = engine.get_poly_var_map(owner_id).await;
        variances.insert(owner_id, VarianceMap::new_unused(&poly_vars));
        owners.push((owner_id, owner, poly_vars));
    }

    // Grow the variances until nothing changes, then make every unused
    // parameter that is not a lifetime invariant. That can make a use of it
    // elsewhere invariant too, so grow again until the defaults change
    // nothing. Variances only move up the lattice, so this terminates.
    loop {
        let mut changed = true;
        while changed {
            changed = false;
            for (owner_id, owner, _) in &owners {
                let walker = VarianceWalker::new(*owner_id, &mut variances, engine);
                changed |= walker.walk_declaration(*owner).await;
            }
        }

        let mut defaulted = false;
        for (owner_id, _, poly_vars) in &owners {
            let map = variances.get_mut(owner_id).expect("every owner has variances");
            defaulted |= map.default_unused(poly_vars);
        }
        if !defaulted {
            break;
        }
    }

    engine.intern(
        variances
            .into_iter()
            .map(|(owner_id, variances)| (owner_id.id, engine.intern(variances)))
            .collect(),
    )
}

#[distributed_slice(RAY_PROGRAM)]
static VARIANCES_EXECUTOR: Registration<Config> =
    Registration::new::<VariancesKey, VariancesExecutor>();

#[executor(config = Config)]
async fn variance_executor(
    &VarianceKey { symbol_id }: &VarianceKey,
    engine: &TrackedEngine,
) -> Interned<VarianceMap> {
    let variances = engine.query(&VariancesKey { target_id: symbol_id.target_id }).await;
    variances.get(&symbol_id.id).cloned().expect("incorrect key")
}

#[distributed_slice(RAY_PROGRAM)]
static VARIANCE_EXECUTOR: Registration<Config> =
    Registration::new::<VarianceKey, VarianceExecutor>();

/// Joins the variance of every occurrence of an owner's polymorphic variables
/// in the types it walks.
struct VarianceWalker<'a> {
    owner_id: GlobalSymbolID,

    /// The current variances of every struct and effect in the owner's
    /// target, the owner's own included. Those of other targets are already
    /// final and come from [`get_variance`].
    local: &'a mut FxHashMap<GlobalSymbolID, VarianceMap>,
    engine: &'a TrackedEngine,

    /// Whether any variance of the owner changed.
    changed: bool,
}

impl<'a> VarianceWalker<'a> {
    /// Creates a walker that joins uses into the variances of `owner_id` in
    /// `local`, which holds the current variances of every owner of its
    /// target.
    const fn new(
        owner_id: GlobalSymbolID,
        local: &'a mut FxHashMap<GlobalSymbolID, VarianceMap>,
        engine: &'a TrackedEngine,
    ) -> Self {
        Self { owner_id, local, engine, changed: false }
    }

    /// Joins every use in the owner's declaration into its variances.
    /// Returns whether they changed.
    async fn walk_declaration(mut self, owner: VarianceOwner) -> bool {
        match owner {
            VarianceOwner::Struct => {
                for (_, field) in self.engine.get_struct_body(self.owner_id).await.iter() {
                    self.walk(field.ty(), Variance::Covariant).await;
                }
            }

            // The handler receives the arguments and supplies the result.
            VarianceOwner::Effect => {
                for operation_id in self.engine.get_members(self.owner_id).await.all_ids() {
                    let operation_id = self.owner_id.target_id.make_global(operation_id);
                    for (_, parameter) in self.engine.get_parameter_map(operation_id).await.iter() {
                        self.walk(parameter.ty(), Variance::Covariant).await;
                    }
                    let return_type = self.engine.get_return_type(operation_id).await;
                    self.walk(&return_type, Variance::Contravariant).await;
                }
            }
        }

        self.changed
    }

    /// Returns the current variances of a struct or effect of the owner's
    /// target, including the owner itself.
    fn local_variances(&self, symbol_id: GlobalSymbolID) -> &VarianceMap {
        self.local.get(&symbol_id).expect("every owner of the target has variances")
    }

    /// Joins the variances of the owner's variables in `root`, which occurs at
    /// a position of variance `ambient`.
    async fn walk(&mut self, root: &Interned<Ty>, ambient: Variance) {
        let mut pending = vec![(root.clone(), ambient)];

        while let Some((ty, variance)) = pending.pop() {
            // Nothing under a bivariant position is a use.
            if variance == Variance::Bivariant {
                continue;
            }

            match &*ty {
                Ty::PolyVar(poly_var) => {
                    if poly_var.parent_id() == self.owner_id {
                        let own =
                            self.local.get_mut(&self.owner_id).expect("the owner has variances");
                        self.changed |= own.join(poly_var.id(), variance);
                    }
                }

                Ty::Application(application) => {
                    // NOTE: `foreign` is needed because `declared` is a Option<&VarianceMap> and we
                    // need to have a place to keep the foreign VarianceMap alive
                    let foreign;
                    let declared = match application.struct_id() {
                        Some(struct_id) if struct_id.target_id == self.owner_id.target_id => {
                            Some(self.local_variances(struct_id))
                        }
                        Some(struct_id) => {
                            foreign = self.engine.get_variance(struct_id).await;
                            Some(&*foreign)
                        }
                        None => None,
                    };

                    pending.extend(
                        application
                            .arguments_with_variance(declared)
                            .map(|(arg, position)| (arg.clone(), variance.xform(position))),
                    );
                }

                // Each label's arguments follow its effect's variances, and
                // the tail keeps the row's position.
                Ty::EffectRow(row) => {
                    for label in row.labels() {
                        let effect_id = label.effect_symbol_id();

                        // NOTE: `foreign` is needed because `declared` is a Option<&VarianceMap>
                        // and we need to have a place to keep the foreign
                        // VarianceMap alive
                        let foreign;
                        let declared = if effect_id.target_id == self.owner_id.target_id {
                            self.local_variances(effect_id)
                        } else {
                            foreign = self.engine.get_variance(effect_id).await;
                            &*foreign
                        };

                        pending.extend(
                            label
                                .arguments_with_variance(declared)
                                .map(|(arg, position)| (arg.clone(), variance.xform(position))),
                        );
                    }
                    pending.extend(row.tail().map(|tail| (tail.clone(), variance)));
                }

                Ty::Lifetime(_) | Ty::Inference(_) | Ty::SelfInstance(_) => {}
            }
        }
    }
}

/// The kinds of declarations whose parameters have a computed variance.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub enum VarianceOwner {
    Struct,
    Effect,
}

impl VarianceOwner {
    /// Returns the owner kind of a symbol, or `None` if its parameters have
    /// no computed variance.
    #[must_use]
    pub const fn of(kind: SymbolKind) -> Option<Self> {
        match kind {
            SymbolKind::Strut => Some(Self::Struct),
            SymbolKind::Effect => Some(Self::Effect),
            SymbolKind::Def
            | SymbolKind::EffectOperation
            | SymbolKind::ExternDef
            | SymbolKind::Instance
            | SymbolKind::InstanceDef
            | SymbolKind::InstanceType
            | SymbolKind::Marker
            | SymbolKind::MarkerImplementation
            | SymbolKind::Module
            | SymbolKind::Trait
            | SymbolKind::TraitDef
            | SymbolKind::TraitType => None,
        }
    }

    /// Returns where a lifetime parameter of this owner can be used.
    const fn usage_site(self) -> &'static str {
        match self {
            Self::Struct => "a field",
            Self::Effect => "an operation signature",
        }
    }
}

/// A lifetime parameter of a struct or `eff` that nothing uses, as Rust's
/// E0392.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct UnusedLifetimeParameter {
    name: Interned<str>,
    span: RelativeSpan,
    owner: VarianceOwner,
}

impl Report for UnusedLifetimeParameter {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let usage = self.owner.usage_site();
        Rendered::builder()
            .message(format!("lifetime parameter `{}` is never used", &*self.name))
            .help_message(format!(
                "consider removing `{}` or referring to it in {usage}",
                &*self.name
            ))
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.span).await)
                    .message("unused lifetime parameter")
                    .build(),
            )
            .build()
    }
}

/// Retrieves the unused lifetime parameters of a struct or `eff`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[UnusedLifetimeParameter]>)]
pub struct UnusedLifetimeKey {
    /// A `SymbolKind::Strut` or `SymbolKind::Effect` symbol.
    pub symbol_id: GlobalSymbolID,
}

#[executor(config = Config)]
async fn unused_lifetime_executor(
    &UnusedLifetimeKey { symbol_id }: &UnusedLifetimeKey,
    engine: &TrackedEngine,
) -> Interned<[UnusedLifetimeParameter]> {
    let owner = VarianceOwner::of(engine.get_symbol_kind(symbol_id).await)
        .expect("only structs and effects have computed variances");
    let variances = engine.get_variance(symbol_id).await;

    // Only lifetimes stay bivariant after unused parameters are defaulted.
    let poly_vars = engine.get_poly_var_map(symbol_id).await;
    engine.intern_unsized(
        poly_vars
            .iter()
            .filter(|(id, _)| variances.get(*id) == Variance::Bivariant)
            .map(|(_, poly_var)| UnusedLifetimeParameter {
                name: poly_var.name().clone(),
                span: poly_var.span(),
                owner,
            })
            .collect::<Vec<_>>(),
    )
}

#[distributed_slice(RAY_PROGRAM)]
static UNUSED_LIFETIME_EXECUTOR: Registration<Config> =
    Registration::new::<UnusedLifetimeKey, UnusedLifetimeExecutor>();

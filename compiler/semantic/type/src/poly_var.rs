use std::{collections::hash_map::Entry, ops::Index};

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_arena::{ID, OrderedArena};
use rayc_extend::extend;
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::{
    GlobalSymbolID, MemberID, parent::get_parent_global, symbol_kind::get_symbol_kind,
};

use crate::{
    subst::Subst,
    trait_ref::TraitRef,
    ty::{Ty, TyKind},
    variance::Variance,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum PolyVarKind {
    Type(TyKind),
    Instance(TraitRef),
}

impl PolyVarKind {
    #[must_use]
    pub const fn ty_kind(&self) -> TyKind {
        match self {
            Self::Type(kind) => *kind,
            Self::Instance(_) => TyKind::Instance,
        }
    }
}

/// Identifies whether a binder comes from source, callable-parameter
/// elaboration, or lifetime elision.
///
/// Each `def(...)` parameter annotation generates a fresh callable type and a
/// `core.Def` dictionary. Their origins pair those binders without using their
/// display names: generated binders are excluded from source-name lookup, and
/// generated dictionaries cannot be supplied as explicit `given` arguments.
///
/// Every generated callable variant carries the zero-based value-parameter
/// index within the owning declaration, counting ordinary parameters too (but
/// not an ellipsis). This is a declaration-local occurrence key, not a
/// polymorphic-variable ID or an index among only callable parameters. For `def
/// apply(x: int32, fn: def())`, the generated binders have origins
/// `CallableType(1)`, `CallableDictionary(1)` and `CallableDropDictionary(1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum PolyVarOrigin {
    /// A source-addressable type/effect variable or explicitly declared
    /// dictionary.
    Source,
    /// The fresh callable type for the value parameter at the given index.
    CallableType(usize),
    /// The hidden `core.Def` dictionary for that parameter's fresh callable
    /// type.
    CallableDictionary(usize),
    /// The hidden `core.Drop` dictionary for that parameter's fresh callable
    /// type, used when the callable is dropped instead of called.
    CallableDropDictionary(usize),
    /// The fresh lifetime parameter introduced for a lifetime elided in a
    /// parameter type. It is keyed by the span of the elided lifetime: the
    /// span of `&t` for a reference written without a lifetime, or the span
    /// of `'_`.
    ElidedLifetime(RelativeSpan),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct PolyVar {
    name: Interned<str>,
    origin: PolyVarOrigin,
    kind: PolyVarKind,
    span: RelativeSpan,

    /// The variance written on the parameter, as in `+t`, if any.
    declared_variance: Option<Variance>,
}

impl PolyVar {
    #[must_use]
    pub const fn new_type(name: Interned<str>, span: RelativeSpan) -> Self {
        Self {
            name,
            origin: PolyVarOrigin::Source,
            kind: PolyVarKind::Type(TyKind::Star),
            span,
            declared_variance: None,
        }
    }

    #[must_use]
    pub const fn new_effect(name: Interned<str>, span: RelativeSpan) -> Self {
        Self {
            name,
            origin: PolyVarOrigin::Source,
            kind: PolyVarKind::Type(TyKind::EffectRow),
            span,
            declared_variance: None,
        }
    }

    /// Creates a lifetime parameter. Its name includes the leading quote, as
    /// in `'a`, so lifetimes and types live in separate namespaces.
    #[must_use]
    pub const fn new_lifetime(name: Interned<str>, span: RelativeSpan) -> Self {
        Self {
            name,
            origin: PolyVarOrigin::Source,
            kind: PolyVarKind::Type(TyKind::Lifetime),
            span,
            declared_variance: None,
        }
    }

    #[must_use]
    pub const fn new_instance(
        name: Interned<str>,
        trait_ref: TraitRef,
        span: RelativeSpan,
    ) -> Self {
        Self {
            name,
            origin: PolyVarOrigin::Source,
            kind: PolyVarKind::Instance(trait_ref),
            span,
            declared_variance: None,
        }
    }

    /// Declares the variance of this parameter, as in `+t`. The declared
    /// variance replaces the inferred one, which must not exceed it.
    #[must_use]
    pub const fn with_declared_variance(mut self, variance: Variance) -> Self {
        self.declared_variance = Some(variance);
        self
    }

    /// Returns the variance written on the parameter, if any.
    #[must_use]
    pub const fn declared_variance(&self) -> Option<Variance> { self.declared_variance }

    #[must_use]
    pub const fn is_source(&self) -> bool { matches!(self.origin, PolyVarOrigin::Source) }

    #[must_use]
    pub const fn name(&self) -> &Interned<str> { &self.name }

    #[must_use]
    pub const fn kind(&self) -> TyKind { self.kind.ty_kind() }

    #[must_use]
    pub const fn trait_ref(&self) -> Option<&TraitRef> {
        match &self.kind {
            PolyVarKind::Type(_) => None,
            PolyVarKind::Instance(trait_ref) => Some(trait_ref),
        }
    }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }
}

pub type PolyVarID = ID<PolyVar>;
pub type GlobalPolyVarID = MemberID<PolyVarID>;

/// The polymorphic variables owned by a symbol, in semantic insertion order.
///
/// The order returned by [`Self::iter`] is significant: type/effect binders
/// precede dictionaries. Arguments and substitutions use this complete order,
/// including generated binders that cannot be addressed by source names.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default, Identifiable)]
pub struct PolyVarMap {
    poly_vars: OrderedArena<PolyVar>,
    poly_var_ids_by_name: FxHashMap<Interned<str>, PolyVarID>,
}

impl PolyVarMap {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    #[must_use]
    pub fn len(&self) -> usize { self.poly_vars.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.poly_vars.is_empty() }

    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (PolyVarID, &PolyVar)> {
        self.poly_vars.iter()
    }

    #[must_use]
    pub fn find_by_name(&self, name: &str) -> Option<PolyVarID> {
        self.poly_var_ids_by_name.get(name).copied()
    }

    #[must_use]
    pub fn name_of(&self, id: PolyVarID) -> &str {
        self.poly_vars.get(id).expect("polymorphic variable ID should be valid").name()
    }

    #[must_use]
    pub fn kind_of(&self, id: PolyVarID) -> TyKind {
        self.poly_vars.get(id).expect("polymorphic variable ID should be valid").kind()
    }

    #[must_use]
    pub fn trait_ref_of(&self, id: PolyVarID) -> Option<&TraitRef> {
        self.poly_vars.get(id).and_then(PolyVar::trait_ref)
    }

    /// Returns the type, effect and lifetime variables, which type arguments
    /// instantiate, in order. They precede the dictionaries.
    pub fn type_parameters(&self) -> impl Iterator<Item = (PolyVarID, &PolyVar)> {
        self.iter().take_while(|(_, poly_var)| poly_var.kind() != TyKind::Instance)
    }

    /// Returns the instances, which given arguments instantiate, in order.
    /// They follow the type parameters.
    pub fn instances(&self) -> impl Iterator<Item = (PolyVarID, &PolyVar)> {
        self.iter().skip_while(|(_, poly_var)| poly_var.kind() != TyKind::Instance)
    }

    /// Returns the kind of the type parameter at `index`; see
    /// [`Self::type_parameters`].
    #[must_use]
    pub fn type_parameter_kind(&self, index: usize) -> Option<TyKind> {
        self.type_parameters().nth(index).map(|(_, poly_var)| poly_var.kind())
    }

    /// Generated binders deliberately bypass the source-name index.
    pub fn insert_generated(&mut self, mut variable: PolyVar, origin: PolyVarOrigin) -> PolyVarID {
        assert!(!matches!(origin, PolyVarOrigin::Source));
        assert!(self.find_generated(&origin).is_none());
        variable.origin = origin;
        self.poly_vars.insert(variable)
    }

    #[must_use]
    pub fn find_generated(&self, origin: &PolyVarOrigin) -> Option<PolyVarID> {
        self.iter().find_map(|(id, variable)| (&variable.origin == origin).then_some(id))
    }

    #[allow(clippy::result_large_err)]
    pub fn insert(&mut self, poly_var: PolyVar) -> Result<PolyVarID, (PolyVar, PolyVarID)> {
        match self.poly_var_ids_by_name.entry(poly_var.name.clone()) {
            Entry::Occupied(en) => Err((poly_var, *en.get())),
            Entry::Vacant(vacant_entry) => {
                let id = self.poly_vars.insert(poly_var);
                vacant_entry.insert(id);
                Ok(id)
            }
        }
    }
}

/// Retrieves the polymorphic variables associated with a symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<PolyVarMap>)]
#[extend(by_val, name = get_poly_var_map)]
pub struct Key {
    pub symbol_id: GlobalSymbolID,
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable, Default)]
pub struct PolyVarStack {
    poly_var_maps: Vec<(GlobalSymbolID, Interned<PolyVarMap>)>,
}

impl PolyVarStack {
    #[must_use]
    pub const fn new() -> Self { Self { poly_var_maps: Vec::new() } }

    #[must_use]
    pub fn all_poly_var_len(&self) -> usize {
        self.poly_var_maps.iter().map(|(_, map)| map.len()).sum()
    }

    #[must_use]
    pub fn find_by_name(&self, name: &str) -> Option<GlobalPolyVarID> {
        for (symbol_id, poly_var_map) in &self.poly_var_maps {
            if let Some(poly_var_id) = poly_var_map.find_by_name(name) {
                return Some(GlobalPolyVarID::new(*symbol_id, poly_var_id));
            }
        }

        None
    }

    #[must_use]
    pub fn kind_of(&self, id: GlobalPolyVarID) -> Option<TyKind> {
        self.poly_var_maps.iter().find_map(|(symbol_id, poly_var_map)| {
            (*symbol_id == id.parent_id()).then(|| poly_var_map.kind_of(id.id()))
        })
    }

    #[must_use]
    pub fn span_of(&self, id: GlobalPolyVarID) -> Option<RelativeSpan> {
        self.poly_var_maps.iter().find_map(|(symbol_id, poly_var_map)| {
            (*symbol_id == id.parent_id()).then(|| poly_var_map[id.id()].span())
        })
    }

    #[must_use]
    pub fn trait_ref_of(&self, id: GlobalPolyVarID) -> Option<&TraitRef> {
        self.poly_var_maps.iter().find_map(|(symbol_id, poly_var_map)| {
            (*symbol_id == id.parent_id()).then(|| poly_var_map.trait_ref_of(id.id())).flatten()
        })
    }

    pub fn push(&mut self, symbol_id: GlobalSymbolID, poly_var_map: Interned<PolyVarMap>) {
        self.poly_var_maps.push((symbol_id, poly_var_map));
    }

    #[must_use]
    pub fn argument_parameters(
        &self,
        symbol_id: GlobalSymbolID,
    ) -> Option<Vec<(Interned<str>, TyKind)>> {
        self.poly_var_maps.iter().find_map(|(candidate, poly_var_map)| {
            (*candidate == symbol_id).then(|| {
                poly_var_map
                    .iter()
                    .map(|(_, poly_var)| (poly_var.name.clone(), poly_var.kind()))
                    .collect()
            })
        })
    }

    /// Returns every instances of the stack, with the trait reference its
    /// declaration requires the given argument to implement.
    pub fn instances(&self) -> impl Iterator<Item = (GlobalPolyVarID, &TraitRef)> {
        self.poly_var_maps.iter().flat_map(|(symbol_id, poly_var_map)| {
            poly_var_map.instances().filter_map(move |(poly_var_id, poly_var)| {
                Some((GlobalPolyVarID::new(*symbol_id, poly_var_id), poly_var.trait_ref()?))
            })
        })
    }

    pub fn all_poly_vars(&self) -> impl Iterator<Item = GlobalPolyVarID> {
        self.poly_var_maps.iter().flat_map(|(symbol_id, poly_var_map)| {
            poly_var_map
                .iter()
                .map(move |(poly_var_id, _)| GlobalPolyVarID::new(*symbol_id, poly_var_id))
        })
    }

    pub fn all_poly_vars_with_kind(&self) -> impl Iterator<Item = (GlobalPolyVarID, TyKind)> + '_ {
        self.poly_var_maps.iter().flat_map(|(symbol_id, poly_var_map)| {
            poly_var_map.iter().map(move |(poly_var_id, poly_var)| {
                (GlobalPolyVarID::new(*symbol_id, poly_var_id), poly_var.kind())
            })
        })
    }
}

/// Retrieves polymorphic-variable maps owned by a symbol and its enclosing
/// symbol hierarchy, ordered from the requested symbol outwards.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<PolyVarStack>)]
#[extend(by_val, name = get_enclosing_poly_var_maps)]
pub struct EnclosingMapsKey {
    pub symbol_id: GlobalSymbolID,
}

#[executor(config = Config)]
async fn enclosing_poly_var_maps_executor(
    &EnclosingMapsKey { symbol_id }: &EnclosingMapsKey,
    engine: &TrackedEngine,
) -> Interned<PolyVarStack> {
    let mut maps = Vec::new();
    let mut current_id = Some(symbol_id);

    while let Some(id) = current_id {
        if engine.get_symbol_kind(id).await.has_poly_var_map() {
            maps.push((id, engine.get_poly_var_map(id).await));
        }
        current_id = engine.get_parent_global(id).await;
    }

    engine.intern(PolyVarStack { poly_var_maps: maps })
}

#[distributed_slice(RAY_PROGRAM)]
static ENCLOSING_POLY_VAR_MAPS_EXECUTOR: Registration<Config> =
    Registration::new::<EnclosingMapsKey, EnclosingPolyVarMapsExecutor>();

#[extend]
pub async fn build_subst_from_args<'a>(
    self: &TrackedEngine,
    symbol_id: GlobalSymbolID,
    args: impl IntoIterator<Item = &'a Interned<Ty>>,
) -> Subst {
    let poly_vars = self.get_poly_var_map(symbol_id).await;

    poly_vars
        .iter()
        .zip(args)
        .map(|((poly_var_id, _), argument)| {
            (GlobalPolyVarID::new(symbol_id, poly_var_id), argument.clone())
        })
        .collect()
}

impl Index<PolyVarID> for PolyVarMap {
    type Output = PolyVar;

    fn index(&self, index: PolyVarID) -> &Self::Output {
        self.poly_vars.get(index).expect("polymorphic variable ID should be valid")
    }
}

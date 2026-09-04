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
    ty::{TyKind, args::Args},
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct PolyVar {
    name: Interned<str>,
    kind: PolyVarKind,
    span: RelativeSpan,
}

impl PolyVar {
    #[must_use]
    pub const fn new_type(name: Interned<str>, span: RelativeSpan) -> Self {
        Self { name, kind: PolyVarKind::Type(TyKind::Star), span }
    }

    #[must_use]
    pub const fn new_effect(name: Interned<str>, span: RelativeSpan) -> Self {
        Self { name, kind: PolyVarKind::Type(TyKind::EffectRow), span }
    }

    #[must_use]
    pub const fn new_instance(
        name: Interned<str>,
        trait_ref: TraitRef,
        span: RelativeSpan,
    ) -> Self {
        Self { name, kind: PolyVarKind::Instance(trait_ref), span }
    }

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
/// The order returned by [`Self::iter`] is significant. Builders of
/// corresponding declarations must insert alpha-equivalent variables in the
/// same order so that consumers can pair variables positionally.
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
pub async fn build_subst_from_args(
    self: &TrackedEngine,
    symbol_id: GlobalSymbolID,
    args: &Args,
) -> Subst {
    if args.is_empty() {
        return Subst::new_empty();
    }

    let poly_vars = self.get_poly_var_map(symbol_id).await;

    assert_eq!(
        args.len(),
        poly_vars.len(),
        "number of arguments must match number of polymorphic variables"
    );

    poly_vars
        .iter()
        .zip(args.interned_iter())
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

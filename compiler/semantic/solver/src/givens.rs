//! Cached where-clause predicates visible at a solver's declaration site.

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::{GlobalSymbolID, parent::scope_walker, symbol_kind::get_symbol_kind};
use rayc_type::where_clause::{PredicateKind, get_where_clause};

/// Collects predicates from the site outward, preserving declaration order
/// within each clause. Nearer scopes therefore take precedence during
/// reduction.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[PredicateKind]>)]
#[extend(by_val, name = get_givens)]
pub struct Key {
    pub site: GlobalSymbolID,
}

#[executor(config = Config)]
pub async fn givens_executor(
    &Key { site }: &Key,
    engine: &TrackedEngine,
) -> Interned<[PredicateKind]> {
    // Track the scope hierarchy and each applicable clause as query dependencies.
    let mut givens = Vec::new();
    let mut scopes = engine.scope_walker(site);
    while let Some(symbol_id) = scopes.next().await {
        let symbol_id = site.target_id.make_global(symbol_id);
        if engine.get_symbol_kind(symbol_id).await.has_where_clause() {
            let clause = engine.get_where_clause(symbol_id).await;
            givens.extend(clause.iter().map(|predicate| predicate.kind().clone()));
        }
    }
    engine.intern_unsized(givens)
}

#[distributed_slice(RAY_PROGRAM)]
static GIVENS_EXECUTOR: Registration<Config> = Registration::new::<Key, GivensExecutor>();

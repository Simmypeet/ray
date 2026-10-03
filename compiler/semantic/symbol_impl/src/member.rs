use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::member::{Key, Member};
use rayc_target::Global;

use crate::index::get_symbol_table;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash, Query,
)]
#[value(Option<Interned<Member>>)]
pub struct ProjectionKey {
    pub symbol_id: Global<rayc_symbol::SymbolID>,
}

#[executor(config = Config, style = qbice::ExecutionStyle::Projection)]
async fn projection_executor(
    key: &ProjectionKey,
    engine: &TrackedEngine,
) -> Option<Interned<Member>> {
    let id = key.symbol_id;
    let table = engine.get_symbol_table(id).await;

    table.member(id.id).cloned()
}

#[distributed_slice(RAY_PROGRAM)]
static PROJECTION_EXECUTOR: Registration<Config> =
    Registration::new::<ProjectionKey, ProjectionExecutor>();

#[executor(config = Config)]
async fn member_executor(key: &Key, engine: &TrackedEngine) -> Interned<Member> {
    engine.query(&ProjectionKey { symbol_id: key.symbol_id }).await.unwrap()
}

#[distributed_slice(RAY_PROGRAM)]
static MEMBER_EXECUTOR: Registration<Config> = Registration::new::<Key, MemberExecutor>();

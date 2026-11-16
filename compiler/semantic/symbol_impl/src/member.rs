use linkme::distributed_slice;
use qbice::{executor, program::Registration, storage::intern::Interned};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::{
    SymbolID,
    member::{Key, Member, MemberByNameKey},
};

use crate::{
    index::try_get_symbol_table,
    projection::{project, symbol_projection},
};

symbol_projection!(MemberProjection -> Interned<Member> = get_members);

#[executor(config = Config)]
async fn member_executor(&Key { symbol_id }: &Key, engine: &TrackedEngine) -> Interned<Member> {
    project(engine, MemberProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static MEMBER_EXECUTOR: Registration<Config> = Registration::new::<Key, MemberExecutor>();

/// A projection of the table storing a symbol for its member of one name, so
/// that a changed table reruns the lookup instead of dirtying every caller,
/// and dirties them only if the member of that name changed.
#[executor(config = Config, style = qbice::ExecutionStyle::Projection)]
async fn member_by_name_executor(
    MemberByNameKey { symbol_id, name }: &MemberByNameKey,
    engine: &TrackedEngine,
) -> Option<SymbolID> {
    engine.try_get_symbol_table(*symbol_id).await?.get_members(symbol_id.id)?.get_by_name(name)
}

#[distributed_slice(RAY_PROGRAM)]
static MEMBER_BY_NAME_EXECUTOR: Registration<Config> =
    Registration::new::<MemberByNameKey, MemberByNameExecutor>();

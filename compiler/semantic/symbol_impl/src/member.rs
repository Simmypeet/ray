use linkme::distributed_slice;
use qbice::{executor, program::Registration, storage::intern::Interned};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::member::{Key, Member};

use crate::projection::{project, symbol_projection};

symbol_projection!(MemberProjection -> Interned<Member> = get_members);

#[executor(config = Config)]
async fn member_executor(&Key { symbol_id }: &Key, engine: &TrackedEngine) -> Interned<Member> {
    project(engine, MemberProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static MEMBER_EXECUTOR: Registration<Config> = Registration::new::<Key, MemberExecutor>();

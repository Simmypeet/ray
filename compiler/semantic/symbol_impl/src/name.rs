use linkme::distributed_slice;
use qbice::{executor, program::Registration, storage::intern::Interned};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::name::Key;

use crate::projection::{project, symbol_projection};

symbol_projection!(NameProjection -> Interned<str> = get_name);

#[executor(config = Config)]
pub async fn name_executor(&Key { symbol_id }: &Key, engine: &TrackedEngine) -> Interned<str> {
    project(engine, NameProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static NAME_EXECUTOR: Registration<Config> = Registration::new::<Key, NameExecutor>();

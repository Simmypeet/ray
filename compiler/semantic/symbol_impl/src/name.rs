use linkme::distributed_slice;
use qbice::{executor, program::Registration, storage::intern::Interned};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::name::Key;

use crate::index::get_symbol_table;

#[executor(config = Config)]
pub async fn name_executor(&Key { symbol_id }: &Key, engine: &TrackedEngine) -> Interned<str> {
    engine.get_symbol_table(symbol_id).await.get_name(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static NAME_EXECUTOR: Registration<Config> = Registration::new::<Key, NameExecutor>();

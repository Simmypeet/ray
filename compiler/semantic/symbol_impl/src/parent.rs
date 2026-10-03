use linkme::distributed_slice;
use qbice::{executor, program::Registration};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::{SymbolID, parent::Key};

use crate::index::get_symbol_table;

#[executor(config = Config)]
pub async fn parent_executor(&Key { symbol_id }: &Key, engine: &TrackedEngine) -> Option<SymbolID> {
    engine.get_symbol_table(symbol_id).await.get_parent(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static PARENT_EXECUTOR: Registration<Config> = Registration::new::<Key, ParentExecutor>();

use linkme::distributed_slice;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::{SymbolID, parent::Key};
use qbice::{executor, program::Registration};

use crate::table::get_table;

#[executor(config = Config)]
pub async fn parent_executor(&Key { symbol_id }: &Key, engine: &TrackedEngine) -> Option<SymbolID> {
    engine.get_table(symbol_id.target_id).await.get_parent(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static PARENT_EXECUTOR: Registration<Config> = Registration::new::<Key, ParentExecutor>();

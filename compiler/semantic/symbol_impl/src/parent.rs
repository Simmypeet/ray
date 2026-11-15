use linkme::distributed_slice;
use qbice::{executor, program::Registration};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::{SymbolID, parent::Key};

use crate::projection::{project, symbol_projection};

symbol_projection!(ParentProjection -> Option<SymbolID> = get_arent);

#[executor(config = Config)]
pub async fn parent_executor(&Key { symbol_id }: &Key, engine: &TrackedEngine) -> Option<SymbolID> {
    project(engine, ParentProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static PARENT_EXECUTOR: Registration<Config> = Registration::new::<Key, ParentExecutor>();

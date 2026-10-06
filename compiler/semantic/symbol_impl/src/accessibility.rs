use linkme::distributed_slice;
use qbice::{executor, program::Registration};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::accessibility::{DeclaredAccessibility, Key};

use crate::index::get_symbol_table;

#[executor(config = Config)]
pub async fn declared_accessibility_executor(
    &Key { symbol_id }: &Key,
    engine: &TrackedEngine,
) -> DeclaredAccessibility {
    engine.get_symbol_table(symbol_id).await.get_declared_accessibility(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static DECLARED_ACCESSIBILITY_EXECUTOR: Registration<Config> =
    Registration::new::<Key, DeclaredAccessibilityExecutor>();

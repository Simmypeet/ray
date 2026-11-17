use linkme::distributed_slice;
use qbice::{executor, program::Registration};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::accessibility::{DeclaredAccessibility, Key};

use crate::projection::{project, symbol_projection};

symbol_projection!(DeclaredAccessibilityProjection -> DeclaredAccessibility = get_declared_accessibility);

#[executor(config = Config)]
pub async fn declared_accessibility_executor(
    &Key { symbol_id }: &Key,
    engine: &TrackedEngine,
) -> DeclaredAccessibility {
    project(engine, DeclaredAccessibilityProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static DECLARED_ACCESSIBILITY_EXECUTOR: Registration<Config> =
    Registration::new::<Key, DeclaredAccessibilityExecutor>();

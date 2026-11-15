//! The projections of the symbol tables, which the queries for the
//! information of a single symbol read.
//!
//! The table of a source file is a firewall, which changes with any edit to
//! its file, even one inside a function body. A query for the information of
//! a symbol (its name, kind, syntax, and so on) reads it through a
//! projection: when the table changes, the engine reruns every projection of
//! it, and only the queries whose projection changed recompute.
//!
//! The engine reruns a projection on its own, whatever the symbol, so a
//! projection must not panic, even for a symbol that the edit removed: it
//! gives `None` for a symbol in no table. The public query that reads it
//! unwraps it, since only a stale query could ask about such a symbol.

use std::fmt::Debug;

use qbice::Query;
use rayc_qbice::TrackedEngine;

/// Defines `$projection`, the projection of the table storing a symbol that
/// gives what the table's `$getter` gives for the symbol, or `None` if no
/// table stores it.
macro_rules! symbol_projection {
    ($projection:ident -> $value:ty = $getter:ident) => {
        paste::paste! {
            #[doc = concat!(
                "The projection of the table storing a symbol that gives what [`Table::",
                stringify!($getter),
                "`](crate::table::Table::",
                stringify!($getter),
                ") gives for it, or `None` if no table stores it.",
            )]
            #[derive(
                Debug,
                Clone,
                Copy,
                PartialEq,
                Eq,
                PartialOrd,
                Ord,
                Hash,
                qbice::StableHash,
                qbice::Encode,
                qbice::Decode,
                qbice::Query,
            )]
            #[value(Option<$value>)]
            struct $projection {
                symbol_id: rayc_symbol::GlobalSymbolID,
            }

            #[qbice::executor(
                config = rayc_qbice::Config,
                style = qbice::ExecutionStyle::Projection
            )]
            async fn [<$projection:snake _executor>](
                &$projection { symbol_id }: &$projection,
                engine: &rayc_qbice::TrackedEngine,
            ) -> Option<$value> {
                use $crate::index::try_get_symbol_table;

                engine.try_get_symbol_table(symbol_id).await?.$getter(symbol_id.id)
            }

            #[linkme::distributed_slice(rayc_qbice::RAY_PROGRAM)]
            static [<$projection:snake:upper _EXECUTOR>]: qbice::program::Registration<
                rayc_qbice::Config,
            > = qbice::program::Registration::new::<$projection, [<$projection Executor>]>();
        }
    };
}

pub(crate) use symbol_projection;

/// Reads the projection `key` of a symbol, which a table must store.
pub(crate) async fn project<Q, T>(engine: &TrackedEngine, key: Q) -> T
where
    Q: Query<Value = Option<T>> + Debug,
{
    engine.query(&key).await.unwrap_or_else(|| panic!("{key:?} names a symbol in no table"))
}

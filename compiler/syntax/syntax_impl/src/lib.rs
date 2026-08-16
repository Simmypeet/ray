//! Implements the syntax parsing executor.

use linkme::distributed_slice;
use rayc_parser::abstract_tree::AbstractTree;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_syntax::{DiagnosticKey, Key, module::ModuleContent};
use qbice::{executor, program::Registration, storage::intern::Interned};

#[executor(config = Config)]
#[allow(clippy::type_complexity)]
async fn parse_executor(
    key: &Key,
    engine: &TrackedEngine,
) -> Result<
    (Option<ModuleContent>, Interned<[rayc_parser::error::Error]>),
    rayc_source_file::Error,
> {
    // load the token tree
    let token_tree = engine
        .query(&rayc_lexical::Key { path: key.path.clone(), target_id: key.target_id })
        .await?;

    let (module, errors) = ModuleContent::parse(&token_tree.0, engine);

    Ok((module, engine.intern_unsized(errors)))
}

#[distributed_slice(RAY_PROGRAM)]
static PARSE_EXECUTOR: Registration<Config> = Registration::new::<Key, ParseExecutor>();

#[executor(config = Config)]
#[allow(clippy::type_complexity)]
async fn diagnostic_executor(
    DiagnosticKey(key): &DiagnosticKey,
    engine: &TrackedEngine,
) -> Result<Interned<[rayc_parser::error::Error]>, rayc_source_file::Error> {
    let token_tree = engine.query(key).await?;

    Ok(token_tree.1)
}

#[distributed_slice(RAY_PROGRAM)]
static DIAGNOSTIC_EXECUTOR: Registration<Config> =
    Registration::new::<DiagnosticKey, DiagnosticExecutor>();

/// A dummy function to make sure this crate is linked by the compiler.
pub const fn black_box() {}

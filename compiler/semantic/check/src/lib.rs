//! Crate for collecting all semantic analysis checks.

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_diagnostic::Rendered;
use rayc_qbice::{
    Config, RAY_PROGRAM, TrackedEngine,
    unordered::{UnorderedCalleeGroup, query_in_task},
};

/// The main data structure collecting all diagnostics for semantic analysis
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Check {
    symbol_immpl: Interned<[Rendered<usize>]>,
    semantic_impl: Interned<[Interned<[Rendered<usize>]>]>,
    typed_ast_impl: Interned<[Interned<[Rendered<usize>]>]>,
    ir_impl: Interned<[Interned<[Rendered<usize>]>]>,
}

impl Check {
    /// Returns an iterator over all diagnostics in this check.
    pub fn all_diagnostics(&self) -> impl Iterator<Item = &Rendered<usize>> {
        self.symbol_immpl
            .iter()
            .chain(self.semantic_impl.iter().flat_map(|diags| diags.iter()))
            .chain(self.typed_ast_impl.iter().flat_map(|diags| diags.iter()))
            .chain(self.ir_impl.iter().flat_map(|diags| diags.iter()))
    }
}

/// The main query for checking all semantic errors.
///
/// This represents the `rayc check` command.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Check)]
pub struct Key {
    /// The target ID for which to check semantic errors.
    pub target_id: rayc_target::TargetID,
}

#[executor(config = Config)]
async fn check_executor(&Key { target_id }: &Key, engine: &TrackedEngine) -> Check {
    // collects the diagnostics of every phase in parallel, one task per phase
    let (sym_diags, semantic_diags, typed_ast_diags, ir_diags) = {
        // SAFETY: the diagnostics of every phase are always collected,
        // whatever the diagnostics of the other phases are
        let _group = unsafe { UnorderedCalleeGroup::start(engine) };

        tokio::join!(
            engine.query_in_task(rayc_symbol_impl::diagnostic::RenderedKey(target_id)),
            engine.query_in_task(rayc_semantic_element_impl::diagnostic::RenderedKey { target_id }),
            engine.query_in_task(rayc_typed_ast_builder::query::RenderedKey { target_id }),
            engine.query_in_task(rayc_ir_builder::query::RenderedKey { target_id }),
        )
    };

    Check {
        symbol_immpl: sym_diags,
        semantic_impl: semantic_diags,
        typed_ast_impl: typed_ast_diags,
        ir_impl: ir_diags,
    }
}

#[distributed_slice(RAY_PROGRAM)]
static CHECK_EXECUTOR: Registration<Config> = Registration::new::<Key, CheckExecutor>();

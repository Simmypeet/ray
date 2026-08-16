use linkme::distributed_slice;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::syntax::{DefBodySyntaxKey, DefSignatureSyntaxKey};
use rayc_syntax::{
    def::{ParameterList, ReturnType},
    statement::Block,
};
use qbice::{executor, program::Registration};

use crate::table::get_table;

#[executor(config = Config)]
pub async fn def_signature_syntax_executor(
    &DefSignatureSyntaxKey { symbol_id }: &DefSignatureSyntaxKey,
    engine: &TrackedEngine,
) -> (Option<ParameterList>, Option<ReturnType>) {
    let table = engine.get_table(symbol_id.target_id).await;

    table.get_def_signature_syntax(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static DEF_SIGNATURE_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<DefSignatureSyntaxKey, DefSignatureSyntaxExecutor>();

#[executor(config = Config)]
pub async fn def_body_syntax_executor(
    &DefBodySyntaxKey { symbol_id }: &DefBodySyntaxKey,
    engine: &TrackedEngine,
) -> Option<Block> {
    let table = engine.get_table(symbol_id.target_id).await;

    table.get_def_body_syntax(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static DEF_BODY_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<DefBodySyntaxKey, DefBodySyntaxExecutor>();

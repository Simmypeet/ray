use linkme::distributed_slice;
use qbice::{executor, program::Registration};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::syntax::{
    DefBodySyntaxKey, EffectTypeParameterSyntaxKey, ParameterListSyntaxKey, ReturnTypeSyntaxKey,
    VariadicDefKey,
};
use rayc_syntax::{
    def::{ParameterList, ReturnType},
    effect::TypeParameterList,
    statement::Block,
};

use crate::table::get_table;

#[executor(config = Config)]
pub async fn parameter_list_syntax_executor(
    &ParameterListSyntaxKey { symbol_id }: &ParameterListSyntaxKey,
    engine: &TrackedEngine,
) -> Option<ParameterList> {
    let table = engine.get_table(symbol_id.target_id).await;

    table.get_parameter_list_syntax(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static PARAMETER_LIST_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<ParameterListSyntaxKey, ParameterListSyntaxExecutor>();

#[executor(config = Config)]
pub async fn return_type_syntax_executor(
    &ReturnTypeSyntaxKey { symbol_id }: &ReturnTypeSyntaxKey,
    engine: &TrackedEngine,
) -> Option<ReturnType> {
    let table = engine.get_table(symbol_id.target_id).await;

    table.get_return_type_syntax(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static RETURN_TYPE_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<ReturnTypeSyntaxKey, ReturnTypeSyntaxExecutor>();

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

#[executor(config = Config)]
pub async fn variadic_def_executor(
    &VariadicDefKey { symbol_id }: &VariadicDefKey,
    engine: &TrackedEngine,
) -> bool {
    engine.get_table(symbol_id.target_id).await.is_variadic_def(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static VARIADIC_DEF_EXECUTOR: Registration<Config> =
    Registration::new::<VariadicDefKey, VariadicDefExecutor>();

#[executor(config = Config)]
pub async fn effect_type_parameter_syntax_executor(
    &EffectTypeParameterSyntaxKey { symbol_id }: &EffectTypeParameterSyntaxKey,
    engine: &TrackedEngine,
) -> Option<TypeParameterList> {
    engine.get_table(symbol_id.target_id).await.get_effect_type_parameter_syntax(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static EFFECT_TYPE_PARAMETER_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<EffectTypeParameterSyntaxKey, EffectTypeParameterSyntaxExecutor>();

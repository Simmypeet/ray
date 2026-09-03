use linkme::distributed_slice;
use qbice::{executor, program::Registration};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::syntax::{
    DefBodySyntaxKey, EffectRowSyntaxKey, InstanceTraitSyntaxKey, ParameterListSyntaxKey,
    ReturnTypeSyntaxKey, TypeParameterListSyntaxKey, VariadicDefKey,
};
use rayc_syntax::{
    def::{ParameterList, ReturnType},
    effect::TypeParameterList,
    effect_row::EffectRowAnnotation,
    path::Path,
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
pub async fn effect_row_syntax_executor(
    &EffectRowSyntaxKey { symbol_id }: &EffectRowSyntaxKey,
    engine: &TrackedEngine,
) -> Option<EffectRowAnnotation> {
    let table = engine.get_table(symbol_id.target_id).await;

    table.get_effect_row_syntax(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static EFFECT_ROW_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<EffectRowSyntaxKey, EffectRowSyntaxExecutor>();

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
pub async fn type_parameter_list_syntax_executor(
    &TypeParameterListSyntaxKey { symbol_id }: &TypeParameterListSyntaxKey,
    engine: &TrackedEngine,
) -> Option<TypeParameterList> {
    engine.get_table(symbol_id.target_id).await.get_type_parameter_list_syntax(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static TYPE_PARAMETER_LIST_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<TypeParameterListSyntaxKey, TypeParameterListSyntaxExecutor>();

#[executor(config = Config)]
pub async fn instance_trait_syntax_executor(
    &InstanceTraitSyntaxKey { symbol_id }: &InstanceTraitSyntaxKey,
    engine: &TrackedEngine,
) -> Option<Path> {
    engine.get_table(symbol_id.target_id).await.get_instance_trait_syntax(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static INSTANCE_TRAIT_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<InstanceTraitSyntaxKey, InstanceTraitSyntaxExecutor>();

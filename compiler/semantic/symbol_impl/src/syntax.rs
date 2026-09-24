use linkme::distributed_slice;
use qbice::{executor, program::Registration};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::syntax::{
    DefBodySyntaxKey, EffectRowSyntaxKey, GivenParameterListSyntaxKey, InstanceTraitSyntaxKey,
    KindAscriptionSyntaxKey, LinearStructKey, MarkerImplementationMarkerSyntaxKey,
    MarkerImplementationTypeSyntaxKey, NegativeMarkerImplementationSyntaxKey,
    ParameterListSyntaxKey, ReturnTypeSyntaxKey, StructBodySyntaxKey, TypeDefinitionSyntaxKey,
    TypeParameterListSyntaxKey, VariadicDefKey, WhereClauseSyntaxKey,
};
use rayc_syntax::{
    def::{ParameterList, ReturnType},
    effect::TypeParameterList,
    effect_row::EffectRowAnnotation,
    given::GivenParameterList,
    kind::KindAscription,
    path::Path,
    statement::Block,
    r#struct::StructBody,
    r#type::Type,
    where_clause::WhereClause,
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
pub async fn struct_body_syntax_executor(
    &StructBodySyntaxKey { symbol_id }: &StructBodySyntaxKey,
    engine: &TrackedEngine,
) -> Option<StructBody> {
    engine.get_table(symbol_id.target_id).await.get_struct_body_syntax(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static STRUCT_BODY_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<StructBodySyntaxKey, StructBodySyntaxExecutor>();

#[executor(config = Config)]
pub async fn linear_struct_executor(
    &LinearStructKey { symbol_id }: &LinearStructKey,
    engine: &TrackedEngine,
) -> bool {
    engine.get_table(symbol_id.target_id).await.is_linear_struct(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static LINEAR_STRUCT_EXECUTOR: Registration<Config> =
    Registration::new::<LinearStructKey, LinearStructExecutor>();

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
pub async fn given_parameter_list_syntax_executor(
    &GivenParameterListSyntaxKey { symbol_id }: &GivenParameterListSyntaxKey,
    engine: &TrackedEngine,
) -> Option<GivenParameterList> {
    engine.get_table(symbol_id.target_id).await.get_given_parameter_list_syntax(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static GIVEN_PARAMETER_LIST_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<GivenParameterListSyntaxKey, GivenParameterListSyntaxExecutor>();

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

#[executor(config = Config)]
pub async fn marker_implementation_marker_syntax_executor(
    &MarkerImplementationMarkerSyntaxKey { symbol_id }: &MarkerImplementationMarkerSyntaxKey,
    engine: &TrackedEngine,
) -> Option<Path> {
    engine
        .get_table(symbol_id.target_id)
        .await
        .get_marker_implementation_marker_syntax(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static MARKER_IMPLEMENTATION_MARKER_SYNTAX_EXECUTOR: Registration<Config> = Registration::new::<
    MarkerImplementationMarkerSyntaxKey,
    MarkerImplementationMarkerSyntaxExecutor,
>();

#[executor(config = Config)]
pub async fn negative_marker_implementation_syntax_executor(
    &NegativeMarkerImplementationSyntaxKey { symbol_id }: &NegativeMarkerImplementationSyntaxKey,
    engine: &TrackedEngine,
) -> bool {
    engine.get_table(symbol_id.target_id).await.is_negative_marker_implementation(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static NEGATIVE_MARKER_IMPLEMENTATION_SYNTAX_EXECUTOR: Registration<Config> = Registration::new::<
    NegativeMarkerImplementationSyntaxKey,
    NegativeMarkerImplementationSyntaxExecutor,
>();

#[executor(config = Config)]
pub async fn marker_implementation_type_syntax_executor(
    &MarkerImplementationTypeSyntaxKey { symbol_id }: &MarkerImplementationTypeSyntaxKey,
    engine: &TrackedEngine,
) -> Option<Type> {
    engine.get_table(symbol_id.target_id).await.get_marker_implementation_type_syntax(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static MARKER_IMPLEMENTATION_TYPE_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<MarkerImplementationTypeSyntaxKey, MarkerImplementationTypeSyntaxExecutor>(
    );

#[executor(config = Config)]
pub async fn type_definition_syntax_executor(
    &TypeDefinitionSyntaxKey { symbol_id }: &TypeDefinitionSyntaxKey,
    engine: &TrackedEngine,
) -> Option<Type> {
    engine.get_table(symbol_id.target_id).await.get_type_definition_syntax(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static TYPE_DEFINITION_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<TypeDefinitionSyntaxKey, TypeDefinitionSyntaxExecutor>();

#[executor(config = Config)]
pub async fn where_clause_syntax_executor(
    &WhereClauseSyntaxKey { symbol_id }: &WhereClauseSyntaxKey,
    engine: &TrackedEngine,
) -> Option<WhereClause> {
    engine.get_table(symbol_id.target_id).await.get_where_clause_syntax(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static WHERE_CLAUSE_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<WhereClauseSyntaxKey, WhereClauseSyntaxExecutor>();

#[executor(config = Config)]
pub async fn kind_ascription_syntax_executor(
    &KindAscriptionSyntaxKey { symbol_id }: &KindAscriptionSyntaxKey,
    engine: &TrackedEngine,
) -> Option<KindAscription> {
    engine.get_table(symbol_id.target_id).await.get_kind_ascription_syntax(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static KIND_ASCRIPTION_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<KindAscriptionSyntaxKey, KindAscriptionSyntaxExecutor>();

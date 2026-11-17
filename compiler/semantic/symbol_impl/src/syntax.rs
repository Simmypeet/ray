use linkme::distributed_slice;
use qbice::{executor, program::Registration, storage::intern::Interned};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::syntax::{
    DefBodySyntaxKey, EffectRowSyntaxKey, GivenParameterListSyntaxKey, ImportsSyntaxKey,
    InstanceTraitSyntaxKey, KindAscriptionSyntaxKey, LinearTypeKey,
    MarkerImplementationMarkerSyntaxKey, MarkerImplementationTypeSyntaxKey,
    NegativeMarkerImplementationSyntaxKey, ParameterListSyntaxKey, ReturnTypeSyntaxKey,
    StructBodySyntaxKey, TypeDefinitionSyntaxKey, TypeParameterListSyntaxKey, VariadicDefKey,
    VariantPayloadSyntaxKey, WhereClauseSyntaxKey,
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

use crate::projection::{project, symbol_projection};

symbol_projection!(ParameterListSyntaxProjection -> Option<ParameterList> = get_parameter_list_syntax);

#[executor(config = Config)]
pub async fn parameter_list_syntax_executor(
    &ParameterListSyntaxKey { symbol_id }: &ParameterListSyntaxKey,
    engine: &TrackedEngine,
) -> Option<ParameterList> {
    project(engine, ParameterListSyntaxProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static PARAMETER_LIST_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<ParameterListSyntaxKey, ParameterListSyntaxExecutor>();

symbol_projection!(ReturnTypeSyntaxProjection -> Option<ReturnType> = get_return_type_syntax);

#[executor(config = Config)]
pub async fn return_type_syntax_executor(
    &ReturnTypeSyntaxKey { symbol_id }: &ReturnTypeSyntaxKey,
    engine: &TrackedEngine,
) -> Option<ReturnType> {
    project(engine, ReturnTypeSyntaxProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static RETURN_TYPE_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<ReturnTypeSyntaxKey, ReturnTypeSyntaxExecutor>();

symbol_projection!(EffectRowSyntaxProjection -> Option<EffectRowAnnotation> = get_effect_row_syntax);

#[executor(config = Config)]
pub async fn effect_row_syntax_executor(
    &EffectRowSyntaxKey { symbol_id }: &EffectRowSyntaxKey,
    engine: &TrackedEngine,
) -> Option<EffectRowAnnotation> {
    project(engine, EffectRowSyntaxProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static EFFECT_ROW_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<EffectRowSyntaxKey, EffectRowSyntaxExecutor>();

symbol_projection!(DefBodySyntaxProjection -> Option<Block> = get_def_body_syntax);

#[executor(config = Config)]
pub async fn def_body_syntax_executor(
    &DefBodySyntaxKey { symbol_id }: &DefBodySyntaxKey,
    engine: &TrackedEngine,
) -> Option<Block> {
    project(engine, DefBodySyntaxProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static DEF_BODY_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<DefBodySyntaxKey, DefBodySyntaxExecutor>();

symbol_projection!(StructBodySyntaxProjection -> Option<StructBody> = get_struct_body_syntax);

#[executor(config = Config)]
pub async fn struct_body_syntax_executor(
    &StructBodySyntaxKey { symbol_id }: &StructBodySyntaxKey,
    engine: &TrackedEngine,
) -> Option<StructBody> {
    project(engine, StructBodySyntaxProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static STRUCT_BODY_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<StructBodySyntaxKey, StructBodySyntaxExecutor>();

symbol_projection!(LinearTypeProjection -> bool = is_linear_type);

#[executor(config = Config)]
pub async fn linear_type_executor(
    &LinearTypeKey { symbol_id }: &LinearTypeKey,
    engine: &TrackedEngine,
) -> bool {
    project(engine, LinearTypeProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static LINEAR_TYPE_EXECUTOR: Registration<Config> =
    Registration::new::<LinearTypeKey, LinearTypeExecutor>();

symbol_projection!(VariantPayloadSyntaxProjection -> Option<Type> = get_variant_payload_syntax);

#[executor(config = Config)]
pub async fn variant_payload_syntax_executor(
    &VariantPayloadSyntaxKey { symbol_id }: &VariantPayloadSyntaxKey,
    engine: &TrackedEngine,
) -> Option<Type> {
    project(engine, VariantPayloadSyntaxProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static VARIANT_PAYLOAD_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<VariantPayloadSyntaxKey, VariantPayloadSyntaxExecutor>();

symbol_projection!(VariadicDefProjection -> bool = is_variadic_def);

#[executor(config = Config)]
pub async fn variadic_def_executor(
    &VariadicDefKey { symbol_id }: &VariadicDefKey,
    engine: &TrackedEngine,
) -> bool {
    project(engine, VariadicDefProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static VARIADIC_DEF_EXECUTOR: Registration<Config> =
    Registration::new::<VariadicDefKey, VariadicDefExecutor>();

symbol_projection!(TypeParameterListSyntaxProjection -> Option<TypeParameterList> = get_type_parameter_list_syntax);

#[executor(config = Config)]
pub async fn type_parameter_list_syntax_executor(
    &TypeParameterListSyntaxKey { symbol_id }: &TypeParameterListSyntaxKey,
    engine: &TrackedEngine,
) -> Option<TypeParameterList> {
    project(engine, TypeParameterListSyntaxProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static TYPE_PARAMETER_LIST_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<TypeParameterListSyntaxKey, TypeParameterListSyntaxExecutor>();

symbol_projection!(GivenParameterListSyntaxProjection -> Option<GivenParameterList> = get_given_parameter_list_syntax);

#[executor(config = Config)]
pub async fn given_parameter_list_syntax_executor(
    &GivenParameterListSyntaxKey { symbol_id }: &GivenParameterListSyntaxKey,
    engine: &TrackedEngine,
) -> Option<GivenParameterList> {
    project(engine, GivenParameterListSyntaxProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static GIVEN_PARAMETER_LIST_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<GivenParameterListSyntaxKey, GivenParameterListSyntaxExecutor>();

symbol_projection!(InstanceTraitSyntaxProjection -> Option<Path> = get_instance_trait_syntax);

#[executor(config = Config)]
pub async fn instance_trait_syntax_executor(
    &InstanceTraitSyntaxKey { symbol_id }: &InstanceTraitSyntaxKey,
    engine: &TrackedEngine,
) -> Option<Path> {
    project(engine, InstanceTraitSyntaxProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static INSTANCE_TRAIT_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<InstanceTraitSyntaxKey, InstanceTraitSyntaxExecutor>();

symbol_projection!(MarkerImplementationMarkerSyntaxProjection -> Option<Path> = get_marker_implementation_marker_syntax);

#[executor(config = Config)]
pub async fn marker_implementation_marker_syntax_executor(
    &MarkerImplementationMarkerSyntaxKey { symbol_id }: &MarkerImplementationMarkerSyntaxKey,
    engine: &TrackedEngine,
) -> Option<Path> {
    project(engine, MarkerImplementationMarkerSyntaxProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static MARKER_IMPLEMENTATION_MARKER_SYNTAX_EXECUTOR: Registration<Config> = Registration::new::<
    MarkerImplementationMarkerSyntaxKey,
    MarkerImplementationMarkerSyntaxExecutor,
>();

symbol_projection!(NegativeMarkerImplementationSyntaxProjection -> bool = is_negative_marker_implementation);

#[executor(config = Config)]
pub async fn negative_marker_implementation_syntax_executor(
    &NegativeMarkerImplementationSyntaxKey { symbol_id }: &NegativeMarkerImplementationSyntaxKey,
    engine: &TrackedEngine,
) -> bool {
    project(engine, NegativeMarkerImplementationSyntaxProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static NEGATIVE_MARKER_IMPLEMENTATION_SYNTAX_EXECUTOR: Registration<Config> = Registration::new::<
    NegativeMarkerImplementationSyntaxKey,
    NegativeMarkerImplementationSyntaxExecutor,
>();

symbol_projection!(MarkerImplementationTypeSyntaxProjection -> Option<Type> = get_marker_implementation_type_syntax);

#[executor(config = Config)]
pub async fn marker_implementation_type_syntax_executor(
    &MarkerImplementationTypeSyntaxKey { symbol_id }: &MarkerImplementationTypeSyntaxKey,
    engine: &TrackedEngine,
) -> Option<Type> {
    project(engine, MarkerImplementationTypeSyntaxProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static MARKER_IMPLEMENTATION_TYPE_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<MarkerImplementationTypeSyntaxKey, MarkerImplementationTypeSyntaxExecutor>(
    );

symbol_projection!(TypeDefinitionSyntaxProjection -> Option<Type> = get_type_definition_syntax);

#[executor(config = Config)]
pub async fn type_definition_syntax_executor(
    &TypeDefinitionSyntaxKey { symbol_id }: &TypeDefinitionSyntaxKey,
    engine: &TrackedEngine,
) -> Option<Type> {
    project(engine, TypeDefinitionSyntaxProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static TYPE_DEFINITION_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<TypeDefinitionSyntaxKey, TypeDefinitionSyntaxExecutor>();

symbol_projection!(WhereClauseSyntaxProjection -> Option<WhereClause> = get_where_clause_syntax);

#[executor(config = Config)]
pub async fn where_clause_syntax_executor(
    &WhereClauseSyntaxKey { symbol_id }: &WhereClauseSyntaxKey,
    engine: &TrackedEngine,
) -> Option<WhereClause> {
    project(engine, WhereClauseSyntaxProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static WHERE_CLAUSE_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<WhereClauseSyntaxKey, WhereClauseSyntaxExecutor>();

symbol_projection!(KindAscriptionSyntaxProjection -> Option<KindAscription> = get_kind_ascription_syntax);

#[executor(config = Config)]
pub async fn kind_ascription_syntax_executor(
    &KindAscriptionSyntaxKey { symbol_id }: &KindAscriptionSyntaxKey,
    engine: &TrackedEngine,
) -> Option<KindAscription> {
    project(engine, KindAscriptionSyntaxProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static KIND_ASCRIPTION_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<KindAscriptionSyntaxKey, KindAscriptionSyntaxExecutor>();

symbol_projection!(ImportsSyntaxProjection -> Interned<[rayc_syntax::import::Import]> = get_imports_syntax);

#[executor(config = Config)]
pub async fn imports_syntax_executor(
    &ImportsSyntaxKey { module_id }: &ImportsSyntaxKey,
    engine: &TrackedEngine,
) -> Interned<[rayc_syntax::import::Import]> {
    project(engine, ImportsSyntaxProjection { symbol_id: module_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static IMPORTS_SYNTAX_EXECUTOR: Registration<Config> =
    Registration::new::<ImportsSyntaxKey, ImportsSyntaxExecutor>();

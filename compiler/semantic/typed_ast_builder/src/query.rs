use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_source_file::SourceElement;
use rayc_symbol::{GlobalSymbolID, span::get_span, symbol_kind::get_all_def_with_body_ids};
use rayc_target::TargetID;
use rayc_typed_ast::{TypedAst, name_binding::Source, typed_function::TypedFunctionLocalID};

use crate::{diagnostic::Diagnostic, tast_builder::TAstBuilder};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash, Query,
)]
#[value((Interned<TypedAst>, Interned<[Diagnostic]>))]
pub struct BuildTAst {
    pub def_id: GlobalSymbolID,
}

#[executor(config = Config)]
pub async fn build_tast_executor(
    &BuildTAst { def_id }: &BuildTAst,
    engine: &TrackedEngine,
) -> (Interned<TypedAst>, Interned<[Diagnostic]>) {
    let mut tast_builder = TAstBuilder::new(engine.clone(), def_id).await;

    tast_builder.build_parameter_pattern().await;
    tast_builder.build_body().await;

    let (func, diags) = tast_builder.finish().await;

    (engine.intern(func), engine.intern_unsized(diags))
}

impl TAstBuilder {
    async fn build_parameter_pattern(&mut self) {
        let parameter_name_binding_group = self.parameter_name_binding_group();
        let parameter_map = self.parameter_map_of_current_function().await;

        if let Some(parameter_syn) = self.parameter_list_syntax_of_current_function().await {
            for ((param_id, parameter), syn) in
                parameter_map.iter().zip(parameter_syn.entries().filter_map(|entry| match entry {
                    rayc_syntax::def::ParameterEntry::Parameter(parameter) => Some(parameter),
                    rayc_syntax::def::ParameterEntry::Ellipsis(_) => None,
                }))
            {
                let Some(pat) = syn.irrefutable_pattern() else {
                    continue;
                };

                self.insert_name_binding_to_group_from_pattern(
                    parameter_name_binding_group,
                    &pat,
                    parameter.ty(),
                    Source::Parameter(TypedFunctionLocalID::new(
                        self.current_typed_function_id(),
                        param_id,
                    )),
                );
            }
        }
    }

    async fn build_body(&mut self) {
        let Some(body_syn) = self.def_body_syntax_of_current_function().await else {
            return;
        };
        let body_span = body_syn.span();

        // The parser drops the tokens it cannot parse, e.g. a malformed `let`
        // annotation, which may leave types undetermined.
        if body_syn.inner_tree().contains_error() {
            self.taint_by_syntax_error();
        }

        for stmt in body_syn.statements() {
            self.bind_statement(&stmt).await;
        }

        let function_name_span =
            self.engine().get_span(self.current_def_id()).await.unwrap_or(body_span);
        self.push_function_effect_constraint(function_name_span).await;
    }
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_TAST_EXECUTOR: Registration<Config> =
    Registration::new::<BuildTAst, BuildTastExecutor>();

#[executor(config = Config)]
async fn typed_ast_executor(
    &rayc_typed_ast::Key { def_id }: &rayc_typed_ast::Key,
    engine: &TrackedEngine,
) -> Interned<TypedAst> {
    engine.query(&BuildTAst { def_id }).await.0
}

#[distributed_slice(RAY_PROGRAM)]
static TYPED_AST_EXECUTOR: Registration<Config> =
    Registration::new::<rayc_typed_ast::Key, TypedAstExecutor>();

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash, Query,
)]
#[value(Interned<[Rendered<ByteIndex>]>)]
pub struct SingleRenderedKey {
    pub def_id: GlobalSymbolID,
}

#[executor(config = Config)]
pub async fn single_rendered_executor(
    &SingleRenderedKey { def_id }: &SingleRenderedKey,
    engine: &TrackedEngine,
) -> Interned<[Rendered<ByteIndex>]> {
    let (_, diags) = engine.query(&BuildTAst { def_id }).await;
    let mut rendered = Vec::new();

    for diag in diags.iter() {
        rendered.push(diag.report(engine).await);
    }

    engine.intern_unsized(rendered)
}

#[distributed_slice(RAY_PROGRAM)]
static SINGLE_RENDERED_EXECUTOR: Registration<Config> =
    Registration::new::<SingleRenderedKey, SingleRenderedExecutor>();

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash, Query,
)]
#[value(Interned<[Interned<[Rendered<ByteIndex>]>]>)]
pub struct RenderedKey {
    pub target_id: TargetID,
}

#[executor(config = Config)]
pub async fn rendered_executor(
    &RenderedKey { target_id }: &RenderedKey,
    engine: &TrackedEngine,
) -> Interned<[Interned<[Rendered<ByteIndex>]>]> {
    let mut rendered = Vec::new();
    let def_ids = engine.get_all_def_with_body_ids(target_id).await;

    for def_id in def_ids.iter().copied() {
        rendered
            .push(engine.query(&SingleRenderedKey { def_id: target_id.make_global(def_id) }).await);
    }

    engine.intern_unsized(rendered)
}

#[distributed_slice(RAY_PROGRAM)]
static RENDERED_EXECUTOR: Registration<Config> =
    Registration::new::<RenderedKey, RenderedExecutor>();

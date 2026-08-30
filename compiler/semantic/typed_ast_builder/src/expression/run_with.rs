use rayc_hash::FxHashMap;
use rayc_semantic_element::{parameter::get_parameter_map, return_type::get_return_type};
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    member::get_members,
    name::get_name,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_syntax::expression::RunWith as RunWithSyntax;
use rayc_type::{subst::Substitutable, ty::Ty};
use rayc_typed_ast::{
    name_binding::Source,
    typed_expr::{TypedExprID, TypedExprKind, errored::Errored},
    typed_function::{TypedFunctionID, TypedFunctionLocalID},
    typed_operation_handler::TypedOperationHandlerParameter,
};

use crate::{
    bind::Bind,
    diagnostic::{
        Diagnostic, DuplicateEffectOperationHandler, EffectHandlerNotSupported,
        ExtraneousEffectOperationHandler, MismatchedEffectOperationHandlerParameterCount,
        MissingEffectOperationHandler,
    },
    tast_builder::TAstBuilder,
};

impl Bind<RunWithSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: RunWithSyntax) -> TypedExprID {
        let unit = Ty::new_unit(self.engine());
        let Some(effect) = syn.effect() else {
            return self.insert_expression(Errored::new_empty(), syn.span(), unit);
        };
        let Ok(effect) = self.resolve_effect_path(&effect).await else {
            return self.insert_expression(Errored::new_empty(), syn.span(), unit);
        };
        let effect_id = effect.symbol_id();
        let effect_substitution = effect.substitution(self.engine()).await;

        let operations = self.effect_operations(effect_id).await;
        let (seen_handlers, operation_handlers) =
            self.bind_operation_handlers(&syn, effect_id, &operations, &effect_substitution).await;

        let mut missing_operations = operations
            .iter()
            .filter(|(name, _)| !seen_handlers.contains_key(*name))
            .collect::<Vec<_>>();
        missing_operations.sort_by(|(left, _), (right, _)| left.as_ref().cmp(right.as_ref()));
        if !missing_operations.is_empty() {
            self.push_diagnostic(Diagnostic::MissingEffectOperationHandler(
                MissingEffectOperationHandler::builder()
                    .operations(
                        missing_operations.into_iter().map(|(_, operation)| *operation).collect(),
                    )
                    .span(syn.effect().map_or_else(|| syn.span(), |effect| effect.span()))
                    .build(),
            ));
        }

        let (body_function, return_type) = self.start_thunk();
        let mut has_explicit_return = false;
        if let Some(body) = syn.block() {
            for statement in body.statements() {
                has_explicit_return |=
                    matches!(&statement, rayc_syntax::statement::Statement::Return(_));
                Box::pin(self.bind_statement(&statement)).await;
            }
        }
        if !has_explicit_return {
            self.push_unit_return_type_constraint(
                syn.block().map_or_else(|| syn.span(), |body| body.span()),
            )
            .await;
        }
        self.finish_thunk();

        self.push_diagnostic(Diagnostic::EffectHandlerNotSupported(
            EffectHandlerNotSupported::builder().span(syn.span()).build(),
        ));

        self.insert_expression(
            TypedExprKind::new_run_with(
                effect_id,
                effect_substitution,
                body_function,
                operation_handlers,
            ),
            syn.span(),
            return_type,
        )
    }
}

impl TAstBuilder {
    async fn bind_operation_handlers(
        &mut self,
        syntax: &RunWithSyntax,
        effect: GlobalSymbolID,
        operations: &FxHashMap<qbice::storage::intern::Interned<str>, GlobalSymbolID>,
        substitution: &rayc_type::subst::Subst,
    ) -> (
        FxHashMap<qbice::storage::intern::Interned<str>, rayc_lexical::tree::RelativeSpan>,
        Vec<TypedFunctionID>,
    ) {
        let mut seen = FxHashMap::default();
        let mut functions = Vec::new();
        let Some(handler_body) = syntax.handler_body() else { return (seen, functions) };

        for handler in handler_body.operations() {
            let Some(name) = handler.name() else { continue };
            if let Some(original_span) = seen.get(&name.kind.0).copied() {
                let duplicate_span = name.span();
                self.push_diagnostic(Diagnostic::DuplicateEffectOperationHandler(
                    DuplicateEffectOperationHandler::builder()
                        .name(name.kind.0)
                        .original_span(original_span)
                        .duplicate_span(duplicate_span)
                        .build(),
                ));
                continue;
            }
            seen.insert(name.kind.0.clone(), name.span());

            let Some(operation) = operations.get(&name.kind.0).copied() else {
                let span = name.span();
                self.push_diagnostic(Diagnostic::ExtraneousEffectOperationHandler(
                    ExtraneousEffectOperationHandler::builder()
                        .effect(effect)
                        .name(name.kind.0)
                        .span(span)
                        .build(),
                ));
                continue;
            };
            if let Some(function) =
                self.bind_operation_handler(operation, handler, substitution).await
            {
                functions.push(function);
            }
        }

        (seen, functions)
    }

    async fn bind_operation_handler(
        &mut self,
        operation: GlobalSymbolID,
        handler: rayc_syntax::expression::HandlerOperation,
        substitution: &rayc_type::subst::Subst,
    ) -> Option<TypedFunctionID> {
        let parameters = self.engine().get_parameter_map(operation).await;
        let parameter_patterns = handler
            .parameter_list()
            .map_or_else(Vec::new, |parameters| parameters.parameters().collect());
        if parameters.len() != parameter_patterns.len() {
            self.push_diagnostic(Diagnostic::MismatchedEffectOperationHandlerParameterCount(
                MismatchedEffectOperationHandlerParameterCount::builder()
                    .operation(operation)
                    .expected(parameters.len())
                    .found(parameter_patterns.len())
                    .span(
                        handler
                            .parameter_list()
                            .map_or_else(|| handler.span(), |parameters| parameters.span()),
                    )
                    .build(),
            ));
            return None;
        }

        let return_type = self
            .engine()
            .get_return_type(operation)
            .await
            .apply_subst_or_clone(substitution, self.engine());
        let function_id = self.start_operation_handler(operation, return_type);
        self.bind_operation_handler_parameters(
            function_id,
            parameters.iter().map(|(_, parameter)| parameter.ty()),
            parameter_patterns,
            substitution,
        );
        if let Some(body) = handler.block() {
            for statement in body.statements() {
                Box::pin(self.bind_statement(&statement)).await;
            }
        }
        self.finish_operation_handler();

        Some(function_id)
    }

    async fn effect_operations(
        &self,
        effect: GlobalSymbolID,
    ) -> FxHashMap<qbice::storage::intern::Interned<str>, GlobalSymbolID> {
        let members = self.engine().get_members(effect).await;
        let mut operations = FxHashMap::default();

        for member in members.all_ids() {
            let member = effect.target_id.make_global(member);
            if self.engine().get_symbol_kind(member).await == SymbolKind::EffectOperation {
                operations.insert(self.engine().get_name(member).await, member);
            }
        }

        operations
    }

    fn bind_operation_handler_parameters<'a>(
        &mut self,
        function_id: TypedFunctionID,
        parameter_types: impl Iterator<Item = &'a qbice::storage::intern::Interned<Ty>>,
        parameter_patterns: Vec<rayc_syntax::irrefutable_pattern::IrrefutablePattern>,
        substitution: &rayc_type::subst::Subst,
    ) {
        let parameter_name_binding_group = self.parameter_name_binding_group();
        for (parameter_type, parameter_pattern) in parameter_types.zip(parameter_patterns) {
            let parameter_type = parameter_type.apply_subst_or_clone(substitution, self.engine());
            let parameter_id =
                self.insert_operation_handler_parameter(TypedOperationHandlerParameter::new(
                    parameter_type.clone(),
                    parameter_pattern.span(),
                ));
            self.insert_name_binding_to_group_from_pattern(
                parameter_name_binding_group,
                &parameter_pattern,
                &parameter_type,
                Source::OperationHandlerParameter(TypedFunctionLocalID::new(
                    function_id,
                    parameter_id,
                )),
            );
        }
    }
}

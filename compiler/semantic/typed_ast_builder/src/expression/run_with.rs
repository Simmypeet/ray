use std::{collections::hash_map::Entry, ops::Not};

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_semantic_element::{parameter::get_parameter_map, return_type::get_return_type};
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID, SymbolID,
    member::{Member, get_members},
};
use rayc_syntax::expression::RunWith as RunWithSyntax;
use rayc_target::TargetID;
use rayc_type::{
    subst::Substitutable,
    ty::{Ty, effect_row::EffectLabel},
};
use rayc_typed_ast::{
    name_binding::Source,
    typed_expr::{TypedExprID, errored::Errored, run_with::RunWith},
    typed_function::{TypedFunctionID, TypedFunctionLocalID},
    typed_operation_handler::TypedOperationHandlerParameter,
};

use crate::{
    bind::Bind,
    diagnostic::{
        Diagnostic, DuplicateEffectOperationHandler, ExtraneousEffectOperationHandler,
        MismatchedEffectOperationHandlerParameterCount, MissingEffectOperationHandler,
    },
    tast_builder::TAstBuilder,
};

impl Bind<RunWithSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: RunWithSyntax) -> TypedExprID {
        let unit = Ty::new_unit(self.engine());
        let Some(effect) = syn.effect() else {
            return self.insert_expression(Errored::new_empty(), syn.span(), unit).await;
        };
        let Ok(effect) = self.resolve_effect_path(&effect).await else {
            return self.insert_expression(Errored::new_empty(), syn.span(), unit).await;
        };

        let effect_id = effect.symbol_id();
        let effect_label = self.engine().intern(EffectLabel::new(effect_id, effect.args().clone()));

        let effect_substitution = effect.substitution(self.engine()).await;

        let operation_handlers =
            self.bind_operation_handlers(&syn, effect_id, &effect_substitution).await;

        let (body_function, return_type) = self.build_run_with_body(&syn).await;

        let run_with =
            RunWith::new(effect_id, effect_substitution, body_function, operation_handlers);
        let handler_functions = run_with.operation_handlers().collect::<Vec<_>>();
        let expression_id =
            self.insert_expression_without_effect_composition(run_with, syn.span(), return_type);

        self.compose_run_with_effect(expression_id, body_function, handler_functions, effect_label)
            .await;

        expression_id
    }
}

impl TAstBuilder {
    fn report_missing_effect_operation_handler(
        &mut self,
        operations: &Member,
        target_id: TargetID,
        built: &FxHashMap<SymbolID, RelativeSpan>,
        syn: &RunWithSyntax,
    ) {
        let missing_operations = operations
            .namable_members()
            .filter_map(|x| built.contains_key(&x).not().then_some(target_id.make_global(x)))
            .collect::<Vec<_>>();

        if !missing_operations.is_empty() {
            self.push_diagnostic(Diagnostic::MissingEffectOperationHandler(
                MissingEffectOperationHandler::builder()
                    .operations(missing_operations)
                    .span(syn.effect().map_or_else(|| syn.span(), |effect| effect.span()))
                    .build(),
            ));
        }
    }

    async fn build_run_with_body(
        &mut self,
        syn: &RunWithSyntax,
    ) -> (TypedFunctionID, Interned<Ty>) {
        let (body_function, return_type) = self.start_thunk();
        let mut has_explicit_return = false;

        // TODO: determining return type like this is quite fragile, we should probably
        // have a more robust way
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

        (body_function, return_type)
    }

    async fn bind_operation_handlers(
        &mut self,
        syntax: &RunWithSyntax,
        effect: GlobalSymbolID,
        substitution: &rayc_type::subst::Subst,
    ) -> FxHashMap<SymbolID, TypedFunctionID> {
        let mut seen = FxHashMap::default();
        let mut functions = FxHashMap::default();
        let operations = self.engine().get_members(effect).await;

        let Some(handler_body) = syntax.handler_body() else { return FxHashMap::default() };

        for handler in handler_body.operations() {
            let Some(name) = handler.name() else { continue };

            // first retrieve the operation symbol ID from the name.
            let Some(operation) = operations.get_by_name(&name.kind) else {
                // oof, non-existent operation handler, report diagnostic and continue
                self.push_diagnostic(Diagnostic::ExtraneousEffectOperationHandler(
                    ExtraneousEffectOperationHandler::builder()
                        .effect(effect)
                        .name(name.kind.0)
                        .span(name.span)
                        .build(),
                ));
                continue;
            };

            // if handler for this operation is already seen, report diagnostic and continue
            match seen.entry(operation) {
                Entry::Vacant(vacant_entry) => {
                    vacant_entry.insert(name.span());
                }
                Entry::Occupied(occupied_entry) => {
                    self.push_diagnostic(Diagnostic::DuplicateEffectOperationHandler(
                        DuplicateEffectOperationHandler::builder()
                            .name(name.kind.0)
                            .original_span(*occupied_entry.get())
                            .duplicate_span(name.span)
                            .build(),
                    ));
                    continue;
                }
            }

            if let Some(function) = self
                .bind_operation_handler(
                    effect.target_id.make_global(operation),
                    handler,
                    substitution,
                )
                .await
            {
                assert!(functions.insert(operation, function).is_none());
            }
        }

        // report missing operation handlers before returning the functions
        self.report_missing_effect_operation_handler(&operations, effect.target_id, &seen, syntax);

        functions
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

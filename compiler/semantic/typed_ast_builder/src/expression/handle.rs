use std::collections::hash_map::Entry;

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_resolution::path::PathResolution;
use rayc_semantic_element::parameter::get_parameter_map;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
    member::get_members,
};
use rayc_syntax::{
    Passable,
    expression::{
        self, Arm as ArmSyntax, Handle as HandleSyntax, HandleClause as ClauseSyntax,
        HandleOperationClause as OperationClauseSyntax, HandleReturnClause as ReturnClauseSyntax,
        Leaf, Postfix, PostfixOperator,
    },
    irrefutable_pattern::IrrefutablePattern,
};
use rayc_type::{
    signature::get_return_type,
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{Ty, TyKind, args::Args, effect_row::EffectLabel},
};
use rayc_typed_ast::{
    name_binding::{NameBindingGroupID, Source},
    typed_expr::{
        TypedExprID,
        arm::Arm,
        errored::{Errored, ErroredChild},
        handle::{Handle, HandledEffect, OperationClause, ReturnClause},
        tuple::Tuple,
    },
    typed_function::TypedFunctionLocalID,
    typed_variable::{TypedVariable, TypedVariableID},
};

use crate::{
    bind::Bind,
    diagnostic::{
        Diagnostic, DuplicateHandleClause, DuplicatedHandleClause, HandleClauseNotOperation,
        HandleOperandNotCall, HandleOperandNotContinuation, MismatchedHandleClauseParameterCount,
        MismatchedResumeValueCount, MissingHandleClause,
    },
    tast_builder::{StatementBlockKind, TAstBuilder},
};

/// The call that a `handle` resumes: `continuation(arguments)`.
struct ResumedCall {
    continuation: TypedExprID,
    arguments: Vec<TypedExprID>,

    /// The arguments of the call, parentheses included. For a `coro`
    /// operand, which has none, it is the operand.
    arguments_span: RelativeSpan,
}

/// The clauses of one effect that a `handle` handles, as they are bound.
struct EffectClauses {
    label: Interned<EffectLabel>,
    substitution: Subst,

    /// The operation the first clause names, which is where the effect is
    /// first handled.
    span: RelativeSpan,

    /// Where each operation that has a clause is named.
    seen: FxHashMap<GlobalSymbolID, RelativeSpan>,
    clauses: Vec<OperationClause>,
}

/// The clauses of a `handle`, as they are bound in source order.
struct BoundClauses {
    /// The clauses of each handled effect.
    effects: FxHashMap<GlobalSymbolID, EffectClauses>,

    /// The `case return` clause.
    return_clause: Option<ReturnClause>,

    /// What is left of the effect of the coroutine, once the labels handled
    /// so far are taken out of it.
    forwarded_effect: Interned<Ty>,
}

/// What the clauses of a `handle` are typed against.
struct ClauseContext {
    /// The `c` of the `Continuation[r, c]` that is resumed. Every
    /// continuation a clause binds resumes the same coroutine.
    coroutine_ty: Interned<Ty>,

    /// The type of the whole `handle`, which every clause has.
    answer_ty: Interned<Ty>,
}

/// The coroutine that a `handle` resumes, as its continuation tells it.
struct ResumedCoroutine {
    continuation: TypedExprID,

    /// The value the coroutine is resumed with, coerced to what the
    /// continuation waits for.
    value: TypedExprID,

    /// The `Coroutine` dictionary of the coroutine behind the continuation.
    instance: Interned<Ty>,

    /// The type the coroutine completes with.
    return_ty: Interned<Ty>,

    /// The effects the coroutine performs.
    effect: Interned<Ty>,
}

impl Bind<HandleSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: HandleSyntax) -> TypedExprID {
        let span = syn.span();
        let Some(operand) = syn.operand() else {
            return self.push_error_expression(span).await;
        };

        // The outermost call of the operand is the resume: its callee is the
        // continuation, and its one argument is the value to resume with.
        let coroutine_ty = self.new_type_inference();
        let resumed = match self.bind_resume_operands(&operand, span).await {
            Ok((continuation, value)) => {
                Ok(self.type_resumed_coroutine(continuation, value, &coroutine_ty).await)
            }
            Err(errored) => Err(errored),
        };

        // Every clause has the answer type, which is the type of the whole
        // expression.
        let context = ClauseContext { coroutine_ty, answer_ty: self.new_type_inference() };

        let (return_ty, coroutine_effect) = resumed.as_ref().map_or_else(
            |_| (self.new_type_inference(), self.new_effect_inference()),
            |coroutine| (coroutine.return_ty.clone(), coroutine.effect.clone()),
        );

        // Each handled label is taken out of the effect of the coroutine
        // where its first clause is written. What is left is forwarded.
        let mut clauses = BoundClauses {
            effects: FxHashMap::default(),
            return_clause: None,
            forwarded_effect: coroutine_effect,
        };

        let syn_clauses = syn.clauses();
        for clause in syn_clauses.iter().flat_map(expression::HandleClauses::clauses) {
            match clause {
                ClauseSyntax::Operation(clause) => {
                    self.bind_operation_clause(&clause, &mut clauses, &context).await;
                }
                ClauseSyntax::Return(clause) => {
                    self.bind_return_clause(&clause, &mut clauses, &return_ty, &context).await;
                }
            }
        }
        let BoundClauses { effects, return_clause, forwarded_effect } = clauses;

        let mut handled_effects = FxHashMap::default();
        for (effect_id, effect) in effects {
            let handled = self.finish_handled_effect(effect_id, effect).await;
            handled_effects.insert(effect_id, handled);
        }

        let coroutine = match resumed {
            Ok(coroutine) => coroutine,
            Err(errored) => {
                let children = std::iter::once(ErroredChild::from(errored))
                    .chain(handled_effects.into_values().flat_map(|effect| {
                        effect
                            .into_clauses()
                            .into_iter()
                            .flat_map(|clause| clause.into_arm().into_errored_children())
                    }))
                    .chain(
                        return_clause
                            .into_iter()
                            .flat_map(|clause| clause.into_arm().into_errored_children()),
                    );
                return self.push_handle_error(span, children).await;
            }
        };

        // Without a return clause, the value the coroutine returned is the
        // value of the `handle`.
        if return_clause.is_none() {
            self.push_handle_clause_constraint(&coroutine.return_ty, &context.answer_ty, span)
                .await;
        }

        let handle = Handle::new(
            coroutine.continuation,
            coroutine.value,
            coroutine.instance,
            coroutine.return_ty,
            handled_effects,
            return_clause,
            forwarded_effect,
        );
        self.insert_handle(handle, span, context.answer_ty).await
    }
}

impl TAstBuilder {
    /// Binds the operand of a `handle` as the continuation it resumes and
    /// the value it resumes it with.
    async fn bind_resume_operands(
        &mut self,
        operand: &Postfix,
        span: RelativeSpan,
    ) -> Result<(TypedExprID, TypedExprID), TypedExprID> {
        let Some(call) = self.bind_resumed_call(operand).await else {
            return Err(self.push_handle_error(span, []).await);
        };
        let ResumedCall { continuation, arguments, arguments_span } = call;

        let value = match arguments.as_slice() {
            &[value] => value,

            // `continuation()` resumes with `()`, which is also what a
            // coroutine that was not started yet waits for.
            [] => {
                let unit = Ty::new_unit(self.engine());
                self.insert_expression(Tuple::new(Vec::new()), arguments_span, unit).await
            }

            // The clauses are still typed, against an errored value that
            // fits whatever the continuation waits for.
            _ => {
                self.push_diagnostic(Diagnostic::MismatchedResumeValueCount(
                    MismatchedResumeValueCount::new(arguments.len(), arguments_span),
                ));
                self.push_error_expression_with_expression_children(arguments_span, arguments).await
            }
        };

        // A callee that is already known not to be a continuation is
        // reported once, here. Typing the rest against it would only repeat
        // the error.
        if !self.may_be_continuation(continuation).await {
            return Err(self.push_handle_error(span, [continuation.into(), value.into()]).await);
        }

        Ok((continuation, value))
    }

    /// Types the call a `handle` resumes: `typeof(continuation) ~
    /// Continuation[x, coroutine_ty]`, `typeof(value) ~ x`, and there is a
    /// `Coroutine` dictionary of `coroutine_ty`.
    async fn type_resumed_coroutine(
        &mut self,
        continuation: TypedExprID,
        value: TypedExprID,
        coroutine_ty: &Interned<Ty>,
    ) -> ResumedCoroutine {
        let resume_ty = self.new_type_inference();
        let continuation_ty = self.continuation_type(&resume_ty, coroutine_ty).await;
        self.push_handle_continuation_constraint(&continuation_ty, continuation).await;

        let value = self.coerce(value, &resume_ty).await;
        self.push_handle_resume_value_constraint(&resume_ty, value).await;

        let coroutine_trait = self.engine().get_core_item(CoreItem::CoroutineTrait).await;
        let trait_ref =
            TraitRef::new(coroutine_trait, Args::new([coroutine_ty.clone()], self.engine()));
        let instance =
            self.infer_trait_instance(&trait_ref, self.span_of_expression(continuation)).await;
        let return_ty = self.coroutine_associated(CoreItem::CoroutineReturn, &instance).await;
        let effect = self.coroutine_associated(CoreItem::CoroutineEffect, &instance).await;

        ResumedCoroutine { continuation, value, instance, return_ty, effect }
    }

    /// Inserts `handle` as the expression at `span` of type `answer_ty`, and
    /// composes its effect.
    async fn insert_handle(
        &mut self,
        handle: Handle,
        span: RelativeSpan,
        answer_ty: Interned<Ty>,
    ) -> TypedExprID {
        if handle.return_clause().is_some() && handle.arms().all(Arm::diverges) {
            let unit = Ty::new_unit(self.engine());
            self.push_handle_clause_constraint(&unit, &answer_ty, span).await;
        }

        // The clauses are branches of this function, so what they perform
        // composes into the expression as its other sub-expressions do.
        let forwarded_effect = handle.forwarded_effect().clone();
        let expression = self.insert_expression(handle, span, answer_ty).await;

        // The `handle` itself performs what it forwards. A row that is known
        // by now is introduced like the effect of a call, which opens it so
        // that it composes with the other effects of the function.
        // TODO: This is a bit of a hack to make the composition work, it relies
        // on the fact that the concrete effect row must be known at this point.
        // Most of the time, it would. But if it is not, it could left user with a
        // confusing error why the effect composition is not working. Perhaps, a
        // new better effect system and inference would solve this issue :-P
        let forwarded_effect = self.latest_type(&forwarded_effect).await;
        self.push_effect_introduction(expression, &forwarded_effect).await;

        expression
    }
}

impl TAstBuilder {
    /// Returns the errored expression a rejected `handle` at `span` is bound
    /// to, keeping `children`.
    async fn push_handle_error(
        &mut self,
        span: RelativeSpan,
        children: impl IntoIterator<Item = ErroredChild>,
    ) -> TypedExprID {
        let children = children.into_iter().collect();
        let ty = Ty::new_error(TyKind::Star, self.engine());
        self.insert_expression(Errored::new(children), span, ty).await
    }

    /// Returns whether `expression`, the callee of the call a `handle`
    /// resumes, may still turn out to be a continuation: its type is one,
    /// or is not inferred yet.
    async fn may_be_continuation(&mut self, expression: TypedExprID) -> bool {
        let ty = self.latest_type(&self.type_of_expression(expression)).await;
        let continuation_id = self.engine().get_core_item(CoreItem::ContinuationStruct).await;

        if ty.as_inference().is_some()
            || ty.as_struct_view().is_some_and(|view| view.symbol_id() == continuation_id)
        {
            return true;
        }

        if !ty.contains_error() {
            self.push_diagnostic(Diagnostic::HandleOperandNotContinuation(
                HandleOperandNotContinuation::new(ty, self.span_of_expression(expression)),
            ));
        }
        false
    }

    /// Returns `core.Continuation[resume_ty, coroutine_ty]`.
    async fn continuation_type(
        &self,
        resume_ty: &Interned<Ty>,
        coroutine_ty: &Interned<Ty>,
    ) -> Interned<Ty> {
        let continuation_id = self.engine().get_core_item(CoreItem::ContinuationStruct).await;
        Ty::new_struct(
            continuation_id,
            Args::new([resume_ty.clone(), coroutine_ty.clone()], self.engine()),
            self.engine(),
        )
    }

    /// Returns the associated type `member` of the `Coroutine` dictionary
    /// `coroutine_instance`: its `Return` or its `Effect`.
    async fn coroutine_associated(
        &self,
        member: CoreItem,
        coroutine_instance: &Interned<Ty>,
    ) -> Interned<Ty> {
        let member = self.engine().get_core_item(member).await;
        Ty::new_instance_associated(member, coroutine_instance.clone(), [], self.engine())
    }

    /// Binds the operand of a `handle` as the call it resumes: the callee as
    /// the continuation, and the arguments without calling anything. A
    /// `coro` operand is the continuation itself, resumed with no argument.
    async fn bind_resumed_call(&mut self, operand: &Postfix) -> Option<ResumedCall> {
        // REVIEW: This function is a bit of a hack. It heavily relies on
        // inspecting the syntax structure of the operand to be in a certain
        // form. For instance,  `handle coro call(): ...` would work but
        // `handle (coro call()): ...` would not. Let's discuss about how we
        // could improve this, you don't have to direcly apply this suggestionn
        // yet. Also, the modification to the postfix binding that takes an
        // additional parameter to determine how many postfix operators to bind
        // is a bit of a hack too.
        let operator_count = operand.postfixes().count();

        // `coro call()` starts the coroutine it creates.
        if operator_count == 0
            && let Some(Leaf::Coro(coro)) = operand.leaf()
        {
            let continuation = Box::pin(self.bind(coro)).await;
            return Some(ResumedCall {
                continuation,
                arguments: Vec::new(),
                arguments_span: operand.span(),
            });
        }

        // `callee(arguments)` with any callee other than a bare name: the
        // last postfix operator is the call.
        if let Some(PostfixOperator::Call(call)) = operand.postfixes().last() {
            let continuation = self.bind_postfix_prefix(operand, operator_count - 1).await;
            let arguments = self.bind_call_arguments(&call).await;
            return Some(ResumedCall { continuation, arguments, arguments_span: call.span() });
        }

        // `name(arguments)` is parsed as the call of a path. A continuation
        // is a value, so only the name of a local binding can be one.
        if operator_count == 0
            && let Some(Leaf::DirectCall(direct_call)) = operand.leaf()
            && let Some(identifier) = direct_call.path().and_then(|path| path.bare_identifier())
            && self.lookup_name_binding(&identifier.kind.0).is_some()
            && let Some(call) = direct_call.call()
        {
            let continuation = self.bind(identifier).await;
            let arguments = self.bind_call_arguments(&call).await;
            return Some(ResumedCall { continuation, arguments, arguments_span: call.span() });
        }

        // Anything else resumes nothing. The operand is not bound: binding a
        // call would introduce its effect, which the `handle` never performs.
        self.push_diagnostic(Diagnostic::HandleOperandNotCall(HandleOperandNotCall::new(
            operand.span(),
        )));
        None
    }

    /// Binds the clause `case Effect.operation(parameters) resume k: ...`
    /// into `clauses`.
    async fn bind_operation_clause(
        &mut self,
        syn: &OperationClauseSyntax,
        clauses: &mut BoundClauses,
        context: &ClauseContext,
    ) {
        let Some(path) = syn.operation() else { return };
        let span = path.span();
        let operation = match self.resolve_path(&path).await {
            Ok(PathResolution::EffectOperation(operation)) => operation,
            Ok(_) => {
                self.push_diagnostic(Diagnostic::HandleClauseNotOperation(
                    HandleClauseNotOperation::new(span),
                ));
                return;
            }

            // The path does not resolve, which was reported.
            Err(_) => return,
        };

        let effect = operation.effect();
        let effect_id = effect.symbol_id();
        let label = self.engine().intern(EffectLabel::new(effect_id, effect.args().clone()));

        let handled = match clauses.effects.entry(effect_id) {
            // Every clause of an effect is written for the same label.
            Entry::Occupied(entry) => {
                let handled = entry.into_mut();
                self.push_same_handled_effect_constraint(handled.label.clone(), label, span).await;
                handled
            }
            Entry::Vacant(entry) => {
                clauses.forwarded_effect = self
                    .push_handled_effect_constraint(
                        clauses.forwarded_effect.clone(),
                        label.clone(),
                        span,
                    )
                    .await;
                entry.insert(EffectClauses {
                    label,
                    substitution: effect.substitution(self.engine()).await,
                    span,
                    seen: FxHashMap::default(),
                    clauses: Vec::new(),
                })
            }
        };

        let operation_id = operation.symbol_id();
        match handled.seen.entry(operation_id) {
            Entry::Vacant(entry) => {
                entry.insert(span);
            }
            Entry::Occupied(entry) => {
                let original_span = *entry.get();
                self.push_diagnostic(Diagnostic::DuplicateHandleClause(
                    DuplicateHandleClause::new(
                        DuplicatedHandleClause::Operation(operation_id),
                        original_span,
                        span,
                    ),
                ));
                return;
            }
        }

        let substitution = handled.substitution.clone();
        if let Some(clause) =
            self.bind_operation_clause_body(operation_id, syn, &substitution, context).await
        {
            handled.clauses.push(clause);
        }
    }

    /// Reports the operations of `handled` that have no clause, and returns
    /// the effect with the clauses it has.
    async fn finish_handled_effect(
        &mut self,
        effect_id: GlobalSymbolID,
        handled: EffectClauses,
    ) -> HandledEffect {
        let operations = self.engine().get_members(effect_id).await;
        let mut missing = operations
            .namable_members()
            .map(|operation| effect_id.target_id.make_global(operation))
            .filter(|operation| !handled.seen.contains_key(operation))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            missing.sort_unstable();
            self.push_diagnostic(Diagnostic::MissingHandleClause(MissingHandleClause::new(
                missing,
                handled.span,
            )));
        }

        HandledEffect::new(handled.substitution, handled.clauses)
    }

    /// Binds the parameters, the continuation and the body of the clause of
    /// `operation`, an operation of the effect whose parameters
    /// `substitution` instantiates.
    async fn bind_operation_clause_body(
        &mut self,
        operation: GlobalSymbolID,
        syn: &OperationClauseSyntax,
        substitution: &Subst,
        context: &ClauseContext,
    ) -> Option<OperationClause> {
        let parameters = self.engine().get_parameter_map(operation).await;
        let patterns = syn
            .parameter_list()
            .map_or_else(Vec::new, |parameters| parameters.parameters().collect());

        if patterns.len() != parameters.len() {
            self.push_diagnostic(Diagnostic::MismatchedHandleClauseParameterCount(
                MismatchedHandleClauseParameterCount::new(
                    operation,
                    parameters.len(),
                    patterns.len(),
                    syn.parameter_list().map_or_else(|| syn.span(), |list| list.span()),
                ),
            ));
            return None;
        }

        // The bindings of the clause are variables of this function, in a
        // scope of their own.
        self.enter_statement_block(StatementBlockKind::Plain);
        let group = self.push_new_name_binding_group();

        // The arguments of the operation, as it was performed.
        let mut parameter_variables = Vec::with_capacity(parameters.len());
        for ((_, parameter), pattern) in parameters.iter().zip(&patterns) {
            let ty = parameter.ty().apply_subst_or_clone(substitution, self.engine());
            parameter_variables.push(self.bind_clause_variable(group, pattern, &ty));
        }

        // The continuation of the rest of the coroutine waits for the result
        // of the operation. A clause that does not name it still receives
        // it, and drops it.
        let result_ty = self
            .engine()
            .get_return_type(operation)
            .await
            .apply_subst_or_clone(substitution, self.engine());
        let continuation_ty = self.continuation_type(&result_ty, &context.coroutine_ty).await;
        let continuation = syn
            .resume()
            .and_then(|resume| resume.pattern())
            .map(|pattern| self.bind_clause_variable(group, &pattern, &continuation_ty));

        let arm = self.bind_clause_arm(syn.arm(), syn.span(), context).await;

        let span = syn.operation().map_or_else(|| syn.span(), |path| path.span());
        Some(OperationClause::new(
            operation,
            span,
            parameter_variables,
            continuation,
            continuation_ty,
            arm,
        ))
    }

    /// Binds the clause `case return x: ...` into `clauses`, where `x` has
    /// the type `return_ty` the coroutine completes with.
    async fn bind_return_clause(
        &mut self,
        syn: &ReturnClauseSyntax,
        clauses: &mut BoundClauses,
        return_ty: &Interned<Ty>,
        context: &ClauseContext,
    ) {
        let span = syn.return_keyword().map_or_else(|| syn.span(), |keyword| keyword.span);
        if let Some(original) = &clauses.return_clause {
            self.push_diagnostic(Diagnostic::DuplicateHandleClause(DuplicateHandleClause::new(
                DuplicatedHandleClause::Return,
                original.span(),
                span,
            )));
            return;
        }

        self.enter_statement_block(StatementBlockKind::Plain);
        let group = self.push_new_name_binding_group();

        // A clause that does not name the returned value still receives it,
        // and drops it.
        let variable =
            syn.pattern().map(|pattern| self.bind_clause_variable(group, &pattern, return_ty));
        let arm = self.bind_clause_arm(syn.arm(), syn.span(), context).await;

        clauses.return_clause = Some(ReturnClause::new(variable, span, arm));
    }

    /// Declares the variable of type `ty` that `pattern` binds in a clause,
    /// and names it in `group`.
    fn bind_clause_variable(
        &mut self,
        group: NameBindingGroupID,
        pattern: &IrrefutablePattern,
        ty: &Interned<Ty>,
    ) -> TypedVariableID {
        let variable = self.insert_variable(TypedVariable::new(ty.clone(), pattern.span()));
        self.insert_name_binding_to_group_from_pattern(
            group,
            pattern,
            ty,
            Source::Variable(TypedFunctionLocalID::new(self.current_typed_function_id(), variable)),
        );
        variable
    }

    /// Binds the body of a clause and closes the statement block that the
    /// clause entered for its bindings.
    async fn bind_clause_arm(
        &mut self,
        arm: Option<ArmSyntax>,
        clause_span: RelativeSpan,
        context: &ClauseContext,
    ) -> Arm {
        let (expression, span) = match arm {
            Some(ArmSyntax::Expression(expression_arm)) => {
                let span = expression_arm.span();
                let expression = match expression_arm.expression() {
                    Some(expression) => Box::pin(self.bind(expression)).await,
                    None => self.push_syntax_error_expression(span).await,
                };
                (Some(expression), span)
            }
            Some(ArmSyntax::Block(block)) => {
                for statement in block.statements().filter_map(Passable::into_option) {
                    Box::pin(self.bind_statement(&statement)).await;
                }
                (None, block.span())
            }

            // The parser has already reported the missing body.
            None => (Some(self.push_syntax_error_expression(clause_span).await), clause_span),
        };
        let statements = self.exit_statement_block();

        let arm = match expression {
            Some(expression) => {
                assert!(statements.is_empty(), "an expression arm should hold no statement");
                Arm::Expression(expression)
            }
            None => Arm::Block(statements),
        };

        match &arm {
            Arm::Expression(expression) => {
                let ty = self.type_of_expression(*expression);
                self.push_handle_clause_constraint(&ty, &context.answer_ty, span).await;
            }
            Arm::Block(_) => {
                if !arm.diverges() {
                    let unit = Ty::new_unit(self.engine());
                    self.push_handle_clause_constraint(&unit, &context.answer_ty, span).await;
                }
            }
        }

        arm
    }
}

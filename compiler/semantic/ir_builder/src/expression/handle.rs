use qbice::storage::intern::Interned;
use rayc_ir::{
    cfg::{BlockID, Handle as HandleTerminator, HandleClause, Terminator},
    ir_expr::{
        IRExpr, IRExprID, IRExprKind,
        handle::{Handle as IRHandle, HandledEffect, Yielded, YieldedValue},
        phi::Phi,
    },
    scope::ScopeID,
};
use rayc_lexical::tree::RelativeSpan;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    typed_expr::{
        arm::Arm,
        handle::{Handle, OperationClause, ReturnClause},
    },
    typed_variable::TypedVariableID,
};

use crate::{
    builder::{Builder, function_build_state::scope_tracker::ScopeKind},
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

/// An operation clause of a `handle` that is being lowered.
struct LoweredClause<'a> {
    clause: &'a OperationClause,
    block: BlockID,

    /// What the clause receives, in the order of [`clause_variables`].
    yielded: Vec<IRExprID>,
}

impl LoweredClause<'_> {
    /// Returns the clause as the terminator has it, keyed by its operation.
    fn to_handle_clause(&self) -> (GlobalSymbolID, HandleClause) {
        (self.clause.operation(), HandleClause::new(self.yielded.clone(), self.block))
    }
}

/// Iterates over what `clause` receives with the variable each is bound to:
/// the arguments of the operation, followed by the continuation of the rest
/// of the coroutine, which the clause may leave unnamed.
fn clause_variables(
    clause: &OperationClause,
) -> impl Iterator<Item = (Option<TypedVariableID>, YieldedValue)> + '_ {
    let arguments = clause
        .parameters()
        .iter()
        .copied()
        .enumerate()
        .map(|(index, parameter)| (Some(parameter), YieldedValue::Argument(index)));

    arguments.chain([(clause.continuation(), YieldedValue::Continuation)])
}

/// The value the coroutine completed with, which the return clause receives.
struct ReturnedValue {
    /// The `Handle` expression, whose value it is.
    expression: IRExprID,
    ty: Interned<Ty>,
}

/// The type and the source of the `handle` that its clauses give a value to.
struct Answer {
    ty: Interned<Ty>,
    span: RelativeSpan,
}

impl<'a> Lower<TypedExprWithID<&'a Handle>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Handle>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let answer = Answer { ty: typed_expression.ty().clone(), span: typed_expression.span() };
        let handle = expression.node();

        // The current block ends by resuming the coroutine. The clauses are
        // branches of this function: each gets a block and a scope of its
        // own, like the arms of an `if`.
        let coro_return_ty = handle.coro_return_ty(self.engine()).await;
        let handle_expression =
            self.lower_handle_expression(context, handle, coro_return_ty.clone(), &answer).await;
        let clauses = self.create_clauses(context, handle, handle_expression);
        let return_block = self.create_block();
        let merge_block = self.create_block();
        let mut arm_scopes = self.create_scope_branch(clauses.len() + 1).into_iter();

        // TODO: a `Handle` that forwards an effect gets an abandon block
        // once the abandon edges are lowered.
        self.terminate(Terminator::Handle(HandleTerminator::new(
            handle_expression,
            return_block,
            clauses.iter().map(LoweredClause::to_handle_clause).collect(),
            None,
        )));

        // Lower every successor, collecting the value each gives the
        // `handle` unless control never leaves it through its end.
        let mut incoming = Vec::new();
        for clause in &clauses {
            self.select_block(clause.block);
            let scope = arm_scopes.next().expect("an operation clause scope should exist");
            let value = self.lower_operation_clause(context, clause, scope, &answer).await;
            if let Some(value) = value {
                incoming.push((self.jump_to(merge_block), value));
            }
        }

        self.select_block(return_block);
        let scope = arm_scopes.next().expect("a return clause scope should exist");
        let value = match handle.return_clause() {
            Some(clause) => {
                let returned = ReturnedValue { expression: handle_expression, ty: coro_return_ty };
                self.lower_return_clause(context, clause, returned, scope, &answer).await
            }

            // Without a return clause, the returned value passes through.
            None => Some(handle_expression),
        };
        if let Some(value) = value {
            incoming.push((self.jump_to(merge_block), value));
        }

        // The value of the clause that ran is the value of the `handle`.
        self.select_block(merge_block);
        if incoming.is_empty() {
            LoweredExpression::RValue(self.emit_unit(answer.span))
        } else {
            LoweredExpression::RValue(self.emit_expression(IRExpr::new(
                IRExprKind::Phi(Phi::new(incoming.into_iter().collect())),
                answer.span,
                answer.ty,
            )))
        }
    }
}

impl Builder {
    /// Lowers the operands of `handle` and inserts its `Handle` expression,
    /// which the terminator of the current block then evaluates.
    ///
    /// The value of the expression is the one the coroutine completed with,
    /// of type `coro_return_ty`.
    async fn lower_handle_expression(
        &mut self,
        context: &LoweringContext<'_>,
        handle: &Handle,
        coro_return_ty: Interned<Ty>,
        answer: &Answer,
    ) -> IRExprID {
        // The continuation is evaluated before the value, as a callee is
        // before its arguments.
        let continuation = self.lower_rvalue_by_id(context, handle.continuation()).await;
        let value = self.lower_rvalue_by_id(context, handle.value()).await;

        let handled_effects = handle
            .handled_effects()
            .map(|(effect_id, effect)| {
                (effect_id, HandledEffect::new(effect.effect_substitution().clone()))
            })
            .collect();

        self.insert_expression(IRExpr::new(
            IRExprKind::Handle(IRHandle::new(
                continuation,
                value,
                handle.coroutine_instance().clone(),
                handled_effects,
                handle.forwarded_effect().clone(),
            )),
            answer.span,
            coro_return_ty,
        ))
    }

    /// Creates the block of every operation clause of `handle`, and the
    /// `Yielded` expressions each receives from `handle_expression`.
    ///
    /// The terminator defines those expressions, on the edge into the
    /// clause, so no instruction evaluates them.
    fn create_clauses<'a>(
        &mut self,
        context: &LoweringContext<'_>,
        handle: &'a Handle,
        handle_expression: IRExprID,
    ) -> Vec<LoweredClause<'a>> {
        let mut clauses = Vec::new();

        for clause in handle.handled_effects().flat_map(|(_, effect)| effect.clauses()) {
            let yielded = clause_variables(clause)
                .map(|(variable_id, value)| {
                    // Only the continuation may be unnamed.
                    let (ty, span) = variable_id.map_or_else(
                        || (clause.continuation_ty().clone(), clause.span()),
                        |variable_id| {
                            let variable = context.variable(variable_id);
                            (variable.ty().clone(), variable.span())
                        },
                    );
                    let yielded = Yielded::new(handle_expression, clause.operation(), value);

                    self.insert_expression(IRExpr::new(IRExprKind::Yielded(yielded), span, ty))
                })
                .collect();

            let block = self.create_block();
            clauses.push(LoweredClause { clause, block, yielded });
        }

        clauses
    }

    /// Lowers an operation clause into the current block, in the scope
    /// `scope`: what the clause received is stored in the variables it
    /// binds, and its body follows.
    ///
    /// Returns the value the clause gives the `handle`, if control leaves it
    /// through its end.
    async fn lower_operation_clause(
        &mut self,
        context: &LoweringContext<'_>,
        clause: &LoweredClause<'_>,
        scope: ScopeID,
        answer: &Answer,
    ) -> Option<IRExprID> {
        self.enter_existing_scope(scope, ScopeKind::Lexical);

        let variables = clause_variables(clause.clause).map(|(variable_id, _)| variable_id);
        for (variable_id, value) in variables.zip(&clause.yielded) {
            match variable_id {
                Some(variable_id) => self.bind_clause_variable(context, variable_id, *value),

                // Only the continuation may be unnamed.
                None => self.bind_unnamed_clause_value(
                    *value,
                    clause.clause.continuation_ty().clone(),
                    clause.clause.span(),
                ),
            }
        }

        self.lower_clause_body(context, clause.clause.arm(), answer).await
    }

    /// Lowers the return clause into the current block, in the scope
    /// `scope`: the value the coroutine returned is stored in the variable
    /// the clause binds, and its body follows.
    ///
    /// Returns the value the clause gives the `handle`, if control leaves it
    /// through its end.
    async fn lower_return_clause(
        &mut self,
        context: &LoweringContext<'_>,
        clause: &ReturnClause,
        returned: ReturnedValue,
        scope: ScopeID,
        answer: &Answer,
    ) -> Option<IRExprID> {
        self.enter_existing_scope(scope, ScopeKind::Lexical);
        match clause.variable() {
            Some(variable_id) => {
                self.bind_clause_variable(context, variable_id, returned.expression);
            }
            None => {
                self.bind_unnamed_clause_value(returned.expression, returned.ty, clause.span());
            }
        }

        self.lower_clause_body(context, clause.arm(), answer).await
    }

    /// Declares the variable `variable_id` that a clause binds, in the
    /// current scope, and stores `value` in it.
    fn bind_clause_variable(
        &mut self,
        context: &LoweringContext<'_>,
        variable_id: TypedVariableID,
        value: IRExprID,
    ) {
        let variable = self.register_source_variable(context, variable_id);
        self.emit_store(
            self.variable_address(variable),
            value,
            context.variable(variable_id).span(),
        );
    }

    /// Stores `value` of type `ty`, which a clause receives without naming
    /// it, in a variable of the current scope, which drops it.
    fn bind_unnamed_clause_value(&mut self, value: IRExprID, ty: Interned<Ty>, span: RelativeSpan) {
        let variable = self.create_unnamed_variable(ty, span);
        self.emit_store(self.variable_address(variable), value, span);
    }

    /// Lowers the body of a clause and closes the scope of the clause.
    async fn lower_clause_body(
        &mut self,
        context: &LoweringContext<'_>,
        arm: &Arm,
        answer: &Answer,
    ) -> Option<IRExprID> {
        let value = self.lower_arm(context, arm, answer.span).await;
        self.exit_scope();

        value
    }
}

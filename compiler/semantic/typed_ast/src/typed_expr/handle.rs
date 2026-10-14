use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::Ty,
};

use crate::{
    typed_expr::{SubExprs, TypedExprID, arm::Arm},
    typed_variable::TypedVariableID,
};

/// The clause of a `handle` that runs when the coroutine performs one
/// operation.
///
/// It is a branch of the enclosing function, not a function of its own: the
/// arguments of the operation and the continuation of the rest of the
/// coroutine are variables of the enclosing function.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct OperationClause {
    operation: GlobalSymbolID,

    /// Where the clause names its operation.
    span: RelativeSpan,
    parameters: Vec<TypedVariableID>,

    /// The variable `resume k` binds, or `None` when the clause has no
    /// `resume`.
    continuation: Option<TypedVariableID>,

    /// The type of the continuation the clause receives, whether or not it
    /// binds it.
    continuation_ty: Interned<Ty>,
    arm: Arm,
}

impl OperationClause {
    #[must_use]
    pub const fn new(
        operation: GlobalSymbolID,
        span: RelativeSpan,
        parameters: Vec<TypedVariableID>,
        continuation: Option<TypedVariableID>,
        continuation_ty: Interned<Ty>,
        arm: Arm,
    ) -> Self {
        Self { operation, span, parameters, continuation, continuation_ty, arm }
    }

    /// Returns the operation the clause runs for.
    #[must_use]
    pub const fn operation(&self) -> GlobalSymbolID { self.operation }

    /// Returns where the clause names its operation.
    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }

    /// Returns the variables bound to the arguments of the operation, in
    /// parameter order.
    #[must_use]
    pub fn parameters(&self) -> &[TypedVariableID] { &self.parameters }

    /// Returns the variable bound to the continuation of the rest of the
    /// coroutine, which waits for the result of the operation.
    ///
    /// Returns `None` when the clause has no `resume`. It still receives the
    /// continuation, and drops it.
    #[must_use]
    pub const fn continuation(&self) -> Option<TypedVariableID> { self.continuation }

    /// Returns the type of the continuation the clause receives.
    #[must_use]
    pub const fn continuation_ty(&self) -> &Interned<Ty> { &self.continuation_ty }

    #[must_use]
    pub const fn arm(&self) -> &Arm { &self.arm }

    #[must_use]
    pub fn into_arm(self) -> Arm { self.arm }
}

/// The operation clauses that a `handle` has for one effect of the coroutine:
/// every `case Effect[arguments].operation(...)` of that effect.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct HandledEffect {
    effect_substitution: Subst,
    clauses: Vec<OperationClause>,
}

impl HandledEffect {
    #[must_use]
    pub const fn new(effect_substitution: Subst, clauses: Vec<OperationClause>) -> Self {
        Self { effect_substitution, clauses }
    }

    /// Returns the substitution that instantiates the parameters of the
    /// effect with the arguments of the handled label.
    #[must_use]
    pub const fn effect_substitution(&self) -> &Subst { &self.effect_substitution }

    /// Returns the clauses of the effect, in source order.
    #[must_use]
    pub fn clauses(&self) -> &[OperationClause] { &self.clauses }

    #[must_use]
    pub fn into_clauses(self) -> Vec<OperationClause> { self.clauses }
}

/// The clause of a `handle` that runs when the coroutine completes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct ReturnClause {
    /// The variable `case return x` binds, or `None` for `case return`.
    variable: Option<TypedVariableID>,

    /// Where the clause is written: its `return` keyword.
    span: RelativeSpan,
    arm: Arm,
}

impl ReturnClause {
    #[must_use]
    pub const fn new(variable: Option<TypedVariableID>, span: RelativeSpan, arm: Arm) -> Self {
        Self { variable, span, arm }
    }

    /// Returns the variable bound to the value the coroutine returned.
    ///
    /// Returns `None` when the clause does not name the value. It still
    /// receives the value, and drops it.
    #[must_use]
    pub const fn variable(&self) -> Option<TypedVariableID> { self.variable }

    /// Returns where the clause is written: its `return` keyword.
    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }

    #[must_use]
    pub const fn arm(&self) -> &Arm { &self.arm }

    #[must_use]
    pub fn into_arm(self) -> Arm { self.arm }
}

/// Resumes a coroutine: `handle continuation(value): case ...`.
///
/// The coroutine runs until it completes or performs an operation that has a
/// clause, and exactly one clause then runs. Every other operation the
/// coroutine performs is forwarded to the enclosing function.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct Handle {
    continuation: TypedExprID,
    value: TypedExprID,

    /// The `Coroutine` dictionary of the coroutine that is resumed, which
    /// tells what it returns and performs.
    coroutine_instance: Interned<Ty>,

    /// The type the coroutine completes with.
    return_ty: Interned<Ty>,

    /// The clauses of each handled effect. An effect is handled at most once.
    handled_effects: FxHashMap<GlobalSymbolID, HandledEffect>,
    return_clause: Option<ReturnClause>,

    /// The effects of the coroutine that no clause handles.
    forwarded_effect: Interned<Ty>,
}

impl Handle {
    #[must_use]
    pub const fn new(
        continuation: TypedExprID,
        value: TypedExprID,
        coroutine_instance: Interned<Ty>,
        return_ty: Interned<Ty>,
        handled_effects: FxHashMap<GlobalSymbolID, HandledEffect>,
        return_clause: Option<ReturnClause>,
        forwarded_effect: Interned<Ty>,
    ) -> Self {
        Self {
            continuation,
            value,
            coroutine_instance,
            return_ty,
            handled_effects,
            return_clause,
            forwarded_effect,
        }
    }

    /// Returns the continuation that is resumed, which the `handle` consumes.
    #[must_use]
    pub const fn continuation(&self) -> TypedExprID { self.continuation }

    /// Returns the value the coroutine is resumed with.
    #[must_use]
    pub const fn value(&self) -> TypedExprID { self.value }

    #[must_use]
    pub const fn coroutine_instance(&self) -> &Interned<Ty> { &self.coroutine_instance }

    /// Returns the type the coroutine completes with.
    #[must_use]
    pub const fn return_ty(&self) -> &Interned<Ty> { &self.return_ty }

    /// Iterates over every handled effect with its clauses, in no particular
    /// order.
    pub fn handled_effects(&self) -> impl Iterator<Item = (GlobalSymbolID, &HandledEffect)> {
        self.handled_effects.iter().map(|(effect, handled)| (*effect, handled))
    }

    /// Returns the return clause. Without one, the value the coroutine
    /// returned is the value of the `handle`.
    #[must_use]
    pub const fn return_clause(&self) -> Option<&ReturnClause> { self.return_clause.as_ref() }

    /// Returns the effect row the `handle` itself performs: what the
    /// coroutine performs and no clause handles.
    #[must_use]
    pub const fn forwarded_effect(&self) -> &Interned<Ty> { &self.forwarded_effect }

    /// Iterates over the arm of every clause: the operation clauses, followed
    /// by the return clause.
    pub fn arms(&self) -> impl Iterator<Item = &Arm> {
        self.handled_effects
            .values()
            .flat_map(|effect| effect.clauses.iter().map(OperationClause::arm))
            .chain(self.return_clause.iter().map(ReturnClause::arm))
    }
}

impl MutSubstitutable for Handle {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.coroutine_instance.apply_in_place(subst, engine);
        self.return_ty.apply_in_place(subst, engine);
        self.forwarded_effect.apply_in_place(subst, engine);
        for effect in self.handled_effects.values_mut() {
            effect.effect_substitution.apply_mut_subst(subst, engine);
            for clause in &mut effect.clauses {
                clause.continuation_ty.apply_in_place(subst, engine);
                clause.arm.apply_mut_subst(subst, engine);
            }
        }
        if let Some(clause) = &mut self.return_clause {
            clause.arm.apply_mut_subst(subst, engine);
        }
    }
}

impl SubExprs for Handle {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> {
        [self.continuation, self.value].into_iter().chain(self.arms().flat_map(Arm::sub_exprs))
    }
}

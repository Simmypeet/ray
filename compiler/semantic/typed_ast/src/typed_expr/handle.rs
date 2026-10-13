use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::Ty,
};

use crate::{
    typed_expr::{SubExprs, TypedExprID, if_else::Arm},
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
    parameters: Vec<TypedVariableID>,
    continuation: TypedVariableID,
    arm: Arm,
}

impl OperationClause {
    #[must_use]
    pub const fn new(
        operation: GlobalSymbolID,
        parameters: Vec<TypedVariableID>,
        continuation: TypedVariableID,
        arm: Arm,
    ) -> Self {
        Self { operation, parameters, continuation, arm }
    }

    /// Returns the operation the clause runs for.
    #[must_use]
    pub const fn operation(&self) -> GlobalSymbolID { self.operation }

    /// Returns the variables bound to the arguments of the operation, in
    /// parameter order.
    #[must_use]
    pub fn parameters(&self) -> &[TypedVariableID] { &self.parameters }

    /// Returns the variable bound to the continuation of the rest of the
    /// coroutine, which waits for the result of the operation.
    #[must_use]
    pub const fn continuation(&self) -> TypedVariableID { self.continuation }

    #[must_use]
    pub const fn arm(&self) -> &Arm { &self.arm }
}

/// The operation clauses that a `handle` has for one effect of the coroutine:
/// every `case Effect[arguments].operation(...)` of that effect.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct HandledEffect {
    effect: GlobalSymbolID,
    effect_substitution: Subst,
    clauses: Vec<OperationClause>,
}

impl HandledEffect {
    #[must_use]
    pub const fn new(
        effect: GlobalSymbolID,
        effect_substitution: Subst,
        clauses: Vec<OperationClause>,
    ) -> Self {
        Self { effect, effect_substitution, clauses }
    }

    #[must_use]
    pub const fn effect(&self) -> GlobalSymbolID { self.effect }

    /// Returns the substitution that instantiates the parameters of the
    /// effect with the arguments of the handled label.
    #[must_use]
    pub const fn effect_substitution(&self) -> &Subst { &self.effect_substitution }

    #[must_use]
    pub fn clauses(&self) -> &[OperationClause] { &self.clauses }
}

/// The clause of a `handle` that runs when the coroutine completes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct ReturnClause {
    variable: TypedVariableID,
    arm: Arm,
}

impl ReturnClause {
    #[must_use]
    pub const fn new(variable: TypedVariableID, arm: Arm) -> Self { Self { variable, arm } }

    /// Returns the variable bound to the value the coroutine returned.
    #[must_use]
    pub const fn variable(&self) -> TypedVariableID { self.variable }

    #[must_use]
    pub const fn arm(&self) -> &Arm { &self.arm }
}

/// Resumes a coroutine: `handle continuation(value): case ...`.
///
/// The coroutine runs until it completes or performs an operation that has a
/// clause, and exactly one clause then runs. Every other operation the
/// coroutine performs is forwarded to the enclosing function.
///
/// The clauses are branches, like the arms of an `if`: the value of the one
/// that runs is the value of the expression.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Handle {
    continuation: TypedExprID,
    value: TypedExprID,

    /// The `Coroutine` dictionary of the coroutine that is resumed, which
    /// tells what it returns and performs.
    coroutine_instance: Interned<Ty>,
    handled_effects: Vec<HandledEffect>,
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
        handled_effects: Vec<HandledEffect>,
        return_clause: Option<ReturnClause>,
        forwarded_effect: Interned<Ty>,
    ) -> Self {
        Self {
            continuation,
            value,
            coroutine_instance,
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

    #[must_use]
    pub fn handled_effects(&self) -> &[HandledEffect] { &self.handled_effects }

    /// Returns the return clause. Without one, the value the coroutine
    /// returned is the value of the `handle`.
    #[must_use]
    pub const fn return_clause(&self) -> Option<&ReturnClause> { self.return_clause.as_ref() }

    /// Returns the effect row the `handle` itself performs: what the
    /// coroutine performs and no clause handles.
    #[must_use]
    pub const fn forwarded_effect(&self) -> &Interned<Ty> { &self.forwarded_effect }

    /// Iterates over the arm of every clause: the operation clauses in source
    /// order, followed by the return clause.
    pub fn arms(&self) -> impl Iterator<Item = &Arm> {
        self.handled_effects
            .iter()
            .flat_map(|effect| effect.clauses.iter().map(OperationClause::arm))
            .chain(self.return_clause.iter().map(ReturnClause::arm))
    }

    fn arms_mut(&mut self) -> impl Iterator<Item = &mut Arm> {
        self.handled_effects
            .iter_mut()
            .flat_map(|effect| effect.clauses.iter_mut().map(|clause| &mut clause.arm))
            .chain(self.return_clause.iter_mut().map(|clause| &mut clause.arm))
    }
}

impl MutSubstitutable for Handle {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.coroutine_instance.apply_in_place(subst, engine);
        self.forwarded_effect.apply_in_place(subst, engine);
        for effect in &mut self.handled_effects {
            effect.effect_substitution.apply_mut_subst(subst, engine);
        }
        for arm in self.arms_mut() {
            arm.apply_mut_subst(subst, engine);
        }
    }
}

impl SubExprs for Handle {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> {
        [self.continuation, self.value].into_iter().chain(self.arms().flat_map(Arm::sub_exprs))
    }
}

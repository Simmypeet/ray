use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_hash::FxHashMap;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    subst::Subst,
    ty::{Ty, effect_row::EffectLabel},
};

use crate::{
    ir_expr::IRExprID,
    visit::{
        TypeSite, TypeVisitor, TypeVisitorMut, TypeVisitorMutAsync, VisitType, VisitTypeMut,
        VisitTypeMutAsync,
    },
};

/// An effect of a coroutine that a [`Handle`] has clauses for, instantiated
/// with the arguments of the handled label. The [`Handle`] keys it by the
/// effect.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct HandledEffect {
    substitution: Subst,
}

impl HandledEffect {
    #[must_use]
    pub const fn new(substitution: Subst) -> Self { Self { substitution } }

    /// Returns the substitution that instantiates the parameters of the
    /// effect, and with them the signatures of its operations.
    #[must_use]
    pub const fn substitution(&self) -> &Subst { &self.substitution }

    /// Returns the label that is handled: `effect_id`, the effect this is
    /// handled for, instantiated with the substitution.
    pub async fn label(
        &self,
        effect_id: GlobalSymbolID,
        engine: &TrackedEngine,
    ) -> Interned<EffectLabel> {
        EffectLabel::instantiated(effect_id, &self.substitution, engine).await
    }
}

/// Resumes a coroutine with a value, and runs it until it completes or
/// performs an operation of a handled effect. Every other operation the
/// coroutine performs is forwarded to the function that resumes it.
///
/// It is evaluated by a [`Terminator::Handle`], never by an instruction,
/// because control leaves it along one of several edges. Its value is the one
/// the coroutine completed with, and is defined on the return edge only. On
/// the edge into a clause, the terminator defines the [`Yielded`] expressions
/// of that clause instead.
///
/// [`Terminator::Handle`]: crate::cfg::Terminator::Handle
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct Handle {
    continuation: IRExprID,
    value: IRExprID,

    /// The `Coroutine` dictionary of the coroutine behind the continuation.
    coroutine_instance: Interned<Ty>,
    handled_effects: FxHashMap<GlobalSymbolID, HandledEffect>,
    forwarded_effect: Interned<Ty>,
}

impl Handle {
    #[must_use]
    pub const fn new(
        continuation: IRExprID,
        value: IRExprID,
        coroutine_instance: Interned<Ty>,
        handled_effects: FxHashMap<GlobalSymbolID, HandledEffect>,
        forwarded_effect: Interned<Ty>,
    ) -> Self {
        Self { continuation, value, coroutine_instance, handled_effects, forwarded_effect }
    }

    /// Returns the continuation that is resumed. It is consumed.
    #[must_use]
    pub const fn continuation(&self) -> IRExprID { self.continuation }

    /// Returns the value the coroutine is resumed with.
    #[must_use]
    pub const fn value(&self) -> IRExprID { self.value }

    /// Returns the `Coroutine` dictionary that tells what the coroutine
    /// returns and performs.
    #[must_use]
    pub const fn coroutine_instance(&self) -> &Interned<Ty> { &self.coroutine_instance }

    /// Iterates over the effects the terminator has clauses for, in no
    /// particular order.
    #[must_use]
    pub fn handled_effects(
        &self,
    ) -> impl ExactSizeIterator<Item = (GlobalSymbolID, &HandledEffect)> {
        self.handled_effects.iter().map(|(effect_id, handled)| (*effect_id, handled))
    }

    /// Returns the handled effect `effect_id`.
    ///
    /// # Panics
    ///
    /// Panics if `effect_id` is not handled.
    #[must_use]
    pub fn handled_effect(&self, effect_id: GlobalSymbolID) -> &HandledEffect {
        self.handled_effects.get(&effect_id).expect("the effect should be handled")
    }

    /// Returns the effect row that is forwarded: what the coroutine performs
    /// and no clause handles.
    #[must_use]
    pub const fn forwarded_effect(&self) -> &Interned<Ty> { &self.forwarded_effect }
}

/// Which of the values of a suspended operation a [`Yielded`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum YieldedValue {
    /// The argument of the operation at this position, in parameter order.
    Argument(usize),

    /// The continuation of the rest of the coroutine, which waits for the
    /// result of the operation.
    Continuation,
}

/// A value that a coroutine gave when it suspended at an operation that the
/// [`Handle`] has a clause for.
///
/// It takes no operand, and no instruction evaluates it: the
/// [`Terminator::Handle`] defines one for each argument of the operation and
/// one for the continuation, on the edge into the clause, which is how the
/// clause receives them.
///
/// [`Terminator::Handle`]: crate::cfg::Terminator::Handle
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Yielded {
    handle: IRExprID,
    effect_id: GlobalSymbolID,
    operation_id: GlobalSymbolID,
    value: YieldedValue,
}

impl Yielded {
    #[must_use]
    pub const fn new(
        handle: IRExprID,
        effect_id: GlobalSymbolID,
        operation_id: GlobalSymbolID,
        value: YieldedValue,
    ) -> Self {
        Self { handle, effect_id, operation_id, value }
    }

    /// Returns the [`Handle`] expression that suspended at the operation. It
    /// is not an operand: nothing of its value is read.
    #[must_use]
    pub const fn handle(&self) -> IRExprID { self.handle }

    /// Returns the handled effect of the [`Handle`] that the operation
    /// belongs to.
    #[must_use]
    pub const fn effect_id(&self) -> GlobalSymbolID { self.effect_id }

    /// Returns the operation the coroutine performed.
    #[must_use]
    pub const fn operation_id(&self) -> GlobalSymbolID { self.operation_id }

    #[must_use]
    pub const fn value(&self) -> YieldedValue { self.value }
}

impl VisitType for Handle {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        visitor.visit_type(&self.coroutine_instance, site);
        visitor.visit_type(&self.forwarded_effect, site);
        for effect in self.handled_effects.values() {
            for ty in effect.substitution.codomain() {
                visitor.visit_type(ty, site);
            }
        }
    }
}

impl VisitTypeMut for Handle {
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, site: TypeSite, visitor: &mut V) {
        visitor.visit_type_mut(&mut self.coroutine_instance, site);
        visitor.visit_type_mut(&mut self.forwarded_effect, site);
        for effect in self.handled_effects.values_mut() {
            for ty in effect.substitution.codomain_mut() {
                visitor.visit_type_mut(ty, site);
            }
        }
    }
}

impl VisitTypeMutAsync for Handle {
    async fn visit_types_mut_async<V: TypeVisitorMutAsync>(
        &mut self,
        site: TypeSite,
        visitor: &mut V,
    ) {
        visitor.visit_type_mut_async(&mut self.coroutine_instance, site).await;
        visitor.visit_type_mut_async(&mut self.forwarded_effect, site).await;
        for effect in self.handled_effects.values_mut() {
            for ty in effect.substitution.codomain_mut() {
                visitor.visit_type_mut_async(ty, site).await;
            }
        }
    }
}

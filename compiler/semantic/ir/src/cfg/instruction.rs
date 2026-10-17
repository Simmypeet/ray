use std::collections::BTreeMap;

use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;

use super::{BlockID, Point};
use crate::{
    address::Address,
    ir_expr::IRExprID,
    scope::ScopeID,
    visit::{
        TypeSite, TypeVisitor, TypeVisitorMut, TypeVisitorMutAsync, VisitType, VisitTypeMut,
        VisitTypeMutAsync,
    },
};

/// Instructions queued for insertion into a control-flow graph, applied all
/// at once by [`Cfg::insert_instructions`].
///
/// Every point refers to the block layout before any insertion, so callers
/// can queue insertions while replaying an analysis of that layout.
#[derive(Debug, Default)]
pub struct InstructionInsertion {
    /// The sequences queued in each block, by the index of the instruction
    /// they precede.
    pub(super) blocks: FxHashMap<BlockID, BTreeMap<usize, Vec<Instruction>>>,
}

impl InstructionInsertion {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    /// Queues `instructions` to run immediately before the instruction
    /// currently at `point`, or at the end of the block when `point` is one
    /// past its last instruction.
    ///
    /// Sequences queued at the same point run in the order they were queued.
    pub fn insert_before(
        &mut self,
        point: Point,
        instructions: impl IntoIterator<Item = Instruction>,
    ) {
        self.blocks
            .entry(point.block_id)
            .or_default()
            .entry(point.instruction_idx)
            .or_default()
            .extend(instructions);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Store {
    pub(super) address: Address,
    pub(super) expression: IRExprID,

    /// The source construct performing the write, such as an assignment or a
    /// `let` initializer.
    pub(super) span: RelativeSpan,
}

impl Store {
    #[must_use]
    pub const fn address(&self) -> &Address { &self.address }

    #[must_use]
    pub const fn expression(&self) -> IRExprID { self.expression }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }
}

/// Drops the unused value of an evaluated expression.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct ExprDiscard {
    pub(super) expression: IRExprID,

    /// The `Drop` dictionary selected for the expression's type.
    pub(super) drop_instance: Interned<Ty>,
}

impl ExprDiscard {
    #[must_use]
    pub const fn new(expression: IRExprID, drop_instance: Interned<Ty>) -> Self {
        Self { expression, drop_instance }
    }

    #[must_use]
    pub const fn expression(&self) -> IRExprID { self.expression }

    #[must_use]
    pub const fn drop_instance(&self) -> &Interned<Ty> { &self.drop_instance }
}

/// Drops the value held in a place with its `Drop` implementation.
///
/// Drop elaboration inserts one wherever a place still holds a value that
/// must be destroyed: before a scope pops, before a place is overwritten, and
/// on the edges where only some paths moved the value out.
///
/// Semantically, it is equivalent to a forced move out of the place followed
/// by a `Drop.drop` call on the moved value, and it consumes the place even
/// when its type is `Copy`. It is kept as a single instruction, rather than
/// being broken down into a `Load` and a `Call`, so analyses that care about
/// why a place is used can see that the place is only dropped. For instance,
/// liveness treats a local which is only dropped afterwards as drop-live
/// rather than use-live.
///
/// The `MonoIR` builder lowers it to the calls selected by the `Drop`
/// dictionary, passing the place directly without an intermediate temporary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct AddressDrop {
    address: Address,

    /// The `Drop` dictionary selected for the type of the value held in
    /// [`Self::address`].
    drop_instance: Interned<Ty>,

    /// The source construct responsible for the drop, such as the binding
    /// going out of scope or the assignment overwriting the place.
    span: RelativeSpan,
}

impl AddressDrop {
    #[must_use]
    pub const fn new(address: Address, drop_instance: Interned<Ty>, span: RelativeSpan) -> Self {
        Self { address, drop_instance, span }
    }

    #[must_use]
    pub const fn address(&self) -> &Address { &self.address }

    #[must_use]
    pub const fn drop_instance(&self) -> &Interned<Ty> { &self.drop_instance }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }
}

/// An operation evaluated at a precise position in a basic block.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum Instruction {
    /// Begins the lifetime of a lexical or temporary scope.
    ScopePush(ScopeID),
    /// Ends the lifetime of a lexical or temporary scope.
    ///
    /// This is the "storage dead" point of every variable declared in the
    /// scope: drop elaboration drops the values still held there just before
    /// it, and the storage itself is gone after it. A borrow of such a
    /// variable which is still live here is therefore a "borrowed value does
    /// not live long enough" error, or, for the temporaries of a temporary
    /// scope, "temporary value dropped while borrowed". A borrow of the data
    /// behind a pointer held in the variable does not end here.
    ScopePop(ScopeID),
    /// Defines and evaluates the identified expression exactly once.
    Expression(IRExprID),
    /// Drops the unused result of an evaluated expression.
    ExprDiscard(ExprDiscard),
    /// Drops the value held in a place. See [`AddressDrop`].
    AddressDrop(AddressDrop),
    /// Writes an already-defined expression value to an address.
    Store(Store),
}

impl VisitType for Instruction {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        match self {
            Self::ExprDiscard(discard) => visitor.visit_type(discard.drop_instance(), site),
            Self::AddressDrop(drop) => visitor.visit_type(drop.drop_instance(), site),

            Self::ScopePush(_) | Self::ScopePop(_) | Self::Expression(_) | Self::Store(_) => {}
        }
    }
}

impl VisitTypeMut for Instruction {
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, site: TypeSite, visitor: &mut V) {
        match self {
            Self::ExprDiscard(discard) => visitor.visit_type_mut(&mut discard.drop_instance, site),
            Self::AddressDrop(drop) => visitor.visit_type_mut(&mut drop.drop_instance, site),

            Self::ScopePush(_) | Self::ScopePop(_) | Self::Expression(_) | Self::Store(_) => {}
        }
    }
}

impl VisitTypeMutAsync for Instruction {
    async fn visit_types_mut_async<V: TypeVisitorMutAsync>(
        &mut self,
        site: TypeSite,
        visitor: &mut V,
    ) {
        match self {
            Self::ExprDiscard(discard) => {
                visitor.visit_type_mut_async(&mut discard.drop_instance, site).await;
            }
            Self::AddressDrop(drop) => {
                visitor.visit_type_mut_async(&mut drop.drop_instance, site).await;
            }

            Self::ScopePush(_) | Self::ScopePop(_) | Self::Expression(_) | Self::Store(_) => {}
        }
    }
}

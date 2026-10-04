use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{ID, OrderedArena};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::{Ty, lifetime::Lifetime};

use crate::{
    ir_lambda::CaptureMapID,
    visit::{
        TypeSite, TypeVisitor, TypeVisitorMut, TypeVisitorMutAsync, VisitType, VisitTypeMut,
        VisitTypeMutAsync,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct IROperationHandlerContext {
    operation: GlobalSymbolID,
    parameters: OperationHandlerParameterMap,
    return_ty: Interned<Ty>,
    effect: Interned<Ty>,
    capture_map: CaptureMapID,

    /// The lifetime `'env` of the `&'env mut Env` that one run of the handler
    /// reaches its captures through.
    ///
    /// The handlers of a `handle` share one environment, which each of them
    /// may run on any number of times, as an `FnMut` closure does in Rust.
    /// So a run only borrows the environment, for a lifetime it does not
    /// choose and that outlives nothing it can name: what it borrows from a
    /// capture may not escape it; see [`Self::environment_lifetime`].
    environment_lifetime: Interned<Ty>,
}

impl IROperationHandlerContext {
    /// Creates the context of a handler of `operation`. Its environment
    /// lifetime starts erased, as every lifetime the borrow checker chooses.
    pub(crate) fn new(
        operation: GlobalSymbolID,
        return_ty: Interned<Ty>,
        effect: Interned<Ty>,
        capture_map: CaptureMapID,
        engine: &TrackedEngine,
    ) -> Self {
        Self {
            operation,
            parameters: OperationHandlerParameterMap::default(),
            return_ty,
            effect,
            capture_map,
            environment_lifetime: Ty::new_lifetime(Lifetime::Erased, engine),
        }
    }

    #[must_use]
    pub const fn operation(&self) -> GlobalSymbolID { self.operation }

    #[must_use]
    pub fn parameters(
        &self,
    ) -> impl ExactSizeIterator<Item = (OperationHandlerParameterID, &OperationHandlerParameter)>
    {
        self.parameters.iter()
    }

    #[must_use]
    pub fn get_parameter(&self, id: OperationHandlerParameterID) -> &OperationHandlerParameter {
        self.parameters.get_parameter(id)
    }

    pub(crate) fn insert_parameter(
        &mut self,
        parameter: OperationHandlerParameter,
    ) -> OperationHandlerParameterID {
        self.parameters.insert_parameter(parameter)
    }

    #[must_use]
    pub const fn return_ty(&self) -> &Interned<Ty> { &self.return_ty }

    #[must_use]
    pub const fn effect(&self) -> &Interned<Ty> { &self.effect }

    #[must_use]
    pub(crate) const fn capture_map(&self) -> CaptureMapID { self.capture_map }

    /// Returns the lifetime `'env` that one run of the handler borrows its
    /// environment for.
    ///
    /// It is part of the signature of the handler, though of no type in it:
    /// a place rooted at a capture is behind one more dereference, of a
    /// `&'env mut` reference to the environment. Nothing instantiates it,
    /// since every run of the handler borrows the environment anew.
    #[must_use]
    pub const fn environment_lifetime(&self) -> &Interned<Ty> { &self.environment_lifetime }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable,
)]
pub struct OperationHandlerParameter {
    ty: Interned<Ty>,
    span: RelativeSpan,
}

impl OperationHandlerParameter {
    #[must_use]
    pub const fn new(ty: Interned<Ty>, span: RelativeSpan) -> Self { Self { ty, span } }

    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }
}

pub type OperationHandlerParameterID = ID<OperationHandlerParameter>;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default, Identifiable)]
struct OperationHandlerParameterMap {
    parameters: OrderedArena<OperationHandlerParameter>,
}

impl OperationHandlerParameterMap {
    fn get_parameter(&self, id: OperationHandlerParameterID) -> &OperationHandlerParameter {
        self.parameters.get(id).expect("operation handler parameter should exist")
    }

    fn insert_parameter(
        &mut self,
        parameter: OperationHandlerParameter,
    ) -> OperationHandlerParameterID {
        self.parameters.insert(parameter)
    }

    fn iter(
        &self,
    ) -> impl ExactSizeIterator<Item = (OperationHandlerParameterID, &OperationHandlerParameter)>
    {
        self.parameters.iter()
    }
}

impl VisitType for IROperationHandlerContext {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        for (_, parameter) in self.parameters() {
            parameter.visit_types(site, visitor);
        }
        visitor.visit_type(self.return_ty(), site);
        visitor.visit_type(self.effect(), site);
        visitor.visit_type(self.environment_lifetime(), site);
    }
}

impl VisitTypeMut for IROperationHandlerContext {
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, site: TypeSite, visitor: &mut V) {
        for (_, parameter) in self.parameters.parameters.iter_mut_unordered() {
            parameter.visit_types_mut(site, visitor);
        }
        visitor.visit_type_mut(&mut self.return_ty, site);
        visitor.visit_type_mut(&mut self.effect, site);
        visitor.visit_type_mut(&mut self.environment_lifetime, site);
    }
}

impl VisitTypeMutAsync for IROperationHandlerContext {
    async fn visit_types_mut_async<V: TypeVisitorMutAsync>(
        &mut self,
        site: TypeSite,
        visitor: &mut V,
    ) {
        for (_, parameter) in self.parameters.parameters.iter_mut_unordered() {
            parameter.visit_types_mut_async(site, visitor).await;
        }
        visitor.visit_type_mut_async(&mut self.return_ty, site).await;
        visitor.visit_type_mut_async(&mut self.effect, site).await;
        visitor.visit_type_mut_async(&mut self.environment_lifetime, site).await;
    }
}

impl VisitType for OperationHandlerParameter {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        visitor.visit_type(self.ty(), site);
    }
}

impl VisitTypeMut for OperationHandlerParameter {
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, site: TypeSite, visitor: &mut V) {
        visitor.visit_type_mut(&mut self.ty, site);
    }
}

impl VisitTypeMutAsync for OperationHandlerParameter {
    async fn visit_types_mut_async<V: TypeVisitorMutAsync>(
        &mut self,
        site: TypeSite,
        visitor: &mut V,
    ) {
        visitor.visit_type_mut_async(&mut self.ty, site).await;
    }
}

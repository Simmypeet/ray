use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::parameter::get_parameter_map;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{signature::get_return_type, ty::Ty};

use crate::{
    address::Local,
    ir_closure::{CaptureMapID, IRClosureContext, IRThunkContext},
    ir_operation_handler::IROperationHandlerContext,
    visit::{
        TypeSite, TypeVisitor, TypeVisitorMut, TypeVisitorMutAsync, VisitType, VisitTypeMut,
        VisitTypeMutAsync,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub enum IRContext {
    Def,
    Closure(IRClosureContext),
    Thunk(IRThunkContext),
    OperationHandler(IROperationHandlerContext),
}

impl IRContext {
    #[track_caller]
    pub fn assert_as_def_context(&self) {
        match self {
            Self::Def => {}
            Self::Closure(_) | Self::Thunk(_) | Self::OperationHandler(_) => {
                panic!("expected a def context, found a nested function context")
            }
        }
    }

    #[must_use]
    #[track_caller]
    pub fn assert_as_closure_context(&self) -> &IRClosureContext {
        match self {
            Self::Def | Self::Thunk(_) | Self::OperationHandler(_) => {
                panic!("expected a closure context, found a non-closure context")
            }
            Self::Closure(context) => context,
        }
    }

    #[track_caller]
    pub(super) fn assert_as_closure_context_mut(&mut self) -> &mut IRClosureContext {
        match self {
            Self::Def | Self::Thunk(_) | Self::OperationHandler(_) => {
                panic!("expected a closure context, found a non-closure context")
            }
            Self::Closure(context) => context,
        }
    }

    #[must_use]
    #[track_caller]
    pub fn assert_as_thunk_context(&self) -> &IRThunkContext {
        match self {
            Self::Thunk(context) => context,
            Self::Def | Self::Closure(_) | Self::OperationHandler(_) => {
                panic!("expected a thunk context, found another function context")
            }
        }
    }

    #[must_use]
    #[track_caller]
    pub fn assert_as_operation_handler_context(&self) -> &IROperationHandlerContext {
        match self {
            Self::OperationHandler(context) => context,
            Self::Def | Self::Closure(_) | Self::Thunk(_) => {
                panic!("expected an operation handler context, found another function context")
            }
        }
    }

    pub(super) const fn assert_capture_map_id(&self) -> CaptureMapID {
        self.capture_map_id().expect("a def context does not have captures")
    }

    /// Returns the effect row stored by a nested function context, or `None`
    /// for a def context, whose effect row is declared by the definition.
    pub(super) const fn nested_effect(&self) -> Option<&Interned<Ty>> {
        match self {
            Self::Def => None,
            Self::Closure(context) => Some(context.effect()),
            Self::Thunk(context) => Some(context.effect()),
            Self::OperationHandler(context) => Some(context.effect()),
        }
    }

    /// Returns the return type of a function of this context, which belongs
    /// to the definition `def_id`.
    ///
    /// The definition function takes the return type declared in the
    /// definition's signature, while a nested function stores its own.
    pub async fn return_ty(&self, def_id: GlobalSymbolID, engine: &TrackedEngine) -> Interned<Ty> {
        match self.nested_return_ty() {
            Some(return_ty) => return_ty.clone(),
            None => engine.get_return_type(def_id).await,
        }
    }

    /// Returns the parameters of a function of this context, which belongs
    /// to the definition `def_id`, in declaration order.
    ///
    /// The definition function takes the parameters declared in the
    /// definition's signature, while a nested function stores its own.
    pub async fn parameter_locals(
        &self,
        def_id: GlobalSymbolID,
        engine: &TrackedEngine,
    ) -> Vec<Local> {
        match self {
            Self::Def => {
                let parameters = engine.get_parameter_map(def_id).await;
                parameters.iter().map(|(parameter_id, _)| Local::Parameter(parameter_id)).collect()
            }
            Self::Closure(context) => context
                .parameters()
                .map(|(parameter_id, _)| Local::ClosureParameter(parameter_id))
                .collect(),
            Self::Thunk(_) => Vec::new(),
            Self::OperationHandler(context) => context
                .parameters()
                .map(|(parameter_id, _)| Local::OperationHandlerParameter(parameter_id))
                .collect(),
        }
    }

    /// Returns whether a function of this context borrows its captures
    /// rather than owning them.
    ///
    /// An operation handler may run many times over one environment, which
    /// the function creating it drops after the handled body.
    #[must_use]
    pub const fn borrows_captures(&self) -> bool {
        match self {
            Self::OperationHandler(_) => true,
            Self::Def | Self::Closure(_) | Self::Thunk(_) => false,
        }
    }

    /// Returns the lifetime that a run of a function of this context borrows
    /// the environment holding its captures for, or `None` for a function
    /// that owns its captures, or has none; see
    /// [`IROperationHandlerContext::environment_lifetime`].
    #[must_use]
    pub const fn environment_lifetime(&self) -> Option<&Interned<Ty>> {
        match self {
            Self::OperationHandler(context) => Some(context.environment_lifetime()),
            Self::Def | Self::Closure(_) | Self::Thunk(_) => None,
        }
    }

    /// Returns the return type stored by a nested function context, or
    /// `None` for a def context, whose return type is declared by the
    /// definition.
    const fn nested_return_ty(&self) -> Option<&Interned<Ty>> {
        match self {
            Self::Def => None,
            Self::Closure(context) => Some(context.return_ty()),
            Self::Thunk(context) => Some(context.return_ty()),
            Self::OperationHandler(context) => Some(context.return_ty()),
        }
    }

    /// Returns the capture layout of a nested function context, or `None`
    /// for a def context.
    #[must_use]
    pub const fn capture_map_id(&self) -> Option<CaptureMapID> {
        match self {
            Self::Def => None,
            Self::Closure(context) => Some(context.capture_map_id()),
            Self::Thunk(context) => Some(context.capture_map_id()),
            Self::OperationHandler(context) => Some(context.capture_map_id()),
        }
    }
}

impl VisitTypeMut for IRContext {
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, site: TypeSite, visitor: &mut V) {
        match self {
            Self::Def => {}
            Self::Closure(context) => context.visit_types_mut(site, visitor),
            Self::Thunk(context) => context.visit_types_mut(site, visitor),
            Self::OperationHandler(context) => context.visit_types_mut(site, visitor),
        }
    }
}

impl VisitTypeMutAsync for IRContext {
    async fn visit_types_mut_async<V: TypeVisitorMutAsync>(
        &mut self,
        site: TypeSite,
        visitor: &mut V,
    ) {
        match self {
            Self::Def => {}
            Self::Closure(context) => context.visit_types_mut_async(site, visitor).await,
            Self::Thunk(context) => context.visit_types_mut_async(site, visitor).await,
            Self::OperationHandler(context) => context.visit_types_mut_async(site, visitor).await,
        }
    }
}

impl VisitType for IRContext {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        match self {
            Self::Def => {}
            Self::Closure(context) => context.visit_types(site, visitor),
            Self::Thunk(context) => context.visit_types(site, visitor),
            Self::OperationHandler(context) => context.visit_types(site, visitor),
        }
    }
}

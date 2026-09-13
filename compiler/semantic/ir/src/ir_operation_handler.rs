use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{ID, OrderedArena};
use rayc_lexical::tree::RelativeSpan;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::Ty;

use crate::ir_lambda::CaptureMapID;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct IROperationHandlerContext {
    operation: GlobalSymbolID,
    parameters: OperationHandlerParameterMap,
    return_ty: Interned<Ty>,
    capture_map: CaptureMapID,
}

impl IROperationHandlerContext {
    pub(crate) fn new(
        operation: GlobalSymbolID,
        return_ty: Interned<Ty>,
        capture_map: CaptureMapID,
    ) -> Self {
        Self {
            operation,
            parameters: OperationHandlerParameterMap::default(),
            return_ty,
            capture_map,
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
    pub(crate) const fn capture_map(&self) -> CaptureMapID { self.capture_map }
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

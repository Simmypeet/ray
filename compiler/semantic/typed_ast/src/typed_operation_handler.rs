use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{ID, OrderedArena};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::Ty,
};

use crate::name_binding::NameBindingGroupID;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct TypedOperationHandlerContext {
    operation: GlobalSymbolID,
    parameters: TypedOperationHandlerParameterMap,
    parameter_name_binding_group_id: NameBindingGroupID,
    return_type: Interned<Ty>,
}

impl TypedOperationHandlerContext {
    pub(crate) fn new(
        operation: GlobalSymbolID,
        parameter_name_binding_group_id: NameBindingGroupID,
        return_type: Interned<Ty>,
    ) -> Self {
        Self {
            operation,
            parameters: TypedOperationHandlerParameterMap::new(),
            parameter_name_binding_group_id,
            return_type,
        }
    }

    #[must_use]
    pub const fn operation(&self) -> GlobalSymbolID { self.operation }

    #[must_use]
    pub const fn parameter_name_binding_group_id(&self) -> NameBindingGroupID {
        self.parameter_name_binding_group_id
    }

    #[must_use]
    pub const fn return_type(&self) -> &Interned<Ty> { &self.return_type }

    #[must_use]
    pub fn parameters(
        &self,
    ) -> impl ExactSizeIterator<Item = (OperationHandlerParameterID, &TypedOperationHandlerParameter)>
    {
        self.parameters.iter()
    }

    pub(crate) fn insert_parameter(
        &mut self,
        parameter: TypedOperationHandlerParameter,
    ) -> OperationHandlerParameterID {
        self.parameters.insert_parameter(parameter)
    }
}

impl MutSubstitutable for TypedOperationHandlerContext {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.parameters.apply_mut_subst(subst, engine);
        self.return_type.apply_in_place(subst, engine);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct TypedOperationHandlerParameter {
    ty: Interned<Ty>,
    span: RelativeSpan,
}

impl TypedOperationHandlerParameter {
    #[must_use]
    pub const fn new(ty: Interned<Ty>, span: RelativeSpan) -> Self { Self { ty, span } }

    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }
}

impl MutSubstitutable for TypedOperationHandlerParameter {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.ty.apply_in_place(subst, engine);
    }
}

pub type OperationHandlerParameterID = ID<TypedOperationHandlerParameter>;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default)]
struct TypedOperationHandlerParameterMap {
    parameters: OrderedArena<TypedOperationHandlerParameter>,
}

impl TypedOperationHandlerParameterMap {
    fn new() -> Self { Self::default() }

    fn insert_parameter(
        &mut self,
        parameter: TypedOperationHandlerParameter,
    ) -> OperationHandlerParameterID {
        self.parameters.insert(parameter)
    }

    fn iter(
        &self,
    ) -> impl ExactSizeIterator<Item = (OperationHandlerParameterID, &TypedOperationHandlerParameter)>
    {
        self.parameters.iter()
    }
}

impl MutSubstitutable for TypedOperationHandlerParameterMap {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        for (_, parameter) in self.parameters.iter_mut_unordered() {
            parameter.apply_mut_subst(subst, engine);
        }
    }
}

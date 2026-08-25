use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{ID, OrderedArena};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::Ty,
};

use crate::name_binding::NameBindingGroupID;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct TypedLambdaContext {
    parameters: TypedLambdaParameterMap,
    parameter_name_binding_group_id: NameBindingGroupID,
}

impl TypedLambdaContext {
    pub(crate) fn new(parameter_name_binding_group_id: NameBindingGroupID) -> Self {
        Self { parameters: TypedLambdaParameterMap::new(), parameter_name_binding_group_id }
    }

    #[must_use]
    pub const fn parameter_name_binding_group_id(&self) -> NameBindingGroupID {
        self.parameter_name_binding_group_id
    }

    #[must_use]
    pub fn parameters(
        &self,
    ) -> impl ExactSizeIterator<Item = (LambdaParameterID, &TypedLambdaParameter)> {
        self.parameters.iter()
    }

    #[must_use]
    pub fn get_parameter(&self, id: LambdaParameterID) -> &TypedLambdaParameter {
        self.parameters.get_parameter(id)
    }

    pub(crate) fn insert_parameter(
        &mut self,
        parameter: TypedLambdaParameter,
    ) -> LambdaParameterID {
        self.parameters.insert_parameter(parameter)
    }
}

impl MutSubstitutable for TypedLambdaContext {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.parameters.apply_mut_subst(subst, engine);
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable,
)]
pub struct TypedLambdaParameter {
    ty: Interned<Ty>,
    span: RelativeSpan,
}

impl TypedLambdaParameter {
    #[must_use]
    pub const fn new(ty: Interned<Ty>, span: RelativeSpan) -> Self { Self { ty, span } }

    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }
}

impl MutSubstitutable for TypedLambdaParameter {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.ty.apply_in_place(subst, engine);
    }
}

pub type LambdaParameterID = ID<TypedLambdaParameter>;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default, Identifiable)]
pub struct TypedLambdaParameterMap {
    parameters: OrderedArena<TypedLambdaParameter>,
}

impl TypedLambdaParameterMap {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    #[must_use]
    pub fn get_parameter(&self, id: LambdaParameterID) -> &TypedLambdaParameter {
        self.parameters.get(id).expect("LambdaParameterID should be valid")
    }

    pub fn insert_parameter(&mut self, parameter: TypedLambdaParameter) -> LambdaParameterID {
        self.parameters.insert(parameter)
    }

    #[must_use]
    pub fn iter(
        &self,
    ) -> impl ExactSizeIterator<Item = (LambdaParameterID, &TypedLambdaParameter)> {
        self.parameters.iter()
    }
}

impl MutSubstitutable for TypedLambdaParameterMap {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        for (_, parameter) in self.parameters.iter_mut_unordered() {
            parameter.apply_mut_subst(subst, engine);
        }
    }
}

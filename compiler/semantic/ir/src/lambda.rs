use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{ID, OrderedArena};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::ty::{Mutability, Ty};

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct LambdaContext {
    parameters: LambdaParameterMap,
    return_ty: Interned<Ty>,
    captures: CaptureMap,
}

impl LambdaContext {
    pub(crate) fn new(return_ty: Interned<Ty>) -> Self {
        Self {
            parameters: LambdaParameterMap::default(),
            return_ty,
            captures: CaptureMap::default(),
        }
    }

    #[must_use]
    pub fn parameters(
        &self,
    ) -> impl ExactSizeIterator<Item = (LambdaParameterID, &LambdaParameter)> {
        self.parameters.iter()
    }

    #[must_use]
    pub fn get_parameter(&self, id: LambdaParameterID) -> &LambdaParameter {
        self.parameters.get_parameter(id)
    }

    #[must_use]
    pub const fn return_ty(&self) -> &Interned<Ty> { &self.return_ty }

    #[must_use]
    pub(crate) fn insert_parameter(&mut self, parameter: LambdaParameter) -> LambdaParameterID {
        self.parameters.insert_parameter(parameter)
    }

    #[must_use]
    pub fn captures(&self) -> impl ExactSizeIterator<Item = (CaptureID, &Capture)> {
        self.captures.iter()
    }

    #[must_use]
    pub fn get_capture(&self, id: CaptureID) -> &Capture { self.captures.get_capture(id) }

    #[must_use]
    pub(crate) fn insert_capture(&mut self, capture: Capture) -> CaptureID {
        self.captures.insert_capture(capture)
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable,
)]
pub struct LambdaParameter {
    ty: Interned<Ty>,
    span: RelativeSpan,
}

impl LambdaParameter {
    #[must_use]
    pub const fn new(ty: Interned<Ty>, span: RelativeSpan) -> Self { Self { ty, span } }

    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }
}

pub type LambdaParameterID = ID<LambdaParameter>;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default, Identifiable)]
pub struct LambdaParameterMap {
    parameters: OrderedArena<LambdaParameter>,
}

impl LambdaParameterMap {
    #[must_use]
    pub fn get_parameter(&self, id: LambdaParameterID) -> &LambdaParameter {
        self.parameters.get(id).expect("Lambda parameter should exist")
    }

    #[must_use]
    pub(crate) fn insert_parameter(&mut self, parameter: LambdaParameter) -> LambdaParameterID {
        self.parameters.insert(parameter)
    }

    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (LambdaParameterID, &LambdaParameter)> {
        self.parameters.iter()
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable,
)]
pub struct Capture {
    pointee_ty: Interned<Ty>,
    mutability: Mutability,
    span: RelativeSpan,
}

impl Capture {
    #[must_use]
    pub const fn new(pointee_ty: Interned<Ty>, mutability: Mutability, span: RelativeSpan) -> Self {
        Self { pointee_ty, mutability, span }
    }

    #[must_use]
    pub const fn pointee_ty(&self) -> &Interned<Ty> { &self.pointee_ty }

    #[must_use]
    pub const fn mutability(&self) -> Mutability { self.mutability }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }

    #[must_use]
    pub fn pointer_ty(&self, engine: &TrackedEngine) -> Interned<Ty> {
        Ty::new_pointer(self.pointee_ty.clone(), self.mutability, engine)
    }
}

pub type CaptureID = ID<Capture>;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default, Identifiable)]
pub struct CaptureMap {
    captures: OrderedArena<Capture>,
}

impl CaptureMap {
    #[must_use]
    pub fn get_capture(&self, id: CaptureID) -> &Capture {
        self.captures.get(id).expect("Capture should exist")
    }

    #[must_use]
    pub(crate) fn insert_capture(&mut self, capture: Capture) -> CaptureID {
        self.captures.insert(capture)
    }

    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (CaptureID, &Capture)> {
        self.captures.iter()
    }
}

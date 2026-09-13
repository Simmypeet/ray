use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{ID, OrderedArena};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::{capture::CaptureMode, ty::Ty};

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct IRLambdaContext {
    parameters: LambdaParameterMap,
    return_ty: Interned<Ty>,
    capture_map: CaptureMapID,
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct IRThunkContext {
    return_ty: Interned<Ty>,
    capture_map: CaptureMapID,
}

impl IRThunkContext {
    pub(crate) const fn new(return_ty: Interned<Ty>, capture_map: CaptureMapID) -> Self {
        Self { return_ty, capture_map }
    }

    #[must_use]
    pub const fn return_ty(&self) -> &Interned<Ty> { &self.return_ty }

    #[must_use]
    pub(crate) const fn capture_map(&self) -> CaptureMapID { self.capture_map }
}

impl IRLambdaContext {
    pub(crate) fn new(return_ty: Interned<Ty>, capture_map: CaptureMapID) -> Self {
        Self { parameters: LambdaParameterMap::default(), return_ty, capture_map }
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
    pub(crate) const fn capture_map(&self) -> CaptureMapID { self.capture_map }
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
    binding_ty: Interned<Ty>,
    mode: CaptureMode,
    span: RelativeSpan,
}

impl Capture {
    #[must_use]
    pub const fn new(binding_ty: Interned<Ty>, mode: CaptureMode, span: RelativeSpan) -> Self {
        Self { binding_ty, mode, span }
    }

    #[must_use]
    pub const fn binding_ty(&self) -> &Interned<Ty> { &self.binding_ty }

    #[must_use]
    pub const fn mode(&self) -> CaptureMode { self.mode }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }

    #[must_use]
    pub fn storage_ty(&self, engine: &TrackedEngine) -> Interned<Ty> {
        match self.mode {
            CaptureMode::Value => self.binding_ty.clone(),
            CaptureMode::Reference(mutability) => {
                Ty::new_pointer(self.binding_ty.clone(), mutability, engine)
            }
        }
    }
}

pub type CaptureID = ID<Capture>;

/// Identifies one arena-owned capture layout in an IR function map.
pub type CaptureMapID = ID<CaptureMap>;

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

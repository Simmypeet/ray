use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::source_map::to_absolute_span;
use rayc_type::ty::{Ty, application::View as ApplicationView};

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub enum InvalidExternSignatureKind {
    Polymorphic,
    UnitParameter,
    UnsupportedParameter,
    UnsupportedReturn,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct InvalidExternSignature {
    kind: InvalidExternSignatureKind,
    span: RelativeSpan,
}

impl InvalidExternSignature {
    pub(crate) const fn new(kind: InvalidExternSignatureKind, span: RelativeSpan) -> Self {
        Self { kind, span }
    }
}

impl Report for InvalidExternSignature {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let message = match self.kind {
            InvalidExternSignatureKind::Polymorphic => "an extern definition cannot be polymorphic",
            InvalidExternSignatureKind::UnitParameter => {
                "unit is not allowed in an extern parameter"
            }
            InvalidExternSignatureKind::UnsupportedParameter => {
                "this type is not supported in an extern parameter"
            }
            InvalidExternSignatureKind::UnsupportedReturn => {
                "this type is not supported as an extern return type"
            }
        };
        Rendered::builder()
            .message(message)
            .primary_highlight(Highlight::new(engine.to_absolute_span(&self.span).await, None))
            .build()
    }
}

pub(crate) fn is_c_abi_value_type(ty: &Ty) -> bool {
    match ty {
        Ty::Application(application) => match application.view() {
            ApplicationView::Primitive(_) => true,
            ApplicationView::Pointer(pointer) => is_c_abi_value_type(pointer.pointee()),
            ApplicationView::Tuple(_)
            | ApplicationView::Lambda(_)
            | ApplicationView::InstanceAssociated(_)
            | ApplicationView::Instance(_)
            | ApplicationView::Error => false,
        },
        Ty::EffectRow(_) | Ty::Inference(_) | Ty::PolyVar(_) | Ty::SelfInstance(_) => false,
    }
}

pub(crate) fn is_unit_type(ty: &Ty) -> bool {
    matches!(ty, Ty::Application(application) if matches!(application.view(), ApplicationView::Tuple(tuple) if tuple.args().is_empty()))
}

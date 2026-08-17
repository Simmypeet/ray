use qbice::Identifiable;
use rayc_qbice::TrackedEngine;
use rayc_symbol::syntax::get_def_signature_syntax;
use rayc_type::ty::Ty;

use crate::{
    build::{Build, Output},
    register_build,
    ty::resolve_ty,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Identifiable)]
pub enum Never {}

impl qbice::StableHash for Never {
    fn stable_hash<H: qbice::stable_hash::StableHasher + ?Sized>(&self, _: &mut H) {
        match *self {}
    }
}

impl qbice::Encode for Never {
    fn encode<E: qbice::serialize::Encoder + ?Sized>(
        &self,
        _: &mut E,
        _: &qbice::serialize::Plugin,
        _: &mut qbice::serialize::session::Session,
    ) -> std::io::Result<()> {
        match *self {}
    }
}

impl qbice::Decode for Never {
    fn decode<D: qbice::serialize::Decoder + ?Sized>(
        _: &mut D,
        _: &qbice::serialize::Plugin,
        _: &mut qbice::serialize::session::Session,
    ) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Cannot decode a value of type `Never`.",
        ))
    }
}

impl Build for rayc_semantic_element::return_type::Key {
    type Diagnostic = Never;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let (_, Some(return_ty)) = engine.get_def_signature_syntax(symbol_id).await else {
            return Output::new(Ty::new_unit(engine), engine);
        };

        let Some(return_ty) = return_ty.r#type() else {
            return Output::new(Ty::new_error(engine), engine);
        };

        Output::new(engine.resolve_ty(&return_ty), engine)
    }
}

register_build!(rayc_semantic_element::return_type::Key);

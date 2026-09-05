use qbice::{Decode, Encode, StableHash};

use super::{InferenceConstraint, TyKind};
use crate::trait_ref::TraitRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Inference {
    kind: TyKind,
    constraint: InferenceConstraint,
    id: u64,
}

impl Inference {
    #[must_use]
    pub const fn new(kind: TyKind, id: u64) -> Self {
        Self { kind, constraint: InferenceConstraint::Any, id }
    }

    #[must_use]
    pub const fn new_with_constraint(
        kind: TyKind,
        constraint: InferenceConstraint,
        id: u64,
    ) -> Self {
        Self { kind, constraint, id }
    }

    #[must_use]
    pub const fn kind(&self) -> TyKind { self.kind }

    #[must_use]
    pub const fn id(&self) -> u64 { self.id }

    #[must_use]
    pub const fn constraint(&self) -> InferenceConstraint { self.constraint }
}

pub trait GenInfer: Send + Sync {
    fn gen_infer(&mut self, kind: TyKind, constraint: InferenceConstraint) -> Inference;

    fn gen_instance_infer(&mut self, expected_trait_ref: &TraitRef) -> Inference;

    fn gen_effect_row_infer(&mut self) -> Inference {
        self.gen_infer(TyKind::EffectRow, InferenceConstraint::Any)
    }
}

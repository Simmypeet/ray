use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;

use super::{InferenceConstraint, Mutability, Primitive, Ty, TyKind, inference::Inference};
use crate::{
    reduce::Reduce,
    subst::{Subst, Substitutable},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Constant {
    Primitive(Primitive),
    Tuple,
    Lambda,
    Pointer(Mutability),
    Error(TyKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TupleView<'x> {
    args: &'x [Interned<Ty>],
}

impl TupleView<'_> {
    #[must_use]
    pub const fn args(&self) -> &[Interned<Ty>] { self.args }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LambdaView<'x> {
    args: &'x [Interned<Ty>],
}

impl LambdaView<'_> {
    #[must_use]
    pub const fn parameter_types(&self) -> &[Interned<Ty>] {
        let (_, parameter_types) = self.args.split_last().expect("lambda has a return type");
        parameter_types
    }

    #[must_use]
    pub const fn return_type(&self) -> &Interned<Ty> {
        self.args.last().expect("lambda has a return type")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PointerView<'x> {
    arg: &'x Interned<Ty>,
    mutability: Mutability,
}

impl PointerView<'_> {
    #[must_use]
    pub const fn pointee(&self) -> &Interned<Ty> { self.arg }

    #[must_use]
    pub const fn mutability(&self) -> Mutability { self.mutability }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum View<'x> {
    Primitive(Primitive),
    Tuple(TupleView<'x>),
    Lambda(LambdaView<'x>),
    Pointer(PointerView<'x>),
    Error,
}

impl<'x> View<'x> {
    #[must_use]
    pub fn unwrap_into_tuple_view(self) -> TupleView<'x> {
        let Self::Tuple(tuple_view) = self else {
            panic!("Expected View::Tuple, found {self:?}");
        };

        tuple_view
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Application {
    constant: Constant,
    args: Interned<[Interned<Ty>]>,
}

impl Application {
    #[must_use]
    pub(super) const fn new(constant: Constant, args: Interned<[Interned<Ty>]>) -> Self {
        Self { constant, args }
    }

    #[must_use]
    pub fn view(&self) -> View<'_> {
        match self.constant {
            Constant::Primitive(primitive) => View::Primitive(primitive),
            Constant::Tuple => View::Tuple(TupleView { args: &self.args }),
            Constant::Lambda => View::Lambda(LambdaView { args: &self.args }),
            Constant::Pointer(mutability) => {
                View::Pointer(PointerView { arg: &self.args[0], mutability })
            }
            Constant::Error(_) => View::Error,
        }
    }

    #[must_use]
    pub(super) const fn kind_of(&self) -> TyKind {
        match self.constant {
            Constant::Primitive(_) | Constant::Tuple | Constant::Lambda | Constant::Pointer(_) => {
                TyKind::Star
            }
            Constant::Error(kind) => kind,
        }
    }

    #[must_use]
    pub fn satisfies_constraint(&self, con: InferenceConstraint) -> bool {
        match con {
            InferenceConstraint::Any => true,
            InferenceConstraint::Numeric => match self.view() {
                View::Primitive(primitive) => match primitive {
                    Primitive::Int32 | Primitive::Float32 | Primitive::CInt => true,
                    Primitive::Bool | Primitive::CStr => false,
                },

                View::Error | View::Tuple(_) | View::Lambda(_) | View::Pointer(_) => false,
            },
            InferenceConstraint::EqualityComparable => match self.view() {
                View::Primitive(primitive) => match primitive {
                    Primitive::Int32 | Primitive::Float32 | Primitive::Bool | Primitive::CInt => {
                        true
                    }
                    Primitive::CStr => false,
                },

                View::Error | View::Tuple(_) | View::Lambda(_) | View::Pointer(_) => false,
            },
        }
    }

    #[must_use]
    pub fn structural_match<'a>(
        &'a self,
        other: &'a Self,
    ) -> Option<impl Iterator<Item = (&'a Interned<Ty>, &'a Interned<Ty>)>> {
        if self.constant == other.constant && self.args.len() == other.args.len() {
            Some(self.args.iter().zip(other.args.iter()))
        } else {
            None
        }
    }

    pub(super) fn interned_iter(&self) -> impl Iterator<Item = &Interned<Ty>> { self.args.iter() }

    pub(super) fn iter(&self) -> impl Iterator<Item = &Ty> {
        self.args.iter().map(std::convert::AsRef::as_ref)
    }

    #[must_use]
    pub(super) fn has_inference_variable(&self, ty: &Inference) -> bool {
        self.args.iter().any(|arg| arg.has_inference_variable(ty))
    }
}

impl Reduce for Application {
    fn reduce(&self, engine: &TrackedEngine) -> Option<Self> {
        self.args.reduce(engine).map(|args| Self { constant: self.constant, args })
    }
}

impl Substitutable for Application {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        self.args.apply_subst(subst, engine).map(|args| Self { constant: self.constant, args })
    }
}

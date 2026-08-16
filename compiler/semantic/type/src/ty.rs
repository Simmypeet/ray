use std::fmt::{Display, Write};

use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;

use crate::subst::{Subst, Substitutable};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Primitive {
    Int32,
    Float32,
    Bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Mutability {
    Immutable,
    Mutable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum TyConstant {
    Primitive(Primitive),
    Tuple,
    Pointer(Mutability),
    Error,
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
pub enum TyApplicationView<'x> {
    Primitive(Primitive),
    Tuple(TupleView<'x>),
    Pointer(PointerView<'x>),
    Error,
}

impl TyApplication {
    #[must_use]
    pub fn view(&self) -> TyApplicationView<'_> {
        match self.constant {
            TyConstant::Primitive(primitive) => TyApplicationView::Primitive(primitive),
            TyConstant::Tuple => TyApplicationView::Tuple(TupleView { args: &self.args }),
            TyConstant::Pointer(mutability) => {
                TyApplicationView::Pointer(PointerView { arg: &self.args[0], mutability })
            }
            TyConstant::Error => TyApplicationView::Error,
        }
    }

    #[must_use]
    pub fn satisfies_constraint(&self, con: InferenceConstraint) -> bool {
        match con {
            InferenceConstraint::Any => true,
            InferenceConstraint::Numeric => match self.view() {
                TyApplicationView::Primitive(primitive) => match primitive {
                    Primitive::Int32 | Primitive::Float32 => true,
                    Primitive::Bool => false,
                },

                TyApplicationView::Error
                | TyApplicationView::Tuple(_)
                | TyApplicationView::Pointer(_) => false,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct TyApplication {
    constant: TyConstant,
    args: Interned<[Interned<Ty>]>,
}

impl TyApplication {
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum TyKind {
    Star,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum InferenceConstraint {
    Any,
    Numeric,
}

impl InferenceConstraint {
    #[must_use]
    #[allow(clippy::match_same_arms)]
    pub const fn meet(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Any, Self::Any) => Some(Self::Any),
            (Self::Any, Self::Numeric) | (Self::Numeric, Self::Any) => Some(Self::Numeric),
            (Self::Numeric, Self::Numeric) => Some(Self::Numeric),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct TyInference {
    kind: TyKind,
    constraint: InferenceConstraint,
    id: u64,
}

impl TyInference {
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

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Ty {
    Application(TyApplication),
    Inference(TyInference),
}

impl Ty {
    #[must_use]
    pub fn has_inference_variable(&self, ty: &TyInference) -> bool {
        match self {
            Self::Application(ty_application) => {
                ty_application.args.iter().any(|arg| arg.has_inference_variable(ty))
            }
            Self::Inference(ty_inference) => ty_inference == ty,
        }
    }
}

impl Substitutable for Interned<Ty> {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        match &**self {
            Ty::Application(ty_application) => {
                let mut new_vec = None;

                for (i, ty_arg) in ty_application.args.iter().enumerate() {
                    match (new_vec.as_mut(), ty_arg.apply_subst(subst, engine)) {
                        (None, Some(new_ty_arg)) => {
                            let mut vec = ty_application.args.as_ref().to_vec();
                            vec[i] = new_ty_arg;
                            new_vec = Some(vec);
                        }
                        (Some(vec), Some(new_ty_arg)) => {
                            vec[i] = new_ty_arg;
                        }
                        _ => {}
                    }
                }

                new_vec.map(|vec| {
                    let new_args = engine.intern_unsized(vec);
                    let new_ty_application =
                        TyApplication { constant: ty_application.constant, args: new_args };

                    engine.intern(Ty::Application(new_ty_application))
                })
            }

            Ty::Inference(ty_inference) => subst.get(ty_inference).cloned(),
        }
    }
}

impl Ty {
    #[must_use]
    pub fn new_primitive(primitive: Primitive, engine: &TrackedEngine) -> Interned<Self> {
        engine.intern(Self::Application(TyApplication {
            constant: TyConstant::Primitive(primitive),
            args: engine.intern_unsized([]),
        }))
    }

    #[must_use]
    pub fn new_tuple(args: Interned<[Interned<Self>]>, engine: &TrackedEngine) -> Interned<Self> {
        engine.intern(Self::Application(TyApplication { constant: TyConstant::Tuple, args }))
    }

    #[must_use]
    pub fn new_pointer(
        arg: Interned<Self>,
        mutability: Mutability,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        engine.intern(Self::Application(TyApplication {
            constant: TyConstant::Pointer(mutability),
            args: engine.intern_unsized([arg]),
        }))
    }

    #[must_use]
    pub fn new_error(engine: &TrackedEngine) -> Interned<Self> {
        engine.intern(Self::Application(TyApplication {
            constant: TyConstant::Error,
            args: engine.intern_unsized([]),
        }))
    }

    #[must_use]
    pub fn new_unit(engine: &TrackedEngine) -> Interned<Self> {
        engine.intern(Self::Application(TyApplication {
            constant: TyConstant::Tuple,
            args: engine.intern_unsized([]),
        }))
    }
}

impl Display for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Application(ty_application) => match ty_application.view() {
                TyApplicationView::Primitive(primitive) => match primitive {
                    Primitive::Int32 => write!(f, "int32"),
                    Primitive::Float32 => write!(f, "float32"),
                    Primitive::Bool => write!(f, "bool"),
                },
                TyApplicationView::Tuple(tuple) => {
                    f.write_char('(')?;

                    for (i, arg) in tuple.args().iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        write!(f, "{}", **arg)?;
                    }

                    f.write_char(')')
                }
                TyApplicationView::Pointer(pointer) => {
                    f.write_char('*')?;
                    if pointer.mutability() == Mutability::Mutable {
                        f.write_str("mut ")?;
                    }
                    write!(f, "{}", **pointer.pointee())
                }
                TyApplicationView::Error => write!(f, "<error>"),
            },

            Self::Inference(inference) => match inference.constraint {
                InferenceConstraint::Any => write!(f, "{{any}}"),
                InferenceConstraint::Numeric => {
                    write!(f, "{{numeric}}")
                }
            },
        }
    }
}

#[cfg(test)]
mod test;

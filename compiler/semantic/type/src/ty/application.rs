use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;

use super::{InferenceConstraint, Mutability, Primitive, Ty, TyKind, inference::Inference};
use crate::{
    reduce::Reduce,
    subst::{Subst, Substitutable},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Constant {
    Primitive(Primitive),
    Tuple,
    /// A lambda whose arguments are its parameter types, return type, then
    /// effect row.
    Lambda,
    Pointer(Mutability),
    Instance(GlobalSymbolID),
    /// An associated type identified by its `SymbolKind::TraitType` symbol.
    /// Arguments are an instance-kind type followed by the trait type's
    /// polymorphic arguments in declaration order.
    InstanceAssociated(GlobalSymbolID),
    Closure(Closure),
    Error(TyKind),
}

/// Represents an anonymous type created by the lambda expression. It represents
/// the storage that holds all the captured variables required by the lambda.
/// The type is created by the compiler and is not visible to the user.
///
/// The arguments of the closure application type are
/// - A list of parameter types, this can be empty if the lambda has no
///   parameters.
/// - The return type of the lambda.
/// - The effect row of the lambda.
/// - The tuple type of all the captured variables of the lambda. This can be an
///   empty tuple if the lambda has no captured variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Closure {
    /// The relative span of the lambda expression that created this closure
    /// type. This is also used to differentiate between different closure types
    /// created by different lambda expressions since the span is unique to each
    /// lambda expression.
    span: RelativeSpan,
}

impl Closure {
    #[must_use]
    pub const fn new(span: RelativeSpan) -> Self { Self { span } }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClosureView<'x> {
    span: RelativeSpan,
    params: &'x [Interned<Ty>],
    return_type: &'x Interned<Ty>,
    effect_row: &'x Interned<Ty>,
    captured_tuple: &'x Interned<Ty>,
}

impl<'x> ClosureView<'x> {
    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }

    #[must_use]
    pub const fn params(&self) -> &'x [Interned<Ty>] { self.params }

    #[must_use]
    pub const fn return_type(&self) -> &'x Interned<Ty> { self.return_type }

    #[must_use]
    pub const fn effect_row(&self) -> &'x Interned<Ty> { self.effect_row }

    #[must_use]
    pub const fn captured_tuple(&self) -> &'x Interned<Ty> { self.captured_tuple }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TupleView<'x> {
    args: &'x [Interned<Ty>],
}

impl<'x> TupleView<'x> {
    #[must_use]
    pub const fn args(&self) -> &'x [Interned<Ty>] { self.args }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LambdaView<'x> {
    args: &'x [Interned<Ty>],
}

impl<'x> LambdaView<'x> {
    #[must_use]
    pub const fn parameter_types(&self) -> &'x [Interned<Ty>] {
        let (_, signature) = self.args.split_last().expect("lambda has an effect row");
        let (_, parameter_types) = signature.split_last().expect("lambda has a return type");
        parameter_types
    }

    #[must_use]
    pub const fn return_type(&self) -> &'x Interned<Ty> {
        let (_, signature) = self.args.split_last().expect("lambda has an effect row");
        signature.last().expect("lambda has a return type")
    }

    #[must_use]
    pub const fn effect_row(&self) -> &'x Interned<Ty> {
        self.args.last().expect("lambda has an effect row")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PointerView<'x> {
    arg: &'x Interned<Ty>,
    mutability: Mutability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceView<'x> {
    symbol_id: GlobalSymbolID,
    args: &'x [Interned<Ty>],
}

impl<'x> InstanceView<'x> {
    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }

    #[must_use]
    pub const fn args(&self) -> &'x [Interned<Ty>] { self.args }
}

/// A projection of a trait associated type from an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceAssociatedView<'x> {
    symbol_id: GlobalSymbolID,
    instance: &'x Interned<Ty>,
    args: &'x [Interned<Ty>],
}

impl<'x> InstanceAssociatedView<'x> {
    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }

    #[must_use]
    pub const fn instance(&self) -> &'x Interned<Ty> { self.instance }

    /// The associated type's polymorphic arguments, excluding the instance.
    #[must_use]
    pub const fn args(&self) -> &'x [Interned<Ty>] { self.args }
}

impl<'x> PointerView<'x> {
    #[must_use]
    pub const fn pointee(&self) -> &'x Interned<Ty> { self.arg }

    #[must_use]
    pub const fn mutability(&self) -> Mutability { self.mutability }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum View<'x> {
    Primitive(Primitive),
    Tuple(TupleView<'x>),
    Lambda(LambdaView<'x>),
    Pointer(PointerView<'x>),
    Instance(InstanceView<'x>),
    InstanceAssociated(InstanceAssociatedView<'x>),
    Closure(ClosureView<'x>),
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
            Constant::Instance(symbol_id) => {
                View::Instance(InstanceView { symbol_id, args: &self.args })
            }
            Constant::InstanceAssociated(symbol_id) => {
                let (instance, args) =
                    self.args.split_first().expect("associated type has an instance");
                View::InstanceAssociated(InstanceAssociatedView { symbol_id, instance, args })
            }
            Constant::Closure(closure) => {
                let tuple_index = self.args.len() - 1;
                let effect_index = self.args.len() - 2;
                let return_index = self.args.len() - 3;

                let params = &self.args[..return_index];
                let return_type = &self.args[return_index];
                let effect_row = &self.args[effect_index];
                let captured_tuple = &self.args[tuple_index];

                View::Closure(ClosureView {
                    params,
                    return_type,
                    effect_row,
                    captured_tuple,
                    span: closure.span,
                })
            }
            Constant::Error(_) => View::Error,
        }
    }

    #[must_use]
    pub const fn is_instance_associated(&self) -> bool {
        matches!(self.constant, Constant::InstanceAssociated(_))
    }

    #[must_use]
    pub(crate) async fn kind_of(&self, engine: &TrackedEngine) -> TyKind {
        match self.constant {
            Constant::Closure(_)
            | Constant::Primitive(_)
            | Constant::Tuple
            | Constant::Lambda
            | Constant::Pointer(_) => TyKind::Star,

            Constant::InstanceAssociated(symbol_id) => {
                use crate::associated_type_kind::get_associated_type_kind;
                engine.get_associated_type_kind(symbol_id).await
            }
            Constant::Instance(_) => TyKind::Instance,
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

                View::Error
                | View::Tuple(_)
                | View::Lambda(_)
                | View::Pointer(_)
                | View::Instance(_)
                | View::Closure(_)
                | View::InstanceAssociated(_) => false,
            },
            InferenceConstraint::EqualityComparable => match self.view() {
                View::Primitive(primitive) => match primitive {
                    Primitive::Int32 | Primitive::Float32 | Primitive::Bool | Primitive::CInt => {
                        true
                    }
                    Primitive::CStr => false,
                },

                View::Error
                | View::Tuple(_)
                | View::Lambda(_)
                | View::Pointer(_)
                | View::Instance(_)
                | View::InstanceAssociated(_)
                | View::Closure(_) => false,
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
    async fn reduce(
        &self,
        engine: &TrackedEngine,
        givens: &[crate::where_clause::PredicateKind],
    ) -> Option<Self> {
        self.args.reduce(engine, givens).await.map(|args| Self { constant: self.constant, args })
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

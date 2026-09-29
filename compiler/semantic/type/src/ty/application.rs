use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::ID;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;

use super::{InferenceConstraint, Mutability, Primitive, Ty, TyKind, inference::Inference};
use crate::{
    poly_var::build_subst_from_args,
    reduce::Reduce,
    rewrite::{Rewrite, RewriteAsync, TyRewriter, TyRewriterAsync},
    subst::{Subst, Substitutable},
    variance::{Variance, VarianceMap, get_variance},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Constant {
    Primitive(Primitive),
    Tuple,
    Pointer(Mutability),
    /// A checked reference. Its arguments are the lifetime followed by the
    /// referenced type.
    Reference(Mutability),
    /// A nominal struct identified by its `SymbolKind::Strut` symbol.
    Struct(GlobalSymbolID),
    Instance(GlobalSymbolID),
    /// An associated type identified by its `SymbolKind::TraitType` symbol.
    /// Arguments are an instance-kind type followed by the trait type's
    /// polymorphic arguments in declaration order.
    InstanceAssociated(GlobalSymbolID),
    Closure(Closure),
    /// The built-in `Def` dictionary whose sole argument is a closure type.
    DefInstance,
    /// The built-in no-op `Drop` dictionary for primitives, pointers,
    /// references, and `core.NoDrop[t]`.
    NoOpDropInstance,
    /// The built-in `Drop` dictionary for a tuple. Its arguments are the tuple
    /// type followed by one `Drop` dictionary for each element in tuple order.
    TupleDropInstance,
    /// The built-in `Drop` dictionary for a closure. Its arguments are the
    /// closure type followed by one `Drop` dictionary for each capture in
    /// environment order.
    ClosureDropInstance,
    /// The generated `Drop` dictionary for a nominal type. Its arguments are
    /// the instantiated nominal type followed by its external dictionaries in
    /// `GeneratedDropPlan::requirements` order.
    NominalDropInstance,
    Error(TyKind),
}

/// Represents an anonymous type created by the lambda expression. It represents
/// the storage that holds all the captured variables required by the lambda.
/// The type is created by the compiler and is not visible to the user.
///
/// The arguments of the closure application type are
/// - The owner's polymorphic arguments in declaration order, followed by those
///   of each enclosing symbol, proceeding outwards.
/// - A list of parameter types, this can be empty if the lambda has no
///   parameters.
/// - The return type of the lambda.
/// - The effect row of the lambda.
/// - The tuple type of all the captured variables of the lambda. This can be an
///   empty tuple if the lambda has no captured variables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Closure {
    /// The source definition containing this closure, including nested lambdas.
    owner_id: GlobalSymbolID,
    local_closure_id: ClosureID,
    /// Separates owner arguments from parameters without querying the owner.
    owner_argument_count: usize,
}

/// Identifies a nominal closure within its owning source definition.
pub type ClosureID = ID<Closure>;

impl Closure {
    #[must_use]
    pub const fn new(
        owner_id: GlobalSymbolID,
        local_closure_id: ClosureID,
        owner_argument_count: usize,
    ) -> Self {
        Self { owner_id, local_closure_id, owner_argument_count }
    }

    #[must_use]
    pub const fn owner_id(&self) -> GlobalSymbolID { self.owner_id }

    #[must_use]
    pub const fn local_closure_id(&self) -> ClosureID { self.local_closure_id }

    #[must_use]
    pub const fn owner_argument_count(&self) -> usize { self.owner_argument_count }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClosureView<'x> {
    closure: Closure,
    owner_arguments: &'x [Interned<Ty>],
    params: &'x [Interned<Ty>],
    return_type: &'x Interned<Ty>,
    effect_row: &'x Interned<Ty>,
    captured_tuple: &'x Interned<Ty>,
}

impl<'x> ClosureView<'x> {
    #[must_use]
    pub const fn owner_id(&self) -> GlobalSymbolID { self.closure.owner_id }

    #[must_use]
    pub const fn local_closure_id(&self) -> ClosureID { self.closure.local_closure_id }

    #[must_use]
    pub const fn owner_arguments(&self) -> &'x [Interned<Ty>] { self.owner_arguments }

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
pub struct PointerView<'x> {
    arg: &'x Interned<Ty>,
    mutability: Mutability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReferenceView<'x> {
    lifetime: &'x Interned<Ty>,
    pointee: &'x Interned<Ty>,
    mutability: Mutability,
}

impl<'x> ReferenceView<'x> {
    #[must_use]
    pub const fn lifetime(&self) -> &'x Interned<Ty> { self.lifetime }

    #[must_use]
    pub const fn pointee(&self) -> &'x Interned<Ty> { self.pointee }

    #[must_use]
    pub const fn mutability(&self) -> Mutability { self.mutability }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StructView<'x> {
    symbol_id: GlobalSymbolID,
    args: &'x [Interned<Ty>],
}

impl<'x> StructView<'x> {
    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }

    #[must_use]
    pub const fn args(&self) -> &'x [Interned<Ty>] { self.args }

    #[must_use]
    pub async fn create_subst(&self, engine: &TrackedEngine) -> Subst {
        engine.build_subst_from_args(self.symbol_id, self.args).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceView<'x> {
    symbol_id: GlobalSymbolID,
    args: &'x [Interned<Ty>],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TupleDropInstanceView<'x> {
    tuple: &'x Interned<Ty>,
    element_instances: &'x [Interned<Ty>],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClosureDropInstanceView<'x> {
    closure: &'x Interned<Ty>,
    capture_instances: &'x [Interned<Ty>],
}

impl<'x> ClosureDropInstanceView<'x> {
    #[must_use]
    pub const fn closure(&self) -> &'x Interned<Ty> { self.closure }

    #[must_use]
    pub const fn capture_instances(&self) -> &'x [Interned<Ty>] { self.capture_instances }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NominalDropInstanceView<'x> {
    nominal: &'x Interned<Ty>,
    external_instances: &'x [Interned<Ty>],
}

impl<'x> NominalDropInstanceView<'x> {
    #[must_use]
    pub const fn nominal(&self) -> &'x Interned<Ty> { self.nominal }

    #[must_use]
    pub const fn external_instances(&self) -> &'x [Interned<Ty>] { self.external_instances }
}

impl<'x> TupleDropInstanceView<'x> {
    #[must_use]
    pub const fn tuple(&self) -> &'x Interned<Ty> { self.tuple }

    #[must_use]
    pub const fn element_instances(&self) -> &'x [Interned<Ty>] { self.element_instances }
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
    Pointer(PointerView<'x>),
    Reference(ReferenceView<'x>),
    Struct(StructView<'x>),
    Instance(InstanceView<'x>),
    InstanceAssociated(InstanceAssociatedView<'x>),
    Closure(ClosureView<'x>),
    DefInstance(&'x Interned<Ty>),
    NoOpDropInstance(&'x Interned<Ty>),
    TupleDropInstance(TupleDropInstanceView<'x>),
    ClosureDropInstance(ClosureDropInstanceView<'x>),
    NominalDropInstance(NominalDropInstanceView<'x>),
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
            Constant::Pointer(mutability) => {
                View::Pointer(PointerView { arg: &self.args[0], mutability })
            }
            Constant::Reference(mutability) => View::Reference(ReferenceView {
                lifetime: &self.args[0],
                pointee: &self.args[1],
                mutability,
            }),
            Constant::Struct(symbol_id) => View::Struct(StructView { symbol_id, args: &self.args }),
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

                let (owner_arguments, params) =
                    self.args[..return_index].split_at(closure.owner_argument_count);
                let return_type = &self.args[return_index];
                let effect_row = &self.args[effect_index];
                let captured_tuple = &self.args[tuple_index];

                View::Closure(ClosureView {
                    closure,
                    owner_arguments,
                    params,
                    return_type,
                    effect_row,
                    captured_tuple,
                })
            }
            Constant::DefInstance => View::DefInstance(&self.args[0]),
            Constant::NoOpDropInstance => View::NoOpDropInstance(&self.args[0]),
            Constant::TupleDropInstance => {
                let (tuple, element_instances) =
                    self.args.split_first().expect("tuple Drop instance has a tuple type");
                View::TupleDropInstance(TupleDropInstanceView { tuple, element_instances })
            }
            Constant::ClosureDropInstance => {
                let (closure, capture_instances) =
                    self.args.split_first().expect("closure Drop instance has a closure type");
                View::ClosureDropInstance(ClosureDropInstanceView { closure, capture_instances })
            }
            Constant::NominalDropInstance => {
                let (nominal, external_instances) =
                    self.args.split_first().expect("nominal Drop instance has a nominal type");
                View::NominalDropInstance(NominalDropInstanceView { nominal, external_instances })
            }
            Constant::Error(_) => View::Error,
        }
    }

    #[must_use]
    pub(super) fn has_same_constant(&self, other: &Self) -> bool {
        self.constant == other.constant && self.args.len() == other.args.len()
    }

    /// Returns the struct this application instantiates, if it is one.
    #[must_use]
    pub const fn struct_id(&self) -> Option<GlobalSymbolID> {
        match self.constant {
            Constant::Struct(symbol_id) => Some(symbol_id),
            Constant::Primitive(_)
            | Constant::Tuple
            | Constant::Pointer(_)
            | Constant::Reference(_)
            | Constant::Instance(_)
            | Constant::InstanceAssociated(_)
            | Constant::Closure(_)
            | Constant::DefInstance
            | Constant::NoOpDropInstance
            | Constant::TupleDropInstance
            | Constant::ClosureDropInstance
            | Constant::NominalDropInstance
            | Constant::Error(_) => None,
        }
    }

    /// Returns each argument with the variance of its position.
    ///
    /// `struct_variances` are the variances of the struct's parameters when
    /// this application is a struct (see [`Self::struct_id`]), and are ignored
    /// otherwise.
    ///
    /// # Panics
    ///
    /// If this application is a struct and `struct_variances` is `None` or
    /// has fewer variances than there are arguments.
    pub fn arguments_with_variance<'a>(
        &'a self,
        struct_variances: Option<&'a VarianceMap>,
    ) -> impl Iterator<Item = (&'a Interned<Ty>, Variance)> {
        self.args
            .iter()
            .enumerate()
            .map(move |(index, arg)| (arg, self.argument_variance(index, struct_variances)))
    }

    /// Returns each argument with the variance of its position, when this
    /// application itself is in a position of variance `ambient`.
    ///
    /// An invariant or bivariant position absorbs every variance inside it,
    /// so the variances of a struct are only queried when `ambient` is
    /// covariant or contravariant.
    pub async fn arguments_with_ambient_variance(
        &self,
        ambient: Variance,
        engine: &TrackedEngine,
    ) -> impl Iterator<Item = (&Interned<Ty>, Variance)> {
        let struct_variances = match (ambient, self.struct_id()) {
            (Variance::Covariant | Variance::Contravariant, Some(struct_id)) => {
                Some(engine.get_variance(struct_id).await)
            }
            (Variance::Covariant | Variance::Contravariant, None)
            | (Variance::Invariant | Variance::Bivariant, _) => None,
        };

        self.args.iter().enumerate().map(move |(index, arg)| {
            let variance = match ambient {
                Variance::Invariant | Variance::Bivariant => ambient,
                Variance::Covariant | Variance::Contravariant => {
                    ambient.xform(self.argument_variance(index, struct_variances.as_deref()))
                }
            };
            (arg, variance)
        })
    }

    /// Returns the same type constructor applied to `args`.
    ///
    /// # Panics
    ///
    /// If `args` does not have as many arguments as this application.
    #[must_use]
    pub fn with_arguments(&self, args: Interned<[Interned<Ty>]>) -> Self {
        assert_eq!(args.len(), self.args.len(), "a type constructor keeps its arity");
        Self { constant: self.constant, args }
    }

    /// Returns the variance of the argument position `index`; see
    /// [`Self::arguments_with_variance`].
    fn argument_variance(&self, index: usize, struct_variances: Option<&VarianceMap>) -> Variance {
        match self.constant {
            Constant::Tuple => Variance::Covariant,

            // Raw pointers are unchecked, so their pointee is never related.
            Constant::Pointer(_) => Variance::Bivariant,

            // The lifetime comes first and is always covariant.
            Constant::Reference(mutability) => {
                if index == 0 {
                    Variance::Covariant
                } else {
                    mutability.pointee_variance()
                }
            }

            Constant::Struct(_) => struct_variances
                .expect("a struct application needs the variances of its struct")
                .get_by_index(index),

            // Closure types are invariant, as in rustc, and so are
            // dictionaries and associated type projections. Primitives and
            // errors have no arguments to relate.
            Constant::Primitive(_)
            | Constant::Instance(_)
            | Constant::InstanceAssociated(_)
            | Constant::Closure(_)
            | Constant::DefInstance
            | Constant::NoOpDropInstance
            | Constant::TupleDropInstance
            | Constant::ClosureDropInstance
            | Constant::NominalDropInstance
            | Constant::Error(_) => Variance::Invariant,
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
            | Constant::Pointer(_)
            | Constant::Reference(_)
            | Constant::Struct(_) => TyKind::Star,

            Constant::InstanceAssociated(symbol_id) => {
                use crate::associated_type_kind::get_associated_type_kind;
                engine.get_associated_type_kind(symbol_id).await
            }

            Constant::NoOpDropInstance
            | Constant::TupleDropInstance
            | Constant::ClosureDropInstance
            | Constant::NominalDropInstance
            | Constant::Instance(_)
            | Constant::DefInstance => TyKind::Instance,
            Constant::Error(kind) => kind,
        }
    }

    #[must_use]
    pub fn satisfies_constraint(&self, con: InferenceConstraint) -> bool {
        match con {
            InferenceConstraint::Any => true,
            InferenceConstraint::Numeric => match self.view() {
                View::Primitive(primitive) => match primitive {
                    Primitive::Int8
                    | Primitive::Int16
                    | Primitive::Int32
                    | Primitive::Int64
                    | Primitive::Isize
                    | Primitive::Uint8
                    | Primitive::Uint16
                    | Primitive::Uint32
                    | Primitive::Uint64
                    | Primitive::Usize
                    | Primitive::Float32
                    | Primitive::CInt => true,
                    Primitive::Bool | Primitive::CStr => false,
                },

                View::Error
                | View::Tuple(_)
                | View::Pointer(_)
                | View::Reference(_)
                | View::Struct(_)
                | View::Instance(_)
                | View::DefInstance(_)
                | View::Closure(_)
                | View::NoOpDropInstance(_)
                | View::TupleDropInstance(_)
                | View::ClosureDropInstance(_)
                | View::NominalDropInstance(_)
                | View::InstanceAssociated(_) => false,
            },
            InferenceConstraint::EqualityComparable => match self.view() {
                View::Primitive(primitive) => match primitive {
                    Primitive::Int8
                    | Primitive::Int16
                    | Primitive::Int32
                    | Primitive::Int64
                    | Primitive::Isize
                    | Primitive::Uint8
                    | Primitive::Uint16
                    | Primitive::Uint32
                    | Primitive::Uint64
                    | Primitive::Usize
                    | Primitive::Float32
                    | Primitive::Bool
                    | Primitive::CInt => true,
                    Primitive::CStr => false,
                },

                View::Error
                | View::Tuple(_)
                | View::Pointer(_)
                | View::Reference(_)
                | View::Struct(_)
                | View::Instance(_)
                | View::DefInstance(_)
                | View::InstanceAssociated(_)
                | View::NoOpDropInstance(_)
                | View::TupleDropInstance(_)
                | View::ClosureDropInstance(_)
                | View::NominalDropInstance(_)
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

    pub(crate) fn interned_iter(&self) -> impl Iterator<Item = &Interned<Ty>> { self.args.iter() }

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
    ) -> Option<(Self, crate::constraint::outlives::OutlivesConstraints)> {
        self.args
            .reduce(engine, givens)
            .await
            .map(|(args, outlives)| (Self { constant: self.constant, args }, outlives))
    }
}

impl Rewrite for Application {
    fn rewrite(&self, rewriter: &mut impl TyRewriter, engine: &TrackedEngine) -> Option<Self> {
        self.args.rewrite(rewriter, engine).map(|args| Self { constant: self.constant, args })
    }
}

impl RewriteAsync for Application {
    async fn rewrite_async(
        &self,
        rewriter: &mut impl TyRewriterAsync,
        engine: &TrackedEngine,
    ) -> Option<Self> {
        self.args
            .rewrite_async(rewriter, engine)
            .await
            .map(|args| Self { constant: self.constant, args })
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

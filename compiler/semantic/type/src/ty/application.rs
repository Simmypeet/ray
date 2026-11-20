use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::ID;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
    member::get_member_by_name,
    name::get_name,
    symbol_kind::{SymbolKind, get_symbol_kind},
};

use super::{
    InferenceConstraint, Mutability, Primitive, Ty, TyKind, inference::Inference,
    self_instance::SelfInstance,
};
use crate::{
    fold::{TyFoldable, TyFolder},
    instance_member::get_instance_member,
    poly_var::{
        GlobalPolyVarID, build_subst_from_args, get_enclosing_poly_var_maps, get_poly_var_map,
    },
    signature::{get_effect_row, get_return_type},
    subst::{Subst, Substitutable},
    type_definition::get_type_definition,
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
    /// A nominal enum identified by its `SymbolKind::Enum` symbol.
    Enum(GlobalSymbolID),
    Instance(GlobalSymbolID),
    /// An associated type, applied to an instance followed by the trait type's
    /// own arguments.
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
    /// The built-in `Drop` dictionary for a closure, applied to the closure
    /// type and one `Drop` dictionary per capture.
    ClosureDropInstance,
    /// The generated `Drop` dictionary for a struct or an enum, applied to the
    /// type and its external dictionaries.
    GeneratedAdtDropInstance,
    /// The anonymous type of a suspended function, one per function rather than
    /// per `coro` expression so its parts derive from the signature.
    DefCoroutine(GlobalSymbolID),
    /// The coroutine of a call through a not-yet-known instance, kept as a
    /// projection that reduces to [`Self::DefCoroutine`] once known.
    TraitDefCoroutine(GlobalSymbolID),
    /// The anonymous type of a suspended closure call, which a
    /// [`Self::TraitDefCoroutine`] through a closure's `Def` dictionary reduces
    /// to.
    ClosureCoroutine,
    /// The built-in `Coroutine` dictionary whose sole argument is a coroutine
    /// type, or a `&mut` to one.
    CoroutineInstance,
    /// The built-in `Def` dictionary that makes `core.Continuation[r, c]`
    /// callable.
    ContinuationDefInstance,
    /// The built-in `Drop` dictionary for a coroutine type, applied to the
    /// coroutine type and one `Drop` dictionary per upvar.
    CoroutineDropInstance,
    Error(TyKind),
}

/// The compiler-generated storage type of a closure's captures, which also
/// carries its owner's arguments and its signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Closure {
    /// The source definition containing this closure, including nested
    /// closures.
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
    pub async fn create_subst(&self, engine: &TrackedEngine) -> Subst {
        let poly_vars = engine.get_enclosing_poly_var_maps(self.closure.owner_id).await;
        poly_vars.create_subst_from_args(self.owner_arguments.iter().cloned())
    }

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
pub struct EnumView<'x> {
    symbol_id: GlobalSymbolID,
    args: &'x [Interned<Ty>],
}

impl<'x> EnumView<'x> {
    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }

    #[must_use]
    pub const fn args(&self) -> &'x [Interned<Ty>] { self.args }

    #[must_use]
    pub async fn create_subst(&self, engine: &TrackedEngine) -> Subst {
        engine.build_subst_from_args(self.symbol_id, self.args).await
    }
}

/// A struct or an enum, for what does not depend on which one it is: the
/// declaration and its type arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdtView<'x> {
    Struct(StructView<'x>),
    Enum(EnumView<'x>),
}

impl<'x> AdtView<'x> {
    /// Returns the `SymbolKind::Strut` or `SymbolKind::Enum` symbol.
    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID {
        match self {
            Self::Struct(struct_) => struct_.symbol_id(),
            Self::Enum(enum_) => enum_.symbol_id(),
        }
    }

    #[must_use]
    pub const fn args(&self) -> &'x [Interned<Ty>] {
        match self {
            Self::Struct(struct_) => struct_.args(),
            Self::Enum(enum_) => enum_.args(),
        }
    }

    /// Returns the substitution that instantiates the declaration with the
    /// type arguments.
    #[must_use]
    pub async fn create_subst(&self, engine: &TrackedEngine) -> Subst {
        engine.build_subst_from_args(self.symbol_id(), self.args()).await
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

/// The type of a suspended function; see [`Constant::DefCoroutine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DefCoroutineView<'x> {
    def_id: GlobalSymbolID,
    poly_args: &'x [Interned<Ty>],
}

impl<'x> DefCoroutineView<'x> {
    /// Returns the function the coroutine suspends.
    #[must_use]
    pub const fn def_id(&self) -> GlobalSymbolID { self.def_id }

    /// Returns the arguments for the polymorphic parameters of the suspended
    /// function, followed by those of each enclosing symbol.
    #[must_use]
    pub const fn poly_args(&self) -> &'x [Interned<Ty>] { self.poly_args }

    /// Returns the substitution that instantiates the signature of the
    /// suspended function with the arguments of this coroutine type.
    #[must_use]
    pub async fn create_subst(&self, engine: &TrackedEngine) -> Subst {
        let poly_vars = engine.get_enclosing_poly_var_maps(self.def_id).await;

        poly_vars.create_subst_from_args(self.poly_args.iter().cloned())
    }
}

/// The type of a suspended call through an instance; see
/// [`Constant::TraitDefCoroutine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TraitDefCoroutineView<'x> {
    trait_def_id: GlobalSymbolID,
    instance: &'x Interned<Ty>,
    args: &'x [Interned<Ty>],
}

impl<'x> TraitDefCoroutineView<'x> {
    /// Returns the trait method the suspended call dispatches to.
    #[must_use]
    pub const fn trait_def_id(&self) -> GlobalSymbolID { self.trait_def_id }

    /// Returns the instance the suspended call dispatches through.
    #[must_use]
    pub const fn instance(&self) -> &'x Interned<Ty> { self.instance }

    /// Returns the arguments for the method's own polymorphic parameters.
    #[must_use]
    pub const fn args(&self) -> &'x [Interned<Ty>] { self.args }

    /// Returns the substitution that instantiates the method's own
    /// polymorphic parameters with [`Self::args`].
    pub async fn create_own_subst(&self, engine: &TrackedEngine) -> Subst {
        engine.build_subst_from_args(self.trait_def_id, self.args.iter()).await
    }

    /// Returns the substitution that instantiates the trait method's signature
    /// for this call.
    pub async fn create_subst(&self, engine: &TrackedEngine) -> Subst {
        let mut subst = self.create_own_subst(engine).await;
        let Ok(trait_ref) = self.instance.instance_trait_ref(engine).await else {
            return subst;
        };

        let trait_id = trait_ref.trait_id();
        let trait_subst =
            engine.build_subst_from_args(trait_id, trait_ref.args().interned_iter()).await;
        for (poly_var_id, argument) in trait_subst.poly_var_mappings() {
            subst.insert(poly_var_id, argument.clone());
        }
        subst.insert(SelfInstance::new(trait_id), self.instance.clone());

        subst
    }
}

/// The type of a suspended call of a closure; see
/// [`Constant::ClosureCoroutine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClosureCoroutineView<'x> {
    closure_ty: &'x Interned<Ty>,
    closure_view: ClosureView<'x>,
}

impl<'x> ClosureCoroutineView<'x> {
    /// Returns the type of the suspended closure.
    #[must_use]
    pub const fn closure(&self) -> &'x Interned<Ty> { self.closure_ty }

    /// Returns the view of the suspended closure type.
    #[must_use]
    pub const fn closure_view(&self) -> ClosureView<'x> { self.closure_view }

    /// Returns the substitution that instantiates the signature of
    /// `core.Def.call` for this closure.
    pub async fn create_subst(&self, engine: &TrackedEngine) -> Subst {
        let def_trait = engine.get_core_item(CoreItem::DefTrait).await;
        let mut subst = engine.build_subst_from_args(def_trait, [self.closure_ty]).await;
        subst.insert(
            SelfInstance::new(def_trait),
            Ty::new_def_instance(self.closure_ty.clone(), engine),
        );

        subst
    }
}

/// A coroutine type of any kind, for what does not depend on how the
/// suspended function is known: its signature and its upvars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CoroutineView<'x> {
    /// The suspended function is known; see [`Constant::DefCoroutine`].
    Def(DefCoroutineView<'x>),

    /// The suspended call dispatches through an instance; see
    /// [`Constant::TraitDefCoroutine`].
    TraitDef(TraitDefCoroutineView<'x>),

    /// The suspended call is of a closure; see
    /// [`Constant::ClosureCoroutine`].
    Closure(ClosureCoroutineView<'x>),
}

impl CoroutineView<'_> {
    /// Returns the symbol whose signature the coroutine runs.
    pub async fn signature_id(&self, engine: &TrackedEngine) -> GlobalSymbolID {
        match self {
            Self::Def(coroutine) => coroutine.def_id(),
            Self::TraitDef(coroutine) => coroutine.trait_def_id(),
            Self::Closure(_) => engine.get_core_item(CoreItem::DefCall).await,
        }
    }

    /// Returns the substitution that instantiates the signature of
    /// [`Self::signature_id`].
    pub async fn create_subst(&self, engine: &TrackedEngine) -> Subst {
        match self {
            Self::Def(coroutine) => coroutine.create_subst(engine).await,
            Self::TraitDef(coroutine) => coroutine.create_subst(engine).await,
            Self::Closure(coroutine) => coroutine.create_subst(engine).await,
        }
    }

    /// Returns the substitution for the type parameters chosen by the `coro`
    /// that created this coroutine.
    pub async fn instantiation(&self, engine: &TrackedEngine) -> Subst {
        match self {
            Self::Def(coroutine) => coroutine.create_subst(engine).await,
            Self::TraitDef(coroutine) => coroutine.create_own_subst(engine).await,
            Self::Closure(_) => Subst::new_empty(),
        }
    }
}

/// A coroutine type whose suspended function is known, which is what a
/// coroutine type reduces to once it is fully concrete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConcreteCoroutineView<'x> {
    /// See [`Constant::DefCoroutine`].
    Def(DefCoroutineView<'x>),

    /// See [`Constant::ClosureCoroutine`].
    Closure(ClosureCoroutineView<'x>),
}

impl<'x> From<ConcreteCoroutineView<'x>> for CoroutineView<'x> {
    fn from(view: ConcreteCoroutineView<'x>) -> Self {
        match view {
            ConcreteCoroutineView::Def(coroutine) => Self::Def(coroutine),
            ConcreteCoroutineView::Closure(coroutine) => Self::Closure(coroutine),
        }
    }
}

impl ConcreteCoroutineView<'_> {
    #[must_use]
    pub async fn signature_id(&self, engine: &TrackedEngine) -> GlobalSymbolID {
        CoroutineView::from(*self).signature_id(engine).await
    }

    /// Returns the def ID that owns this closure body
    #[must_use]
    pub const fn owner_def_id(&self) -> GlobalSymbolID {
        match self {
            Self::Def(coroutine) => coroutine.def_id(),
            Self::Closure(coroutine) => coroutine.closure_view().owner_id(),
        }
    }

    /// See [`CoroutineView::create_subst`].
    pub async fn create_subst(&self, engine: &TrackedEngine) -> Subst {
        CoroutineView::from(*self).create_subst(engine).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CoroutineDropInstanceView<'x> {
    coroutine: &'x Interned<Ty>,
    upvar_instances: &'x [Interned<Ty>],
}

impl<'x> CoroutineDropInstanceView<'x> {
    #[must_use]
    pub const fn coroutine(&self) -> &'x Interned<Ty> { self.coroutine }

    /// Returns the `Drop` dictionary of each upvar, in parameter order.
    #[must_use]
    pub const fn upvar_instances(&self) -> &'x [Interned<Ty>] { self.upvar_instances }
}

/// The `Def` dictionary of a continuation; see
/// [`Constant::ContinuationDefInstance`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContinuationDefInstanceView<'x> {
    continuation: &'x Interned<Ty>,
    coroutine_instance: &'x Interned<Ty>,
}

impl<'x> ContinuationDefInstanceView<'x> {
    /// Returns the `core.Continuation[r, c]` type the dictionary is for.
    #[must_use]
    pub const fn continuation(&self) -> &'x Interned<Ty> { self.continuation }

    /// Returns the `Coroutine` dictionary of the coroutine the continuation
    /// resumes.
    #[must_use]
    pub const fn coroutine_instance(&self) -> &'x Interned<Ty> { self.coroutine_instance }

    /// Returns the type the continuation waits for: the `r` of
    /// `core.Continuation[r, c]`.
    #[must_use]
    pub fn resume_ty(&self) -> &'x Interned<Ty> {
        &self
            .continuation
            .as_struct_view()
            .expect("a continuation `Def` instance is built for a continuation type")
            .args()[0]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GeneratedAdtDropInstanceView<'x> {
    nominal: &'x Interned<Ty>,
    external_instances: &'x [Interned<Ty>],
}

impl<'x> GeneratedAdtDropInstanceView<'x> {
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
    Enum(EnumView<'x>),
    Instance(InstanceView<'x>),
    InstanceAssociated(InstanceAssociatedView<'x>),
    Closure(ClosureView<'x>),
    DefInstance(&'x Interned<Ty>),
    NoOpDropInstance(&'x Interned<Ty>),
    TupleDropInstance(TupleDropInstanceView<'x>),
    ClosureDropInstance(ClosureDropInstanceView<'x>),
    GeneratedAdtDropInstance(GeneratedAdtDropInstanceView<'x>),
    DefCoroutine(DefCoroutineView<'x>),
    TraitDefCoroutine(TraitDefCoroutineView<'x>),
    ClosureCoroutine(ClosureCoroutineView<'x>),
    CoroutineInstance(&'x Interned<Ty>),
    ContinuationDefInstance(ContinuationDefInstanceView<'x>),
    CoroutineDropInstance(CoroutineDropInstanceView<'x>),
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
            Constant::Enum(symbol_id) => View::Enum(EnumView { symbol_id, args: &self.args }),
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
            Constant::GeneratedAdtDropInstance => {
                let (nominal, external_instances) =
                    self.args.split_first().expect("nominal Drop instance has a nominal type");
                View::GeneratedAdtDropInstance(GeneratedAdtDropInstanceView {
                    nominal,
                    external_instances,
                })
            }
            Constant::DefCoroutine(def_id) => {
                View::DefCoroutine(DefCoroutineView { def_id, poly_args: &self.args })
            }
            Constant::TraitDefCoroutine(trait_def_id) => {
                let (instance, args) =
                    self.args.split_first().expect("a trait-def coroutine has an instance");
                View::TraitDefCoroutine(TraitDefCoroutineView { trait_def_id, instance, args })
            }
            Constant::ClosureCoroutine => View::ClosureCoroutine(ClosureCoroutineView {
                closure_ty: &self.args[0],
                closure_view: self.args[0]
                    .as_closure_view()
                    .expect("a closure coroutine has a closure type"),
            }),
            Constant::CoroutineInstance => View::CoroutineInstance(&self.args[0]),
            Constant::ContinuationDefInstance => {
                View::ContinuationDefInstance(ContinuationDefInstanceView {
                    continuation: &self.args[0],
                    coroutine_instance: &self.args[1],
                })
            }
            Constant::CoroutineDropInstance => {
                let (coroutine, upvar_instances) =
                    self.args.split_first().expect("coroutine Drop instance has a coroutine type");
                View::CoroutineDropInstance(CoroutineDropInstanceView {
                    coroutine,
                    upvar_instances,
                })
            }
            Constant::Error(_) => View::Error,
        }
    }

    #[must_use]
    pub(super) fn has_same_constant(&self, other: &Self) -> bool {
        self.constant == other.constant && self.args.len() == other.args.len()
    }

    /// Returns the struct or enum this application instantiates, if it is
    /// one.
    #[must_use]
    pub const fn adt_id(&self) -> Option<GlobalSymbolID> {
        match self.constant {
            Constant::Struct(symbol_id) | Constant::Enum(symbol_id) => Some(symbol_id),
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
            | Constant::GeneratedAdtDropInstance
            | Constant::DefCoroutine(_)
            | Constant::TraitDefCoroutine(_)
            | Constant::ClosureCoroutine
            | Constant::ContinuationDefInstance
            | Constant::CoroutineInstance
            | Constant::CoroutineDropInstance
            | Constant::Error(_) => None,
        }
    }

    /// Returns each argument with the variance of its position; `adt_variances`
    /// is required when this is a struct or an enum.
    pub fn arguments_with_variance<'a>(
        &'a self,
        adt_variances: Option<&'a VarianceMap>,
    ) -> impl Iterator<Item = (&'a Interned<Ty>, Variance)> {
        self.args
            .iter()
            .enumerate()
            .map(move |(index, arg)| (arg, self.argument_variance(index, adt_variances)))
    }

    /// Returns each argument's variance within an `ambient` position, querying
    /// ADT variances only when they can matter.
    pub async fn arguments_with_ambient_variance(
        &self,
        ambient: Variance,
        engine: &TrackedEngine,
    ) -> impl Iterator<Item = (&Interned<Ty>, Variance)> {
        let adt_variances = match (ambient, self.adt_id()) {
            (Variance::Covariant | Variance::Contravariant, Some(adt_id)) => {
                Some(engine.get_variance(adt_id).await)
            }
            (Variance::Covariant | Variance::Contravariant, None)
            | (Variance::Invariant | Variance::Bivariant, _) => None,
        };

        self.args.iter().enumerate().map(move |(index, arg)| {
            let variance = match ambient {
                Variance::Invariant | Variance::Bivariant => ambient,
                Variance::Covariant | Variance::Contravariant => {
                    ambient.xform(self.argument_variance(index, adt_variances.as_deref()))
                }
            };
            (arg, variance)
        })
    }

    /// Returns the same type constructor applied to `args`, reduced if that
    /// makes a projection's instance known.
    pub async fn with_arguments(
        &self,
        args: Interned<[Interned<Ty>]>,
        engine: &TrackedEngine,
    ) -> Interned<Ty> {
        assert_eq!(args.len(), self.args.len(), "a type constructor keeps its arity");
        Ty::new_application(Self { constant: self.constant, args }, engine).await
    }

    /// Returns the variance of the argument position `index`; see
    /// [`Self::arguments_with_variance`].
    fn argument_variance(&self, index: usize, adt_variances: Option<&VarianceMap>) -> Variance {
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

            Constant::Struct(_) | Constant::Enum(_) => adt_variances
                .expect("a struct or enum application needs the variances of its declaration")
                .get_by_index(index),

            // Closure and coroutine types are invariant, as in rustc, and so
            // are dictionaries and associated type projections. Primitives
            // and errors have no arguments to relate.
            Constant::Primitive(_)
            | Constant::Instance(_)
            | Constant::InstanceAssociated(_)
            | Constant::Closure(_)
            | Constant::DefInstance
            | Constant::NoOpDropInstance
            | Constant::TupleDropInstance
            | Constant::ClosureDropInstance
            | Constant::GeneratedAdtDropInstance
            | Constant::DefCoroutine(_)
            | Constant::TraitDefCoroutine(_)
            | Constant::ClosureCoroutine
            | Constant::ContinuationDefInstance
            | Constant::CoroutineInstance
            | Constant::CoroutineDropInstance
            | Constant::Error(_) => Variance::Invariant,
        }
    }

    #[must_use]
    pub const fn is_instance_associated(&self) -> bool {
        matches!(self.constant, Constant::InstanceAssociated(_))
    }

    /// Returns whether this is a projection through an instance, which may
    /// still reduce once the instance is known.
    #[must_use]
    pub const fn is_projection(&self) -> bool {
        matches!(self.constant, Constant::InstanceAssociated(_) | Constant::TraitDefCoroutine(_))
    }

    #[must_use]
    pub(crate) async fn kind_of(&self, engine: &TrackedEngine) -> TyKind {
        match self.constant {
            Constant::Closure(_)
            | Constant::DefCoroutine(_)
            | Constant::TraitDefCoroutine(_)
            | Constant::ClosureCoroutine
            | Constant::Primitive(_)
            | Constant::Tuple
            | Constant::Pointer(_)
            | Constant::Reference(_)
            | Constant::Struct(_)
            | Constant::Enum(_) => TyKind::Star,

            Constant::InstanceAssociated(symbol_id) => {
                use crate::associated_type_kind::get_associated_type_kind;
                engine.get_associated_type_kind(symbol_id).await
            }

            Constant::NoOpDropInstance
            | Constant::TupleDropInstance
            | Constant::ClosureDropInstance
            | Constant::GeneratedAdtDropInstance
            | Constant::ContinuationDefInstance
            | Constant::CoroutineInstance
            | Constant::CoroutineDropInstance
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
                    Primitive::Integer(_) | Primitive::Float32 => true,
                    Primitive::Bool | Primitive::CStr => false,
                },

                View::Error
                | View::Tuple(_)
                | View::Pointer(_)
                | View::Reference(_)
                | View::Struct(_)
                | View::Enum(_)
                | View::Instance(_)
                | View::DefInstance(_)
                | View::Closure(_)
                | View::NoOpDropInstance(_)
                | View::TupleDropInstance(_)
                | View::ClosureDropInstance(_)
                | View::GeneratedAdtDropInstance(_)
                | View::DefCoroutine(_)
                | View::TraitDefCoroutine(_)
                | View::ClosureCoroutine(_)
                | View::ContinuationDefInstance(_)
                | View::CoroutineInstance(_)
                | View::CoroutineDropInstance(_)
                | View::InstanceAssociated(_) => false,
            },
            InferenceConstraint::EqualityComparable => match self.view() {
                View::Primitive(primitive) => match primitive {
                    Primitive::Integer(_) | Primitive::Float32 | Primitive::Bool => true,
                    Primitive::CStr => false,
                },

                View::Error
                | View::Tuple(_)
                | View::Pointer(_)
                | View::Reference(_)
                | View::Struct(_)
                | View::Enum(_)
                | View::Instance(_)
                | View::DefInstance(_)
                | View::InstanceAssociated(_)
                | View::NoOpDropInstance(_)
                | View::TupleDropInstance(_)
                | View::ClosureDropInstance(_)
                | View::GeneratedAdtDropInstance(_)
                | View::DefCoroutine(_)
                | View::TraitDefCoroutine(_)
                | View::ClosureCoroutine(_)
                | View::ContinuationDefInstance(_)
                | View::CoroutineInstance(_)
                | View::CoroutineDropInstance(_)
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

impl Application {
    /// Expands this projection once its instance is known, or returns `None`
    /// if it stays a projection; see [`Ty::new_application`].
    pub(super) async fn reduce_projection(&self, engine: &TrackedEngine) -> Option<Interned<Ty>> {
        match self.view() {
            View::InstanceAssociated(associated) => associated.reduce(engine).await,
            View::TraitDefCoroutine(coroutine) => coroutine.reduce(engine).await,

            View::Primitive(_)
            | View::Tuple(_)
            | View::Pointer(_)
            | View::Reference(_)
            | View::Struct(_)
            | View::Enum(_)
            | View::Instance(_)
            | View::Closure(_)
            | View::DefInstance(_)
            | View::NoOpDropInstance(_)
            | View::TupleDropInstance(_)
            | View::ClosureDropInstance(_)
            | View::GeneratedAdtDropInstance(_)
            | View::DefCoroutine(_)
            | View::ClosureCoroutine(_)
            | View::CoroutineInstance(_)
            | View::ContinuationDefInstance(_)
            | View::CoroutineDropInstance(_)
            | View::Error => None,
        }
    }
}

impl InstanceAssociatedView<'_> {
    // TODO: diagnose recursive associated types (e.g. `Item[a] =
    // Instance.Item[(a, a)]`), which may currently fail to terminate.
    /// Expands the projection by one definition step once its instance and
    /// member correspondence are known.
    async fn reduce(self, engine: &TrackedEngine) -> Option<Interned<Ty>> {
        // Closure dictionaries carry their signature directly, even during
        // inference.
        if let Ty::Application(application) = &**self.instance
            && let View::DefInstance(closure) = application.view()
        {
            if !self.args.is_empty() {
                return None;
            }
            let closure = closure.unwrap_as_closure_view();
            return if self.symbol_id == engine.get_core_item(CoreItem::DefArgs).await {
                Some(Ty::new_tuple(engine.intern_unsized(closure.params().to_vec()), engine))
            } else if self.symbol_id == engine.get_core_item(CoreItem::DefReturn).await {
                Some(closure.return_type().clone())
            } else if self.symbol_id == engine.get_core_item(CoreItem::DefEffect).await {
                Some(closure.effect_row().clone())
            } else {
                None
            };
        }

        // Coroutine dictionaries are told by the signature of the function
        // that the coroutine suspends.
        if let Ty::Application(application) = &**self.instance
            && let View::CoroutineInstance(coroutine) = application.view()
        {
            return self.reduce_through_coroutine(coroutine, engine).await;
        }

        // The dictionary that makes a continuation callable is told by the
        // continuation type and the dictionary of the coroutine it resumes.
        if let Ty::Application(application) = &**self.instance
            && let View::ContinuationDefInstance(continuation) = application.view()
        {
            return self.reduce_through_continuation(continuation, engine).await;
        }

        let instance = self.instance.as_instance_view()?;

        // The projection names a trait member, while the definition belongs to
        // its same-named implementation in the concrete instance.
        let name = engine.get_name(self.symbol_id).await;
        let member_id = engine.get_member_by_name(instance.symbol_id(), &name).await?;
        if engine.get_symbol_kind(member_id).await != SymbolKind::InstanceType {
            return None;
        }
        let member = engine.get_instance_member(member_id).await?;
        if member.trait_member_id() != self.symbol_id {
            return None;
        }

        let trait_poly_vars = engine.get_poly_var_map(self.symbol_id).await;
        if trait_poly_vars.len() != self.args.len() {
            return None;
        }

        // Enclosing variables are supplied by the instance application.
        // Translate member arguments through the checked trait-to-implementation
        // mapping.
        let mut substitution =
            engine.build_subst_from_args(instance.symbol_id(), instance.args()).await;
        for ((id, _), argument) in trait_poly_vars.iter().zip(self.args) {
            let trait_var = GlobalPolyVarID::new(self.symbol_id, id);
            let implementation_var =
                member.poly_var_substitution().get(&trait_var)?.as_poly_var()?;

            substitution.insert(*implementation_var, argument.clone());
        }

        let definition = engine.get_type_definition(member_id).await;
        Some(definition.apply_subst_or_clone(&substitution, engine).await)
    }

    /// Reduces `Return` or `Effect` of the built-in `Coroutine` dictionary
    /// from the signature of the suspended function.
    async fn reduce_through_coroutine(
        self,
        coroutine: &Interned<Ty>,
        engine: &TrackedEngine,
    ) -> Option<Interned<Ty>> {
        if !self.args.is_empty() {
            return None;
        }
        let coroutine = coroutine.as_coroutine_behind_reference()?;

        // Neither associated type depends on where the coroutine is
        // suspended. A call through an instance that is not known yet is told
        // by the signature of the trait method.
        let signature_id = coroutine.signature_id(engine).await;
        let signature = if self.symbol_id == engine.get_core_item(CoreItem::CoroutineReturn).await {
            engine.get_return_type(signature_id).await
        } else if self.symbol_id == engine.get_core_item(CoreItem::CoroutineEffect).await {
            engine.get_effect_row(signature_id).await
        } else {
            return None;
        };

        let substitution = coroutine.create_subst(engine).await;
        Some(signature.apply_subst_or_clone(&substitution, engine).await)
    }

    /// Reduces `Args`, `Return` or `Effect` of the built-in `Def` dictionary
    /// of a continuation, which forwards those of its coroutine.
    async fn reduce_through_continuation(
        self,
        continuation: ContinuationDefInstanceView<'_>,
        engine: &TrackedEngine,
    ) -> Option<Interned<Ty>> {
        if !self.args.is_empty() {
            return None;
        }

        // `Return` and `Effect` stay projections of the coroutine dictionary,
        // which may be a given that only a where clause tells more about.
        let coroutine_member = if self.symbol_id == engine.get_core_item(CoreItem::DefArgs).await {
            return Some(Ty::new_tuple(
                engine.intern_unsized([continuation.resume_ty().clone()]),
                engine,
            ));
        } else if self.symbol_id == engine.get_core_item(CoreItem::DefReturn).await {
            engine.get_core_item(CoreItem::CoroutineReturn).await
        } else if self.symbol_id == engine.get_core_item(CoreItem::DefEffect).await {
            engine.get_core_item(CoreItem::CoroutineEffect).await
        } else {
            return None;
        };

        Some(
            Ty::new_instance_associated(
                coroutine_member,
                continuation.coroutine_instance().clone(),
                [],
                engine,
            )
            .await,
        )
    }
}

impl TraitDefCoroutineView<'_> {
    /// Reduces the coroutine of a call through an instance to the implementing
    /// method's or closure's coroutine, once the instance is known.
    async fn reduce(self, engine: &TrackedEngine) -> Option<Interned<Ty>> {
        // `Def.call` through `DefInstance(C)` runs the body of the closure
        // `C`. Both coroutine types have the signature of `Def.call` under the
        // same substitution, so nothing read from the coroutine changes.
        if let Ty::Application(application) = &**self.instance
            && let View::DefInstance(closure_def_instance) = application.view()
        {
            if self.trait_def_id != engine.get_core_item(CoreItem::DefCall).await
                || !self.args.is_empty()
            {
                return None;
            }

            return Some(Ty::new_closure_coroutine(closure_def_instance.clone(), engine));
        }

        let instance = self.instance.as_instance_view()?;

        // The trait method and its implementation correspond by name, as for
        // an associated type.
        let name = engine.get_name(self.trait_def_id).await;
        let member_id = engine.get_member_by_name(instance.symbol_id(), &name).await?;
        if engine.get_symbol_kind(member_id).await != SymbolKind::InstanceDef {
            return None;
        }
        let member = engine.get_instance_member(member_id).await?;
        if member.trait_member_id() != self.trait_def_id {
            return None;
        }

        let trait_poly_vars = engine.get_poly_var_map(self.trait_def_id).await;
        if trait_poly_vars.len() != self.args.len() {
            return None;
        }

        // The instance application supplies the parameters of the instance.
        // The method's own arguments move to the corresponding parameters of
        // its implementation.
        let mut substitution =
            engine.build_subst_from_args(instance.symbol_id(), instance.args()).await;
        for ((id, _), argument) in trait_poly_vars.iter().zip(self.args) {
            let trait_var = GlobalPolyVarID::new(self.trait_def_id, id);
            let implementation_var =
                member.poly_var_substitution().get(&trait_var)?.as_poly_var()?;

            substitution.insert(*implementation_var, argument.clone());
        }

        // A coroutine type lists the arguments of the implementation first,
        // then those of its instance.
        let poly_vars = engine.get_enclosing_poly_var_maps(member_id).await;
        let poly_args = poly_vars.create_args_from_subst(&substitution)?;

        Some(Ty::new_def_coroutine(member_id, poly_args, engine))
    }
}

impl Application {
    /// Folds the arguments, reducing the application if it is a projection
    /// whose instance the fold made known; see [`Ty::super_fold_with`].
    pub(crate) async fn fold_args(
        &self,
        folder: &mut impl TyFolder,
        engine: &TrackedEngine,
    ) -> Option<Interned<Ty>> {
        let args = self.args.fold_with(folder, engine).await?;
        Some(self.with_arguments(args, engine).await)
    }
}

#[cfg(test)]
mod associated_type_tests;

#[cfg(test)]
mod closure_coroutine_tests;

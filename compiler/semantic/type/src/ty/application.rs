use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::ID;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
};

use super::{
    InferenceConstraint, Mutability, Primitive, Ty, TyKind, inference::Inference,
    self_instance::SelfInstance,
};
use crate::{
    poly_var::{build_subst_from_args, get_enclosing_poly_var_maps},
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
    /// A nominal enum identified by its `SymbolKind::Enum` symbol.
    Enum(GlobalSymbolID),
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
    /// The generated `Drop` dictionary for a struct or an enum. Its arguments
    /// are the instantiated nominal type followed by its external dictionaries
    /// in `GeneratedDropPlan::requirements` order.
    GeneratedAdtDropInstance,
    /// The anonymous type of a suspended function, identified by the function
    /// `coro` was applied to. Its arguments are the arguments for the
    /// polymorphic parameters of that function, followed by those of each
    /// enclosing symbol, proceeding outwards.
    ///
    /// There is one coroutine type per function, not one per `coro`
    /// expression. Everything else about it is derived by substituting the
    /// arguments into the signature of the function.
    DefCoroutine(GlobalSymbolID),
    /// The anonymous type of a suspended call through an instance that is not
    /// known yet, identified by the `SymbolKind::TraitDef` symbol of the
    /// called method. Its arguments are the instance, followed by the
    /// arguments for the method's own polymorphic parameters in declaration
    /// order; those of the trait are told by the instance.
    ///
    /// It is a projection, like [`Self::InstanceAssociated`]: once the
    /// instance is a declared one, it reduces to the [`Self::DefCoroutine`] of
    /// that instance's implementation of the method, and until then it is
    /// equal only to itself.
    TraitDefCoroutine(GlobalSymbolID),
    /// The anonymous type of a suspended call of a closure, whose sole
    /// argument is the closure type. The closure type tells the closure, the
    /// arguments of its owner and its captures.
    ///
    /// It runs the signature of `core.Def.call`, with the trait's `f` as the
    /// closure type and `this` as its built-in [`Self::DefInstance`]. That is
    /// the [`Self::TraitDefCoroutine`] of a call through that dictionary,
    /// which reduces to this type.
    ClosureCoroutine,
    /// The built-in `Coroutine` dictionary whose sole argument is a coroutine
    /// type, or a `&mut` to one.
    CoroutineInstance,
    /// The built-in `Def` dictionary of `core.Continuation[r, c]`, which makes
    /// a continuation callable. Its arguments are the continuation type
    /// followed by the `Coroutine` dictionary of `c`, which tells what
    /// resuming the coroutine returns and performs.
    ContinuationDefInstance,
    /// The built-in `Drop` dictionary for a coroutine type. Its arguments are
    /// the coroutine type followed by one `Drop` dictionary for each upvar,
    /// which are the parameters of the suspended function in declaration
    /// order.
    CoroutineDropInstance,
    Error(TyKind),
}

/// Represents an anonymous type created by the closure expression. It
/// represents the storage that holds all the captured variables required by the
/// closure. The type is created by the compiler and is not visible to the user.
///
/// The arguments of the closure application type are
/// - The owner's polymorphic arguments in declaration order, followed by those
///   of each enclosing symbol, proceeding outwards.
/// - A list of parameter types, this can be empty if the closure has no
///   parameters.
/// - The return type of the closure.
/// - The effect row of the closure.
/// - The tuple type of all the captured variables of the closure. This can be
///   an empty tuple if the closure has no captured variables.
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

        poly_vars.subst_from_args(self.poly_args.iter().cloned())
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

    /// Returns the substitution that instantiates the signature of the trait
    /// method: its own parameters with [`Self::args`], the parameters of its
    /// trait with the arguments of the trait reference that the instance
    /// implements, and the trait's `this` with the instance.
    ///
    /// The trait reference is unknown while the instance is an inference
    /// variable, or after an invalid declaration; the parameters of the trait
    /// are then left out.
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
    closure: &'x Interned<Ty>,
}

impl<'x> ClosureCoroutineView<'x> {
    /// Returns the type of the suspended closure.
    #[must_use]
    pub const fn closure(&self) -> &'x Interned<Ty> { self.closure }

    /// Returns the substitution that instantiates the signature of
    /// `core.Def.call`: the trait's `f` with the closure type, and `this`
    /// with the closure's built-in `Def` dictionary. The method has no
    /// parameters of its own.
    pub async fn create_subst(&self, engine: &TrackedEngine) -> Subst {
        let def_trait = engine.get_core_item(CoreItem::DefTrait).await;
        let mut subst = engine.build_subst_from_args(def_trait, [self.closure]).await;
        subst.insert(
            SelfInstance::new(def_trait),
            Ty::new_def_instance(self.closure.clone(), engine),
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
    /// Returns the symbol whose signature the coroutine runs: the suspended
    /// function, the trait method of a call through an instance, or
    /// `core.Def.call` for a call of a closure.
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

    /// Returns the substitution that instantiates the type parameters that
    /// the `coro` creating this coroutine chose, as a call of the same callee
    /// would: those of the suspended function and its enclosing symbols, or
    /// the trait method's own. `core.Def.call` has none of its own.
    pub async fn instantiation(&self, engine: &TrackedEngine) -> Subst {
        match self {
            Self::Def(coroutine) => coroutine.create_subst(engine).await,
            Self::TraitDef(coroutine) => coroutine.create_own_subst(engine).await,
            Self::Closure(_) => Subst::new_empty(),
        }
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
            Constant::ClosureCoroutine => {
                View::ClosureCoroutine(ClosureCoroutineView { closure: &self.args[0] })
            }
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

    /// Returns each argument with the variance of its position.
    ///
    /// `adt_variances` are the variances of the struct's or enum's parameters
    /// when this application is one (see [`Self::adt_id`]), and are ignored
    /// otherwise.
    ///
    /// # Panics
    ///
    /// If this application is a struct or an enum and `adt_variances` is
    /// `None` or has fewer variances than there are arguments.
    pub fn arguments_with_variance<'a>(
        &'a self,
        adt_variances: Option<&'a VarianceMap>,
    ) -> impl Iterator<Item = (&'a Interned<Ty>, Variance)> {
        self.args
            .iter()
            .enumerate()
            .map(move |(index, arg)| (arg, self.argument_variance(index, adt_variances)))
    }

    /// Returns each argument with the variance of its position, when this
    /// application itself is in a position of variance `ambient`.
    ///
    /// An invariant or bivariant position absorbs every variance inside it,
    /// so the variances of a struct or an enum are only queried when `ambient`
    /// is covariant or contravariant.
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

    /// Returns whether this application is a projection through an instance,
    /// which may still reduce once the instance is known: an associated type
    /// or the coroutine of a call through an instance.
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

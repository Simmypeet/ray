use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_extend::extend;
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    poly_var::build_subst_from_args,
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::{Integer, Mutability, Primitive, Ty, application::View as ApplicationView},
};

use crate::{
    instance::{MonoCoroutineInstance, MonoEffectInstance, MonoStructInstance},
    layout::TagType,
    operand::Constant,
};

/// A fully concrete type with a direct runtime representation.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum MonoType {
    Bool,
    Int8,
    Int16,
    Int32,
    Int64,
    Isize,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Usize,
    Float32,
    CInt,
    CStr,
    OpaquePointer(PointerMutability),
    Pointer(PointerType),
    Aggregate(AggregateType),
    FunctionPointer(FunctionSignature),
}

impl MonoType {
    /// Returns the runtime representation of the given integer type.
    #[must_use]
    pub const fn from_integer(integer: Integer) -> Self {
        match integer {
            Integer::Int8 => Self::Int8,
            Integer::Int16 => Self::Int16,
            Integer::Int32 => Self::Int32,
            Integer::Int64 => Self::Int64,
            Integer::Isize => Self::Isize,
            Integer::Uint8 => Self::Uint8,
            Integer::Uint16 => Self::Uint16,
            Integer::Uint32 => Self::Uint32,
            Integer::Uint64 => Self::Uint64,
            Integer::Usize => Self::Usize,
            Integer::CInt => Self::CInt,
        }
    }

    #[must_use]
    pub const fn is_opauque_mut_pointer(&self) -> bool {
        matches!(self, Self::OpaquePointer(PointerMutability::Mut))
    }

    #[must_use]
    pub fn is_unit(&self) -> bool {
        matches!(self, Self::Aggregate(AggregateType::Tuple(tuple)) if tuple.is_empty())
    }

    #[must_use]
    pub fn assert_as_tuple(&self) -> &Tuple {
        match self {
            Self::Aggregate(AggregateType::Tuple(tuple)) => tuple,
            _ => panic!("compiler-internal invariant violation: expected tuple type, got {self:?}"),
        }
    }

    #[must_use]
    pub fn assert_as_pointer(&self) -> &PointerType {
        match self {
            Self::Pointer(pointer) => pointer,
            _ => {
                panic!("compiler-internal invariant violation: expected pointer type, got {self:?}")
            }
        }
    }

    #[must_use]
    pub fn assert_as_effect_handler(&self) -> &EffectHandler {
        match self {
            Self::Aggregate(AggregateType::EffectHandler(handler)) => handler,
            _ => panic!(
                "compiler-internal invariant violation: expected effect handler type, got {self:?}"
            ),
        }
    }

    #[must_use]
    pub fn assert_as_struct(&self) -> &MonoStructInstance {
        match self {
            Self::Aggregate(AggregateType::Struct(struct_)) => struct_,
            _ => {
                panic!("compiler-internal invariant violation: expected struct type, got {self:?}")
            }
        }
    }

    /// Returns the frame type of the coroutine this type is, or panics.
    #[must_use]
    pub fn assert_as_coroutine_frame(&self) -> &MonoCoroutineInstance {
        match self {
            Self::Aggregate(AggregateType::CoroutineFrame(instance)) => instance,
            _ => panic!(
                "compiler-internal invariant violation: expected coroutine frame, got {self:?}"
            ),
        }
    }

    /// The empty tuple.
    #[must_use]
    pub fn new_unit(engine: &TrackedEngine) -> Interned<Self> {
        let fields = engine.intern_unsized(Vec::new());
        engine.intern(Self::Aggregate(AggregateType::Tuple(Tuple::new(fields))))
    }

    #[must_use]
    pub fn new_opaque_pointer(engine: &TrackedEngine) -> Interned<Self> {
        engine.intern(Self::OpaquePointer(PointerMutability::Mut))
    }

    #[must_use]
    pub fn new_pointer(
        pointee: Interned<Self>,
        mutability: PointerMutability,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        engine.intern(Self::Pointer(PointerType::new(pointee, mutability)))
    }

    #[must_use]
    pub fn new_function_signature(
        parameter_types: impl IntoIterator<Item = Interned<Self>>,
        return_type: Interned<Self>,
        engine: &TrackedEngine,
    ) -> FunctionSignature {
        FunctionSignature::new(
            engine.intern_unsized(parameter_types.into_iter().collect::<Vec<_>>()),
            ReturnType::Value(return_type),
        )
    }

    #[must_use]
    pub fn new_handler_pointer(
        instance: MonoEffectInstance,
        engine: &TrackedEngine,
    ) -> Interned<Self> {
        let handler = engine
            .intern(Self::Aggregate(AggregateType::EffectHandler(EffectHandler::new(instance))));
        Self::new_pointer(handler, PointerMutability::Const, engine)
    }
}

/// Whether writes through a pointer are permitted by the `MonoIR` type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum PointerMutability {
    Const,
    Mut,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct PointerType {
    pointee: Interned<MonoType>,
    mutability: PointerMutability,
}

impl PointerType {
    #[must_use]
    pub const fn new(pointee: Interned<MonoType>, mutability: PointerMutability) -> Self {
        Self { pointee, mutability }
    }

    #[must_use]
    pub const fn pointee(&self) -> &Interned<MonoType> { &self.pointee }

    #[must_use]
    pub const fn mutability(&self) -> PointerMutability { self.mutability }
}

/// The nominal record type for one concrete effect instantiation.
///
/// Its fields are described by a corresponding [`HandlerLayout`], which can
/// be derived from the concrete effect instance when needed. C code generation
/// can emit that layout as a struct of callback closures and pass a pointer to
/// the struct as a hidden parameter to effectful functions. The type is nominal
/// so a callback that itself uses effects does not create a recursively
/// expanded structural type.
///
/// [`HandlerLayout`]: crate::layout::HandlerLayout
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct EffectHandler {
    mono_effect_instance: MonoEffectInstance,
}

impl EffectHandler {
    #[must_use]
    pub const fn new(mono_effect_instance: MonoEffectInstance) -> Self {
        Self { mono_effect_instance }
    }

    #[must_use]
    pub const fn mono_effect_instance(&self) -> &MonoEffectInstance { &self.mono_effect_instance }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Tuple {
    fields: Interned<[Interned<MonoType>]>,
}

impl Tuple {
    #[must_use]
    pub const fn new(fields: Interned<[Interned<MonoType>]>) -> Self { Self { fields } }

    #[must_use]
    pub fn fields(&self) -> &[Interned<MonoType>] { &self.fields }

    #[must_use]
    pub fn len(&self) -> usize { self.fields.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.fields.is_empty() }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Environment {
    captures: Interned<[Interned<MonoType>]>,
}

impl Environment {
    #[must_use]
    pub const fn new(captures: Interned<[Interned<MonoType>]>) -> Self { Self { captures } }

    #[must_use]
    pub fn captures(&self) -> &[Interned<MonoType>] { &self.captures }
}

/// An aggregate with a layout determined by its semantic role.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum AggregateType {
    EffectHandler(EffectHandler),
    Tuple(Tuple),
    Environment(Environment),
    /// A concrete struct. It is nominal, so a struct type is lowered without
    /// lowering its fields, which a [`StructLayout`] tells.
    ///
    /// [`StructLayout`]: crate::layout::StructLayout
    Struct(MonoStructInstance),
    /// The `coroutine_frame_t` of a coroutine. It is nominal, so a frame type
    /// is lowered without reading the body of the suspended function; a
    /// [`CoroutineFrameLayout`] tells its fields.
    ///
    /// [`CoroutineFrameLayout`]: crate::layout::CoroutineFrameLayout
    CoroutineFrame(MonoCoroutineInstance),
    /// The `coroutine_resume_t` of an effect row, which holds what a suspended
    /// coroutine is resumed with.
    CoroutineResume(CoroutineResume),
    /// The `coroutine_yield_t` of an effect row, which holds the operation a
    /// coroutine suspended at.
    CoroutineYield(CoroutineYield),
    /// The `coroutine_progress_t` of a coroutine: the result of resuming it.
    CoroutineProgress(CoroutineProgress),
}

/// A closed effect row in its canonical order, which is that of
/// [`lower_effects`].
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct EffectRow {
    effects: Interned<[MonoEffectInstance]>,
}

impl EffectRow {
    /// Lowers a concrete, closed effect row using the supplied substitution.
    #[must_use]
    pub async fn new(solver: &Solver, effect: &Interned<Ty>, substitution: &Subst) -> Self {
        let effects = lower_effects(solver, effect, substitution).await;
        Self { effects: solver.engine().intern_unsized(effects) }
    }

    #[must_use]
    pub fn effects(&self) -> &[MonoEffectInstance] { &self.effects }
}

/// The `coroutine_resume_t` of an effect row.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct CoroutineResume(EffectRow);

impl CoroutineResume {
    #[must_use]
    pub const fn new(row: EffectRow) -> Self { Self(row) }

    #[must_use]
    pub const fn row(&self) -> &EffectRow { &self.0 }
}

/// The `coroutine_yield_t` of an effect row.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct CoroutineYield(EffectRow);

impl CoroutineYield {
    #[must_use]
    pub const fn new(row: EffectRow) -> Self { Self(row) }

    #[must_use]
    pub const fn row(&self) -> &EffectRow { &self.0 }
}

/// What resuming a coroutine with the given effect row and return type gives.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct CoroutineProgress {
    row: EffectRow,
    return_type: Interned<MonoType>,
}

impl CoroutineProgress {
    /// The type of the tag, which tells a return from a yield.
    pub const TAG_TYPE: TagType = TagType::Uint8;

    /// The coroutine completed; the progress holds its return value.
    #[must_use]
    pub fn return_tag() -> Constant { Self::TAG_TYPE.constant(0) }

    /// The coroutine suspended; the progress holds its `coroutine_yield_t`.
    #[must_use]
    pub fn yield_tag() -> Constant { Self::TAG_TYPE.constant(1) }

    #[must_use]
    pub const fn new(row: EffectRow, return_type: Interned<MonoType>) -> Self {
        Self { row, return_type }
    }

    #[must_use]
    pub const fn row(&self) -> &EffectRow { &self.row }

    #[must_use]
    pub const fn return_type(&self) -> &Interned<MonoType> { &self.return_type }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum ReturnType {
    Void,
    Value(Interned<MonoType>),
}

impl ReturnType {
    /// Whether the function returns no value at all, as C `void`.
    #[must_use]
    pub const fn is_void(&self) -> bool {
        match self {
            Self::Void => true,
            Self::Value(_) => false,
        }
    }
}

/// A concrete calling signature shared by direct and indirect calls.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct FunctionSignature {
    parameter_types: Interned<[Interned<MonoType>]>,
    return_type: ReturnType,
    variadic: bool,
}

impl FunctionSignature {
    #[must_use]
    pub const fn new(
        parameter_types: Interned<[Interned<MonoType>]>,
        return_type: ReturnType,
    ) -> Self {
        Self { parameter_types, return_type, variadic: false }
    }

    #[must_use]
    pub const fn new_variadic(
        parameter_types: Interned<[Interned<MonoType>]>,
        return_type: ReturnType,
    ) -> Self {
        Self { parameter_types, return_type, variadic: true }
    }

    #[must_use]
    pub fn parameter_types(&self) -> &[Interned<MonoType>] { &self.parameter_types }

    #[must_use]
    pub const fn return_type(&self) -> &ReturnType { &self.return_type }

    #[must_use]
    pub const fn is_variadic(&self) -> bool { self.variadic }
}

/// Lowers a type of kind star using the supplied concrete substitution.
///
/// `solver` normalizes projections; callers lowering many types should reuse
/// one solver so any normalization state it keeps is shared.
pub async fn lower_type(
    solver: &Solver,
    ty: &Interned<Ty>,
    substitution: &Subst,
) -> Interned<MonoType> {
    let ty = ty.apply_subst_or_clone(substitution, solver.engine());
    let ty = solver.normalize(&ty).await;

    // Lifetimes never affect code generation. Erasing them makes nominal
    // types that differ only in lifetimes lower to the same type.
    let ty = Ty::erase_lifetimes(&ty, solver.engine()).await;
    lower_concrete_type(solver, &ty).await
}

async fn lower_concrete_type(solver: &Solver, ty: &Interned<Ty>) -> Interned<MonoType> {
    let engine = solver.engine();
    match &**ty {
        Ty::Application(application) => match application.view() {
            ApplicationView::Primitive(primitive) => {
                let ty = match primitive {
                    Primitive::Integer(integer) => MonoType::from_integer(integer),
                    Primitive::Float32 => MonoType::Float32,
                    Primitive::Bool => MonoType::Bool,
                    Primitive::CStr => MonoType::CStr,
                };
                engine.intern(ty)
            }

            ApplicationView::Closure(closure) => {
                let environment = Box::pin(nominal_environment(solver, closure)).await;
                engine.intern(MonoType::Aggregate(AggregateType::Environment(environment)))
            }

            ApplicationView::Tuple(tuple) => {
                let mut fields = Vec::with_capacity(tuple.args().len());
                for ty in tuple.args() {
                    fields.push(Box::pin(lower_concrete_type(solver, ty)).await);
                }
                let fields = engine.intern_unsized(fields);
                engine.intern(MonoType::Aggregate(AggregateType::Tuple(Tuple::new(fields))))
            }

            ApplicationView::Pointer(pointer) => {
                let pointee_type = Box::pin(lower_concrete_type(solver, pointer.pointee())).await;
                MonoType::new_pointer(pointee_type, lower_mutability(pointer.mutability()), engine)
            }
            // A reference has the same representation as a raw pointer.
            ApplicationView::Reference(reference) => {
                let pointee_type = Box::pin(lower_concrete_type(solver, reference.pointee())).await;
                MonoType::new_pointer(
                    pointee_type,
                    lower_mutability(reference.mutability()),
                    engine,
                )
            }
            ApplicationView::Struct(struct_view) => {
                let substitution = struct_view.create_subst(engine).await;
                let instance = MonoStructInstance::new(struct_view.symbol_id(), substitution);
                engine.intern(MonoType::Aggregate(AggregateType::Struct(instance)))
            }
            ApplicationView::InstanceAssociated(_) => {
                panic!("unresolved associated type reached code generation")
            }
            ApplicationView::DefCoroutine(_) | ApplicationView::ClosureCoroutine(_) => {
                let instance = MonoCoroutineInstance::new(ty, solver).await;
                engine.intern(MonoType::Aggregate(AggregateType::CoroutineFrame(instance)))
            }
            // Only the coroutine of a declared instance's method or of a
            // closure has a body to suspend, and both have reduced by now.
            ApplicationView::TraitDefCoroutine(_) => {
                panic!("a coroutine without a body to suspend reached MonoIR")
            }
            ApplicationView::Enum(_) => panic!("enum types are not lowered to MonoIR yet"),
            ApplicationView::NoOpDropInstance(_)
            | ApplicationView::TupleDropInstance(_)
            | ApplicationView::ClosureDropInstance(_)
            | ApplicationView::ContinuationDefInstance(_)
            | ApplicationView::CoroutineInstance(_)
            | ApplicationView::CoroutineDropInstance(_)
            | ApplicationView::GeneratedAdtDropInstance(_)
            | ApplicationView::Instance(_)
            | ApplicationView::DefInstance(_) => {
                panic!("compiler-internal invariant violation: instance used as a value type")
            }
            ApplicationView::Error => {
                panic!("compiler-internal invariant violation: error type reached MonoIR")
            }
        },
        Ty::Inference(_) | Ty::PolyVar(_) | Ty::SelfInstance(_) => {
            panic!(
                "compiler-internal invariant violation: non-concrete type reached MonoIR: {ty:?}"
            )
        }
        Ty::EffectRow(_) => {
            panic!("compiler-internal invariant violation: effect row used as a value type")
        }
        Ty::Lifetime(_) => {
            panic!("compiler-internal invariant violation: lifetime used as a value type")
        }
    }
}

/// Lowers a concrete, closed effect row using the supplied substitution.
pub async fn lower_effects(
    solver: &Solver,
    effect: &Interned<Ty>,
    substitution: &Subst,
) -> Vec<MonoEffectInstance> {
    let effect = effect.apply_subst_or_clone(substitution, solver.engine());
    let effect = solver.normalize(&effect).await;
    let effect = Ty::erase_lifetimes(&effect, solver.engine()).await;
    lower_concrete_effects(solver.engine(), &effect).await
}

async fn lower_concrete_effects(
    engine: &TrackedEngine,
    effect: &Interned<Ty>,
) -> Vec<MonoEffectInstance> {
    let Ty::EffectRow(row) = &**effect else {
        panic!("compiler-internal invariant violation: function effect is not an effect row")
    };
    assert!(
        row.tail().is_none(),
        "compiler-internal invariant violation: open effect row reached MonoIR: {:?}",
        row.tail()
    );

    let mut effects = Vec::with_capacity(row.labels().len());
    for label in row.labels() {
        let substitution =
            engine.build_subst_from_args(label.effect_symbol_id(), label.arguments()).await;
        effects.push(MonoEffectInstance::new(label.effect_symbol_id(), substitution));
    }
    effects.sort();
    effects.dedup();
    effects
}

#[extend]
pub async fn instantiate_effect(
    self: &TrackedEngine,
    effect_id: GlobalSymbolID,
    substitution: &Subst,
    owner_substitution: &Subst,
) -> MonoEffectInstance {
    let mut substitution = substitution.clone();
    substitution.apply_mut_subst(owner_substitution, self);
    substitution.erase_lifetimes(self).await;
    MonoEffectInstance::new(effect_id, substitution)
}

const fn lower_mutability(mutability: Mutability) -> PointerMutability {
    match mutability {
        Mutability::Immutable => PointerMutability::Const,
        Mutability::Mutable => PointerMutability::Mut,
    }
}

/// The inline storage shared by nominal values and their body ABI.
pub async fn nominal_environment(
    solver: &Solver,
    closure: rayc_type::ty::application::ClosureView<'_>,
) -> Environment {
    let engine = solver.engine();
    let tuple = lower_type(solver, closure.captured_tuple(), &Subst::new_empty()).await;
    let MonoType::Aggregate(AggregateType::Tuple(tuple)) = &*tuple else {
        panic!("closure captures must be a tuple")
    };
    Environment::new(engine.intern_unsized(tuple.fields().to_vec()))
}

/// Plans the concrete nominal body call, including the inline environment and
/// handlers.
pub async fn nominal_signature(
    solver: &Solver,
    closure: rayc_type::ty::application::ClosureView<'_>,
) -> (FunctionSignature, Vec<MonoEffectInstance>) {
    let engine = solver.engine();
    let environment = nominal_environment(solver, closure).await;
    let mut parameters = Vec::new();
    for parameter in closure.params() {
        parameters.push(lower_type(solver, parameter, &Subst::new_empty()).await);
    }
    let effects = lower_effects(solver, closure.effect_row(), &Subst::new_empty()).await;
    let result = lower_type(solver, closure.return_type(), &Subst::new_empty()).await;
    (nominal_body_signature(engine, environment, parameters, result, &effects), effects)
}

/// Builds the by-value nominal ABI from concrete storage and source parameters.
#[must_use]
pub fn nominal_body_signature(
    engine: &TrackedEngine,
    environment: Environment,
    parameters: impl IntoIterator<Item = Interned<MonoType>>,
    result: Interned<MonoType>,
    effects: &[MonoEffectInstance],
) -> FunctionSignature {
    let mut body_parameters =
        vec![engine.intern(MonoType::Aggregate(AggregateType::Environment(environment)))];
    body_parameters.extend(parameters);
    body_parameters
        .extend(effects.iter().map(|effect| MonoType::new_handler_pointer(effect.clone(), engine)));
    MonoType::new_function_signature(body_parameters, result, engine)
}

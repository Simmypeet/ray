use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_extend::extend;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{parameter::get_parameter_map, return_type::get_return_type};
use rayc_symbol::{GlobalSymbolID, member::get_members};
use rayc_type::{
    poly_var::build_subst_from_args,
    reduce::Reduce,
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::{Mutability, Primitive, Ty, application::View as ApplicationView},
};

use crate::instance::MonoEffectInstance;

/// A fully concrete type with a direct runtime representation.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum MonoType {
    Unit,
    Bool,
    Int32,
    Float32,
    CInt,
    CStr,
    OpaquePointer(PointerMutability),
    Pointer(PointerType),
    Aggregate(AggregateType),
    FunctionPointer(FunctionSignature),
}

impl MonoType {
    #[must_use]
    pub fn new_opaque_pointer(engine: &TrackedEngine) -> Interned<Self> {
        engine.intern(Self::OpaquePointer(PointerMutability::Const))
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
            ReturnType::Value(engine.intern_unsized([return_type])),
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

/// ABI of Closure is roughly described as follows:
///
/// ```c
/// typedef struct Closure {
///     void* environment;
///     return_type (*function_pointer)(void* environment, parameter_types...)
/// };
/// ```
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Closure {
    function_signature: FunctionSignature,
}

impl Closure {
    #[must_use]
    pub const fn new(function_signature: FunctionSignature) -> Self { Self { function_signature } }

    #[must_use]
    pub const fn function_signature(&self) -> &FunctionSignature { &self.function_signature }
}

/// The nominal record type for one concrete effect instantiation.
///
/// Its fields are described by a corresponding [`HandlerLayout`], which can
/// be derived from the concrete effect instance when needed. C code generation
/// can emit that layout as a struct of callback closures and pass a pointer to
/// the struct as a hidden parameter to effectful functions. The type is nominal
/// so a callback that itself uses effects does not create a recursively
/// expanded structural type.
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
    Closure(Closure),
    EffectHandler(EffectHandler),
    Tuple(Tuple),
    Environment(Environment),
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum ReturnType {
    Void,
    Value(Interned<[Interned<MonoType>]>),
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

/// One callback slot in a concrete effect-handler record.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct EffectOperation {
    operation_id: GlobalSymbolID,
    signature: FunctionSignature,
}

impl EffectOperation {
    #[must_use]
    pub const fn new(operation_id: GlobalSymbolID, signature: FunctionSignature) -> Self {
        Self { operation_id, signature }
    }

    #[must_use]
    pub const fn operation_id(&self) -> GlobalSymbolID { self.operation_id }

    #[must_use]
    pub const fn signature(&self) -> &FunctionSignature { &self.signature }
}

/// The operation slots of one nominal, concrete effect-handler record.
///
/// C code generation can represent this as a struct with one closure field per
/// operation. Each closure consists of a callback address with the operation's
/// [`FunctionSignature`] and an opaque environment pointer. There is no
/// continuation or resumption slot: invoking an operation is an ordinary
/// callback call that returns to its caller.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct HandlerLayout {
    instance: MonoEffectInstance,
    operations: Vec<EffectOperation>,
}

impl HandlerLayout {
    #[must_use]
    pub const fn new(instance: MonoEffectInstance, operations: Vec<EffectOperation>) -> Self {
        Self { instance, operations }
    }

    #[must_use]
    pub const fn instance(&self) -> &MonoEffectInstance { &self.instance }

    #[must_use]
    pub fn operations(&self) -> &[EffectOperation] { &self.operations }
}

/// Lowers a type of kind star using the supplied concrete substitution.
#[extend]
pub async fn lower_type(
    self: &TrackedEngine,
    ty: &Interned<Ty>,
    substitution: &Subst,
) -> Interned<MonoType> {
    let ty = ty.apply_subst_or_clone(substitution, self);
    lower_concrete_type(self, &ty).await
}

async fn lower_concrete_type(engine: &TrackedEngine, ty: &Interned<Ty>) -> Interned<MonoType> {
    match &**ty {
        Ty::Application(application) => match application.view() {
            ApplicationView::Primitive(primitive) => {
                let ty = match primitive {
                    Primitive::Int32 => MonoType::Int32,
                    Primitive::Float32 => MonoType::Float32,
                    Primitive::Bool => MonoType::Bool,
                    Primitive::CInt => MonoType::CInt,
                    Primitive::CStr => MonoType::CStr,
                };
                engine.intern(ty)
            }
            ApplicationView::Tuple(tuple) => {
                if tuple.args().is_empty() {
                    return engine.intern(MonoType::Unit);
                }
                let mut fields = Vec::with_capacity(tuple.args().len());
                for ty in tuple.args() {
                    fields.push(Box::pin(lower_concrete_type(engine, ty)).await);
                }
                let fields = engine.intern_unsized(fields);
                engine.intern(MonoType::Aggregate(AggregateType::Tuple(Tuple::new(fields))))
            }
            ApplicationView::Lambda(lambda) => {
                let mut parameters = vec![MonoType::new_opaque_pointer(engine)];
                for parameter in lambda.parameter_types() {
                    parameters.push(Box::pin(lower_concrete_type(engine, parameter)).await);
                }
                for effect in lower_concrete_effects(engine, lambda.effect_row()).await {
                    parameters.push(MonoType::new_handler_pointer(effect, engine));
                }
                let return_type = Box::pin(lower_concrete_type(engine, lambda.return_type())).await;
                let signature = MonoType::new_function_signature(parameters, return_type, engine);
                engine.intern(MonoType::Aggregate(AggregateType::Closure(Closure::new(signature))))
            }
            ApplicationView::Pointer(pointer) => {
                let pointee_type = Box::pin(lower_concrete_type(engine, pointer.pointee())).await;
                MonoType::new_pointer(pointee_type, lower_mutability(pointer.mutability()), engine)
            }
            ApplicationView::Error => {
                panic!("compiler-internal invariant violation: error type reached MonoIR")
            }
        },
        Ty::Inference(_) | Ty::PolyVar(_) => {
            panic!(
                "compiler-internal invariant violation: non-concrete type reached MonoIR: {ty:?}"
            )
        }
        Ty::EffectRow(_) => {
            panic!("compiler-internal invariant violation: effect row used as a value type")
        }
    }
}

/// Lowers a concrete, closed effect row using the supplied substitution.
#[extend]
pub async fn lower_effects(
    self: &TrackedEngine,
    effect: &Interned<Ty>,
    substitution: &Subst,
) -> Vec<MonoEffectInstance> {
    let effect = reduce_fully(effect.apply_subst_or_clone(substitution, self), self);
    lower_concrete_effects(self, &effect).await
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
        "compiler-internal invariant violation: open effect row reached MonoIR"
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
pub fn instantiate_effect(
    self: &TrackedEngine,
    effect_id: GlobalSymbolID,
    substitution: &Subst,
    owner_substitution: &Subst,
) -> MonoEffectInstance {
    let mut substitution = substitution.clone();
    substitution.apply_mut_subst(owner_substitution, self);
    MonoEffectInstance::new(effect_id, substitution)
}

/// Derives the callback slots for one concrete effect instantiation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<HandlerLayout>)]
#[extend(name = build_handler_layout, by_val)]
pub struct BuildHandlerLayout {
    /// The concrete effect whose operation signatures determine the layout.
    pub instance: MonoEffectInstance,
}

#[executor(config = Config)]
async fn build_handler_layout_executor(
    key: &BuildHandlerLayout,
    engine: &TrackedEngine,
) -> Interned<HandlerLayout> {
    let instance = &key.instance;
    let members = engine.get_members(instance.effect_id()).await;
    let mut operations = members
        .namable_members()
        .map(|operation| instance.effect_id().target_id.make_global(operation))
        .collect::<Vec<_>>();
    operations.sort_unstable();

    let mut lowered_operations = Vec::with_capacity(operations.len());
    for operation_id in operations {
        let parameters = engine.get_parameter_map(operation_id).await;
        let mut parameter_types = vec![MonoType::new_opaque_pointer(engine)];
        for (_, parameter) in parameters.iter() {
            parameter_types.push(engine.lower_type(parameter.ty(), instance.substitution()).await);
        }
        let return_type = engine.get_return_type(operation_id).await;
        let return_type = engine.lower_type(&return_type, instance.substitution()).await;
        lowered_operations.push(EffectOperation::new(
            operation_id,
            MonoType::new_function_signature(parameter_types, return_type, engine),
        ));
    }
    engine.intern(HandlerLayout::new(instance.clone(), lowered_operations))
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_HANDLER_LAYOUT_EXECUTOR: Registration<Config> =
    Registration::new::<BuildHandlerLayout, BuildHandlerLayoutExecutor>();

const fn lower_mutability(mutability: Mutability) -> PointerMutability {
    match mutability {
        Mutability::Immutable => PointerMutability::Const,
        Mutability::Mutable => PointerMutability::Mut,
    }
}

fn reduce_fully(mut ty: Interned<Ty>, engine: &TrackedEngine) -> Interned<Ty> {
    while let Some(reduced) = ty.reduce(engine) {
        ty = reduced;
    }
    ty
}

use std::collections::BTreeMap;

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_extend::extend;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    parameter::get_parameter_map,
    return_type::get_return_type,
    struct_body::{FieldID, get_struct_body},
};
use rayc_solver::Solver;
use rayc_symbol::{GlobalSymbolID, member::get_members};
use rayc_type::{
    poly_var::build_subst_from_args,
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::{Mutability, Primitive, Ty, application::View as ApplicationView},
};

use crate::instance::{MonoEffectInstance, MonoStructInstance};

/// A fully concrete type with a direct runtime representation.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum MonoType {
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
    pub fn assert_as_struct(&self) -> &Struct {
        match self {
            Self::Aggregate(AggregateType::Struct(struct_)) => struct_,
            _ => {
                panic!("compiler-internal invariant violation: expected struct type, got {self:?}")
            }
        }
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

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Struct {
    instance: MonoStructInstance,
    fields: BTreeMap<FieldID, Interned<MonoType>>,
}

impl Struct {
    #[must_use]
    pub const fn new(
        instance: MonoStructInstance,
        fields: BTreeMap<FieldID, Interned<MonoType>>,
    ) -> Self {
        Self { instance, fields }
    }

    #[must_use]
    pub const fn instance(&self) -> &MonoStructInstance { &self.instance }

    #[must_use]
    pub const fn fields(&self) -> &BTreeMap<FieldID, Interned<MonoType>> { &self.fields }
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
    Struct(Struct),
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum ReturnType {
    Void,
    Value(Interned<MonoType>),
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
                    Primitive::Int32 => MonoType::Int32,
                    Primitive::Float32 => MonoType::Float32,
                    Primitive::Bool => MonoType::Bool,
                    Primitive::CInt => MonoType::CInt,
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
                let struct_body = engine.get_struct_body(struct_view.symbol_id()).await;

                // Lower each struct field's type under the concrete type argument substitution.
                let mut fields = BTreeMap::new();
                for (field_id, field) in struct_body.iter() {
                    let field_type = Box::pin(lower_type(solver, field.ty(), &substitution)).await;
                    fields.insert(field_id, field_type);
                }

                let instance = MonoStructInstance::new(struct_view.symbol_id(), substitution);
                engine.intern(MonoType::Aggregate(AggregateType::Struct(Struct::new(
                    instance, fields,
                ))))
            }
            ApplicationView::InstanceAssociated(_) => {
                panic!("unresolved associated type reached code generation")
            }
            ApplicationView::NoOpDropInstance(_)
            | ApplicationView::TupleDropInstance(_)
            | ApplicationView::ClosureDropInstance(_)
            | ApplicationView::NominalDropInstance(_)
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
    let solver = Solver::without_givens(engine.clone()).await;
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
            parameter_types
                .push(lower_type(&solver, parameter.ty(), instance.substitution()).await);
        }
        let return_type = engine.get_return_type(operation_id).await;
        let return_type = lower_type(&solver, &return_type, instance.substitution()).await;
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

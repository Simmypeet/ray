//! The layouts of nominal aggregates: what fields a value of each type has, and
//! the tags that tell its variants apart.
//!
//! A nominal aggregate type only names what it is, so lowering a type never
//! lowers its fields. The queries here derive those fields on demand.
//!
//! The fields of a `coroutine_frame_t` come from the body of the suspended
//! function, so [`BuildCoroutineFrame`] reads its semantic IR.

use std::collections::{BTreeMap, hash_map::Entry};

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_arena::{ID, OrderedArena};
use rayc_hash::FxHashMap;
use rayc_ir::{
    get_ir,
    ir_closure::{CaptureID, CaptureMapID},
    ir_function::{IRContext, IRFunctionID, IRFunctionMap},
};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    parameter::get_parameter_map,
    struct_body::{FieldID, get_struct_body},
};
use rayc_solver::Solver;
use rayc_symbol::{GlobalSymbolID, member::get_members};
use rayc_type::{
    signature::get_return_type,
    ty::application::{ClosureID, ConcreteCoroutineView},
};

use crate::{
    instance::{MonoDefInstance, MonoEffectInstance, MonoEnvironmentInstance, MonoStructInstance},
    operand::Constant,
    ty::{EnvironmentSlot, FunctionSignature, MonoType, ReturnType, lower_effects, lower_type},
};

pub mod coroutine;

/// The unsigned integer type of a tag, which tells the variant of a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagType {
    Uint8,
    Uint16,
    Uint32,
    Uint64,
}

impl TagType {
    /// The narrowest type that can tell `count` variants apart, whose tags are
    /// `0..count`.
    #[must_use]
    pub fn for_variant_count(count: usize) -> Self {
        let largest = count.saturating_sub(1);
        if u8::try_from(largest).is_ok() {
            Self::Uint8
        } else if u16::try_from(largest).is_ok() {
            Self::Uint16
        } else if u32::try_from(largest).is_ok() {
            Self::Uint32
        } else {
            Self::Uint64
        }
    }

    /// The runtime representation of the tag.
    #[must_use]
    pub const fn mono_type(self) -> MonoType {
        match self {
            Self::Uint8 => MonoType::Uint8,
            Self::Uint16 => MonoType::Uint16,
            Self::Uint32 => MonoType::Uint32,
            Self::Uint64 => MonoType::Uint64,
        }
    }

    /// The tag `value` as a constant of this type.
    ///
    /// # Panics
    ///
    /// Panics if `value` does not fit, so a tag is never silently truncated.
    #[must_use]
    pub fn constant(self, value: u64) -> Constant {
        const FITS: &str = "a tag should fit the type chosen for its variant count";
        match self {
            Self::Uint8 => Constant::Uint8(value.try_into().expect(FITS)),
            Self::Uint16 => Constant::Uint16(value.try_into().expect(FITS)),
            Self::Uint32 => Constant::Uint32(value.try_into().expect(FITS)),
            Self::Uint64 => Constant::Uint64(value),
        }
    }
}

/// The fields of one concrete struct.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct StructLayout {
    instance: MonoStructInstance,
    fields: BTreeMap<FieldID, Interned<MonoType>>,
}

impl StructLayout {
    #[must_use]
    pub const fn instance(&self) -> &MonoStructInstance { &self.instance }

    #[must_use]
    pub const fn fields(&self) -> &BTreeMap<FieldID, Interned<MonoType>> { &self.fields }
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
pub struct EffectHandlerCallbackLayout {
    instance: MonoEffectInstance,
    operations: Vec<EffectOperation>,
}

impl EffectHandlerCallbackLayout {
    #[must_use]
    pub const fn new(instance: MonoEffectInstance, operations: Vec<EffectOperation>) -> Self {
        Self { instance, operations }
    }

    #[must_use]
    pub const fn instance(&self) -> &MonoEffectInstance { &self.instance }

    #[must_use]
    pub fn operations(&self) -> &[EffectOperation] { &self.operations }
}

/// Derives the callback slots for one concrete effect instantiation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<EffectHandlerCallbackLayout>)]
#[extend(name = build_handler_layout, by_val)]
pub struct BuildHandlerLayout {
    /// The concrete effect whose operation signatures determine the layout.
    pub instance: MonoEffectInstance,
}

#[executor(config = Config)]
async fn build_handler_layout_executor(
    key: &BuildHandlerLayout,
    engine: &TrackedEngine,
) -> Interned<EffectHandlerCallbackLayout> {
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
        let mut parameter_types = Vec::new();
        for (_, parameter) in parameters.iter() {
            parameter_types
                .push(lower_type(&solver, parameter.ty(), instance.substitution()).await);
        }
        let return_type = engine.get_return_type(operation_id).await;
        let return_type = lower_type(&solver, &return_type, instance.substitution()).await;

        // The callback is called without knowing its environment.
        let signature = FunctionSignature::builder()
            .parameter_types(engine.intern_unsized(parameter_types))
            .return_type(ReturnType::Value(return_type))
            .environment_slot(EnvironmentSlot::Opaque)
            .engine(engine)
            .build();

        lowered_operations.push(EffectOperation::new(operation_id, signature));
    }
    engine.intern(EffectHandlerCallbackLayout::new(instance.clone(), lowered_operations))
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_HANDLER_LAYOUT_EXECUTOR: Registration<Config> =
    Registration::new::<BuildHandlerLayout, BuildHandlerLayoutExecutor>();

/// Derives the fields of one concrete struct.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<StructLayout>)]
#[extend(name = build_struct_layout, by_val)]
pub struct BuildStructLayout {
    /// The concrete struct whose declared fields determine the layout.
    pub instance: MonoStructInstance,
}

#[executor(config = Config)]
async fn build_struct_layout_executor(
    key: &BuildStructLayout,
    engine: &TrackedEngine,
) -> Interned<StructLayout> {
    let instance = &key.instance;
    let solver = Solver::without_givens(engine.clone()).await;
    let struct_body = engine.get_struct_body(instance.struct_id()).await;

    // Lower each field's type under the concrete type argument substitution.
    let mut fields = BTreeMap::new();
    for (field_id, field) in struct_body.iter() {
        fields.insert(field_id, lower_type(&solver, field.ty(), instance.substitution()).await);
    }

    engine.intern(StructLayout { instance: instance.clone(), fields })
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_STRUCT_LAYOUT_EXECUTOR: Registration<Config> =
    Registration::new::<BuildStructLayout, BuildStructLayoutExecutor>();

/// One handler record that an environment stores.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct HandlerRecord {
    effect: MonoEffectInstance,
}

impl HandlerRecord {
    /// The effect that the record handles.
    #[must_use]
    pub const fn effect(&self) -> &MonoEffectInstance { &self.effect }
}

pub type HandlerRecordID = ID<HandlerRecord>;

/// The members of one concrete capture environment: its captures, and the
/// handler records an operation handler finds in it.
///
/// ```c
/// struct environment {
///     capture_t capture0;                 // one per capture
///     ...
///     const EffectHandler_E *handler0;    // one per handler record
///     ...
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct EnvironmentLayout {
    instance: MonoEnvironmentInstance,
    captures: FxHashMap<CaptureID, Interned<MonoType>>,
    // Ordered, since code generation iterates over the records.
    handler_records: OrderedArena<HandlerRecord>,
    handler_record_ids_by_effect: FxHashMap<MonoEffectInstance, HandlerRecordID>,
}

impl EnvironmentLayout {
    #[must_use]
    pub const fn instance(&self) -> &MonoEnvironmentInstance { &self.instance }

    /// The type of each capture.
    #[must_use]
    pub fn captures(&self) -> impl ExactSizeIterator<Item = (CaptureID, &Interned<MonoType>)> {
        self.captures.iter().map(|(capture, ty)| (*capture, ty))
    }

    #[must_use]
    pub fn capture_type(&self, capture: CaptureID) -> &Interned<MonoType> {
        self.captures.get(&capture).expect("the environment should hold the capture")
    }

    #[must_use]
    pub fn handler_records(
        &self,
    ) -> impl ExactSizeIterator<Item = (HandlerRecordID, &HandlerRecord)> {
        self.handler_records.iter()
    }

    /// The record that holds the handler of `effect`.
    #[must_use]
    pub fn get_handler_record_id(&self, effect: &MonoEffectInstance) -> HandlerRecordID {
        *self
            .handler_record_ids_by_effect
            .get(effect)
            .unwrap_or_else(|| panic!("the environment should hold a handler record of {effect:?}"))
    }

    /// Whether the environment holds nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool { self.captures.is_empty() && self.handler_records.is_empty() }
}

/// Derives the members of one concrete capture environment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<EnvironmentLayout>)]
#[extend(name = build_environment_layout, by_val)]
pub struct BuildEnvironmentLayout {
    /// The environment whose captures determine the layout.
    pub instance: MonoEnvironmentInstance,
}

#[executor(config = Config)]
async fn build_environment_layout_executor(
    key: &BuildEnvironmentLayout,
    engine: &TrackedEngine,
) -> Interned<EnvironmentLayout> {
    let instance = &key.instance;
    let owner = instance.owner();
    let solver = Solver::without_givens(engine.clone()).await;
    let source = engine.get_ir(owner.def_id()).await;
    let capture_map = match instance {
        MonoEnvironmentInstance::Closure(closure) => {
            source.capture_map_id(source.get_function_id_of_closure(closure.closure_id()))
        }
        MonoEnvironmentInstance::Nested { capture_map, .. } => *capture_map,
    };

    // Each capture is stored as its binding, or as a reference to it.
    let mut captures = FxHashMap::default();
    for (capture_id, capture) in source.get_capture_map(capture_map).iter() {
        let ty = lower_type(&solver, &capture.storage_ty(engine), owner.substitution()).await;
        captures.insert(capture_id, ty);
    }

    // An operation handler is called through the slot of a handler record,
    // which has no handler-record slots, so the handlers it performs with are
    // in its environment instead.
    let (handler_records, handler_record_ids_by_effect) =
        handled_effects_of_capture_map(&source, capture_map, owner, &solver).await;

    engine.intern(EnvironmentLayout {
        instance: instance.clone(),
        captures,
        handler_records,
        handler_record_ids_by_effect,
    })
}

/// The effects whose handlers the operation handlers of `capture_map` perform
/// with, or nothing if no operation handler uses it. The clauses of one handler
/// record share an environment, so they must perform the same effects.
async fn handled_effects_of_capture_map(
    source: &IRFunctionMap,
    capture_map: CaptureMapID,
    owner: &MonoDefInstance,
    solver: &Solver,
) -> (OrderedArena<HandlerRecord>, FxHashMap<MonoEffectInstance, HandlerRecordID>) {
    let engine = solver.engine();

    let mut handler_records = OrderedArena::new();
    let mut handler_record_ids_by_effect: FxHashMap<MonoEffectInstance, HandlerRecordID> =
        FxHashMap::default();

    for (function_id, function) in source.functions() {
        let context = function.context();

        // callback handler records are only captured into environments iff the
        // capture is used for the operation handler
        if context.capture_map_id() != Some(capture_map)
            || !matches!(context, IRContext::OperationHandler(_))
        {
            continue;
        }

        let effect = source.effect_of(function_id, engine).await;
        let lowered = lower_effects(solver, &effect, owner.substitution()).await;

        for mono_effect_inst in lowered {
            match handler_record_ids_by_effect.entry(mono_effect_inst.clone()) {
                Entry::Occupied(_) => {}
                Entry::Vacant(vacant_entry) => {
                    let id = handler_records.insert(HandlerRecord { effect: mono_effect_inst });
                    vacant_entry.insert(id);
                }
            }
        }
    }

    (handler_records, handler_record_ids_by_effect)
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_ENVIRONMENT_LAYOUT_EXECUTOR: Registration<Config> =
    Registration::new::<BuildEnvironmentLayout, BuildEnvironmentLayoutExecutor>();

/// The function a coroutine suspends, in the IR of its owning definition.
#[derive(Debug)]
pub struct CoroutineBody {
    owner: MonoDefInstance,
    closure: Option<ClosureID>,
}

impl CoroutineBody {
    pub async fn of(view: ConcreteCoroutineView<'_>, solver: &Solver) -> Self {
        let engine = solver.engine();
        match view {
            ConcreteCoroutineView::Def(coroutine) => {
                let substitution = coroutine.create_subst(engine).await;
                let owner = MonoDefInstance::new(coroutine.def_id(), substitution, solver).await;
                Self { owner, closure: None }
            }
            ConcreteCoroutineView::Closure(coroutine) => {
                let closure = coroutine.closure_view();
                let substitution = closure.create_subst(engine).await;
                let owner = MonoDefInstance::new(closure.owner_id(), substitution, solver).await;
                Self { owner, closure: Some(closure.local_closure_id()) }
            }
        }
    }

    #[must_use]
    pub fn into_owner(self) -> MonoDefInstance { self.owner }

    #[must_use]
    pub const fn def_id(&self) -> GlobalSymbolID { self.owner.def_id() }

    #[must_use]
    pub fn function_id(&self, source: &IRFunctionMap) -> IRFunctionID {
        self.closure
            .map_or_else(|| source.root_id(), |closure| source.get_function_id_of_closure(closure))
    }
}

#[cfg(test)]
mod tests {
    use super::TagType;

    /// Input: variant counts at each boundary of the unsigned integer types.
    /// Premise: tags are `0..count`, so a type fits one more variant than its
    /// largest value. Output: the narrowest type that holds the largest tag.
    #[test]
    fn tag_type_is_the_narrowest_that_holds_the_largest_tag() {
        assert_eq!(TagType::for_variant_count(0), TagType::Uint8);
        assert_eq!(TagType::for_variant_count(256), TagType::Uint8);
        assert_eq!(TagType::for_variant_count(257), TagType::Uint16);
        assert_eq!(TagType::for_variant_count(1 << 16), TagType::Uint16);
        assert_eq!(TagType::for_variant_count((1 << 16) + 1), TagType::Uint32);
        assert_eq!(TagType::for_variant_count(1 << 32), TagType::Uint32);
        assert_eq!(TagType::for_variant_count((1 << 32) + 1), TagType::Uint64);
    }
}

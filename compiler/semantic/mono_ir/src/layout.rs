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
    ir_function::{IRContext, IRFunctionMap},
};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    parameter::get_parameter_map,
    struct_body::{FieldID, get_struct_body},
    variant_payload_ty::get_variant_payload_ty,
};
use rayc_solver::Solver;
use rayc_symbol::{
    GlobalSymbolID,
    member::{get_enum_variants, get_members},
};
use rayc_type::{signature::get_return_type, subst::Subst};

use crate::{
    instance::{MonoEffectInstance, MonoEnumInstance, MonoEnvironmentInstance, MonoStructInstance},
    operand::Constant,
    ty::{
        AggregateType, EnvironmentSlot, FunctionSignature, MonoType, ReturnType, lower_effects,
        lower_type,
    },
};

pub mod coroutine;
pub mod recursion;
pub mod repr;
pub mod run_with;

/// The unsigned integer type of a tag, which tells the variant of a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
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

    /// The largest value of the type.
    #[must_use]
    pub const fn max_value(self) -> u64 {
        match self {
            Self::Uint8 => u8::MAX as u64,
            Self::Uint16 => u16::MAX as u64,
            Self::Uint32 => u32::MAX as u64,
            Self::Uint64 => u64::MAX,
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

/// The position of a variant among the variants of its enum, as an
/// [`EnumLayout`] orders them. The discriminant of a value is the index of its
/// variant.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct VariantIndex(u32);

impl VariantIndex {
    /// The variant at a position counted in `usize`, as an iterator gives it.
    ///
    /// # Panics
    ///
    /// Panics if `index` does not fit in `u32`.
    #[must_use]
    pub fn from_usize(index: usize) -> Self {
        Self(index.try_into().expect("a variant index should fit in u32"))
    }

    #[must_use]
    pub const fn index(self) -> u32 { self.0 }
}

/// One variant of a concrete enum.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct EnumVariant {
    variant_id: GlobalSymbolID,
    payload: Option<Interned<MonoType>>,
}

impl EnumVariant {
    /// The `SymbolKind::Variant` symbol of the variant.
    #[must_use]
    pub const fn variant_id(&self) -> GlobalSymbolID { self.variant_id }

    /// The type of the value the variant carries, or `None` for a variant
    /// declared without one.
    #[must_use]
    pub const fn payload(&self) -> Option<&Interned<MonoType>> { self.payload.as_ref() }
}

/// The variants of one concrete enum, each with the type of its payload.
///
/// The variants are in declaration order, which is also the order the enum's
/// generated `Drop` plan visits them in.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct EnumLayout {
    instance: MonoEnumInstance,
    variants: Vec<EnumVariant>,
}

impl EnumLayout {
    #[must_use]
    pub const fn instance(&self) -> &MonoEnumInstance { &self.instance }

    /// Every variant with its index, in order.
    #[must_use]
    pub fn variants(&self) -> impl ExactSizeIterator<Item = (VariantIndex, &EnumVariant)> {
        self.variants
            .iter()
            .enumerate()
            .map(|(index, variant)| (VariantIndex::from_usize(index), variant))
    }

    /// The variant at `index`.
    #[must_use]
    pub fn variant(&self, index: VariantIndex) -> &EnumVariant {
        &self.variants[index.index() as usize]
    }

    /// The index of the variant whose symbol is `variant_id`.
    ///
    /// # Panics
    ///
    /// Panics if the enum has no such variant.
    #[must_use]
    pub fn variant_index(&self, variant_id: GlobalSymbolID) -> VariantIndex {
        let index = self
            .variants
            .iter()
            .position(|variant| variant.variant_id == variant_id)
            .expect("the variant should be one of its enum's");
        VariantIndex::from_usize(index)
    }

    /// The type of a discriminant: the narrowest that holds the index of
    /// every variant.
    #[must_use]
    pub fn discriminant_type(&self) -> TagType { TagType::for_variant_count(self.variants.len()) }

    /// Whether a value stores which variant it is: only an enum with more than
    /// one variant needs to.
    #[must_use]
    pub const fn has_tag(&self) -> bool { self.variants.len() > 1 }
}

/// Derives the [`EnumLayout`] of a concrete enum.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<EnumLayout>)]
#[extend(name = build_enum_layout, by_val)]
pub struct BuildEnumLayout {
    /// The concrete enum whose declared variants determine the layout.
    pub instance: MonoEnumInstance,
}

#[executor(config = Config)]
async fn build_enum_layout_executor(
    key: &BuildEnumLayout,
    engine: &TrackedEngine,
) -> Interned<EnumLayout> {
    let instance = &key.instance;
    let solver = Solver::without_givens(engine.clone()).await;
    let enum_id = instance.enum_id();

    // lower each payload's type under the concrete type argument substitution
    let variant_ids = engine.get_enum_variants(enum_id).await;
    let mut variants = Vec::with_capacity(variant_ids.len());
    for variant_id in variant_ids.iter().copied() {
        let variant_id = enum_id.target_id.make_global(variant_id);
        let payload = match engine.get_variant_payload_ty(variant_id).await {
            Some(payload) => Some(lower_type(&solver, &payload, instance.substitution()).await),
            None => None,
        };
        variants.push(EnumVariant { variant_id, payload });
    }

    engine.intern(EnumLayout { instance: instance.clone(), variants })
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_ENUM_LAYOUT_EXECUTOR: Registration<Config> =
    Registration::new::<BuildEnumLayout, BuildEnumLayoutExecutor>();

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

    /// The callback slot of `operation_id`.
    ///
    /// # Panics
    ///
    /// Panics if the effect has no such operation.
    #[must_use]
    pub fn operation(&self, operation_id: GlobalSymbolID) -> &EffectOperation {
        self.operations
            .iter()
            .find(|operation| operation.operation_id == operation_id)
            .expect("an effect handler record should have a slot for each of its operations")
    }
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
    /// The type of each capture.
    captures: FxHashMap<CaptureID, Interned<MonoType>>,
    /// The captures in the order of the IR capture map, which is the order of
    /// the capture expressions that build the environment.
    capture_order: Vec<CaptureID>,
    // Ordered, since code generation iterates over the records.
    handler_records: OrderedArena<HandlerRecord>,
    handler_record_ids_by_effect: FxHashMap<MonoEffectInstance, HandlerRecordID>,
}

impl EnvironmentLayout {
    #[must_use]
    pub const fn instance(&self) -> &MonoEnvironmentInstance { &self.instance }

    /// The type of each capture, in the order of the capture expressions that
    /// build the environment.
    #[must_use]
    pub fn captures(
        &self,
    ) -> impl ExactSizeIterator<Item = (CaptureID, &Interned<MonoType>)> + DoubleEndedIterator {
        self.capture_ids().map(|capture| (capture, self.capture_type(capture)))
    }

    /// The captures, in the order of the capture expressions that build the
    /// environment.
    #[must_use]
    pub fn capture_ids(
        &self,
    ) -> impl ExactSizeIterator<Item = CaptureID> + DoubleEndedIterator + '_ {
        self.capture_order.iter().copied()
    }

    /// The type of the environment itself.
    #[must_use]
    pub fn environment_type(&self) -> MonoType {
        MonoType::Aggregate(AggregateType::Environment(self.instance.clone()))
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

    /// The handler record `record_id`.
    ///
    /// # Panics
    ///
    /// Panics if the environment holds no such record.
    #[must_use]
    pub fn handler_record(&self, record_id: HandlerRecordID) -> &HandlerRecord {
        self.handler_records.get(record_id).expect("the environment should hold the handler record")
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
    let substitution = instance.substitution();
    let solver = Solver::without_givens(engine.clone()).await;
    let source = engine.get_ir(instance.def_id()).await;
    let capture_map = instance.capture_map();

    // Each capture is stored as its binding, or as a reference to it. The
    // order of the capture map is kept as is.
    let ordered_captures = source.get_capture_map(capture_map).ordered_captures();
    let mut captures = FxHashMap::default();
    let mut capture_order = Vec::with_capacity(ordered_captures.len());
    for (capture_id, capture) in ordered_captures {
        let ty = lower_type(&solver, &capture.storage_ty(engine), substitution).await;
        captures.insert(capture_id, ty);
        capture_order.push(capture_id);
    }

    // An operation handler is called through the slot of a handler record,
    // which has no handler-record slots, so the handlers it performs with are
    // in its environment instead.
    let (handler_records, handler_record_ids_by_effect) =
        handled_effects_of_capture_map(&source, capture_map, substitution, &solver).await;

    engine.intern(EnvironmentLayout {
        instance: instance.clone(),
        captures,
        capture_order,
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
    substitution: &Subst,
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
        let lowered = lower_effects(solver, &effect, substitution).await;

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

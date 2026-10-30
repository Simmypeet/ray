//! The layouts of nominal aggregates: what fields a value of each type has, and
//! the tags that tell its variants apart.
//!
//! A nominal aggregate type only names what it is, so lowering a type never
//! lowers its fields. The queries here derive those fields on demand.
//!
//! The fields of a `coroutine_frame_t` come from the body of the suspended
//! function, so [`BuildCoroutineFrame`] reads its semantic IR.

use std::collections::BTreeMap;

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_coroutine_frame_layout::{
    frame_layout::{FrameLayout, Placement, SuspensionLocalPlacement},
    savable::Savable,
};
use rayc_hash::FxHashMap;
use rayc_ir::{
    address::Local,
    cfg::BlockID as IRBlockID,
    get_ir,
    ir_function::{IRFunction, IRFunctionID, IRFunctionMap},
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
    ty::{
        Ty,
        application::{ClosureID, ConcreteCoroutineView},
    },
};

use crate::{
    instance::{MonoCoroutineInstance, MonoDefInstance, MonoEffectInstance, MonoStructInstance},
    operand::Constant,
    place::{FieldIndex, Place, Projection},
    ty::{AggregateType, EffectRow, FunctionSignature, MonoType, ReturnType, Tuple, lower_type},
};

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

/// One operation of a concrete effect row.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct RowOperation {
    effect: MonoEffectInstance,
    operation_id: GlobalSymbolID,
    arguments: Interned<MonoType>,
}

impl RowOperation {
    /// The tuple of the operation's arguments, which is what a coroutine
    /// yields when it performs the operation.
    #[must_use]
    pub const fn arguments(&self) -> &Interned<MonoType> { &self.arguments }
}

/// The members of the `coroutine_resume_t` and `coroutine_yield_t` of one
/// concrete effect row.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct EffectRowLayout {
    operations: Vec<RowOperation>,
    resume_types: Vec<Interned<MonoType>>,
}

impl EffectRowLayout {
    /// The operations of the row. An operation's position is its yield tag.
    #[must_use]
    pub fn operations(&self) -> &[RowOperation] { &self.operations }

    /// The type of the tag of a `coroutine_yield_t`, which tells the operations
    /// of the row apart.
    #[must_use]
    pub fn yield_tag_type(&self) -> TagType { TagType::for_variant_count(self.operations.len()) }

    /// The tag of the operation at position `operation`.
    #[must_use]
    pub fn yield_tag(&self, operation: FieldIndex) -> Constant {
        self.yield_tag_type().constant(u64::from(operation.index()))
    }

    /// The distinct types a coroutine of this row can be resumed with.
    ///
    /// `coroutine_resume_t` has one member per type, not per operation: a
    /// `Continuation[r, c]` that is resumed knows the `r` it waits for, not
    /// which operation it suspended at. Unit is always first, since it is what
    /// a coroutine that has not started yet is resumed with.
    #[must_use]
    pub fn resume_types(&self) -> &[Interned<MonoType>] { &self.resume_types }

    /// The member of `coroutine_resume_t` that holds a value of type `ty`.
    #[must_use]
    pub fn resume_index(&self, ty: &Interned<MonoType>) -> FieldIndex {
        let index = self
            .resume_types
            .iter()
            .position(|resume_type| resume_type == ty)
            .expect("a coroutine should only be resumed with a type its row resumes with");
        FieldIndex::new(index.try_into().expect("a resume type index should fit in u32"))
    }

    /// The position of `operation_id` of `effect` in the row, which is its
    /// yield tag.
    #[must_use]
    pub fn operation_index(
        &self,
        effect: &MonoEffectInstance,
        operation_id: GlobalSymbolID,
    ) -> FieldIndex {
        let index = self
            .operations
            .iter()
            .position(|operation| {
                operation.operation_id == operation_id && &operation.effect == effect
            })
            .expect("an operation of a coroutine should be in its effect row");
        FieldIndex::new(index.try_into().expect("an operation index should fit in u32"))
    }
}

/// The fields of the `coroutine_frame_t` of one concrete coroutine.
///
/// The frame is `{ state; prefix; start; union { variants } }`. Start fields
/// stay out of the union: they are not candidates of the storage conflict
/// analysis, so a value of a suspension variant may be written while one of
/// them is still needed.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct CoroutineFrameLayout {
    prefix: Vec<Interned<MonoType>>,
    start: Vec<Interned<MonoType>>,
    variants: Vec<Vec<Interned<MonoType>>>,
}

impl CoroutineFrameLayout {
    /// The states that are not a suspension: not started, and completed.
    const NON_SUSPENDED_STATES: usize = 2;

    #[must_use]
    pub const fn new(
        prefix: Vec<Interned<MonoType>>,
        start: Vec<Interned<MonoType>>,
        variants: Vec<Vec<Interned<MonoType>>>,
    ) -> Self {
        Self { prefix, start, variants }
    }

    /// The type of the frame's state, which tells its states apart.
    #[must_use]
    pub fn state_type(&self) -> TagType {
        TagType::for_variant_count(self.variants.len() + Self::NON_SUSPENDED_STATES)
    }

    /// The state of a coroutine that has not been resumed yet.
    #[must_use]
    pub fn start_state(&self) -> Constant { self.state_type().constant(0) }

    /// The state of a coroutine that has returned.
    #[must_use]
    pub fn completed_state(&self) -> Constant { self.state_type().constant(1) }

    /// The state of a coroutine suspended at the given variant.
    #[must_use]
    pub fn suspended_state(&self, variant: FieldIndex) -> Constant {
        self.state_type().constant(u64::from(variant.index()) + 2)
    }

    #[must_use]
    pub fn prefix(&self) -> &[Interned<MonoType>] { &self.prefix }

    #[must_use]
    pub fn start(&self) -> &[Interned<MonoType>] { &self.start }

    /// The fields of each suspension variant, which overlap in memory.
    #[must_use]
    pub fn variants(&self) -> &[Vec<Interned<MonoType>>] { &self.variants }
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

/// Derives the members of the `coroutine_resume_t` and `coroutine_yield_t` of
/// one concrete effect row.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<EffectRowLayout>)]
#[extend(name = build_effect_row_layout, by_val)]
pub struct BuildEffectRowLayout {
    /// The row whose operations determine the layout.
    pub row: EffectRow,
}

#[executor(config = Config)]
async fn build_effect_row_layout_executor(
    key: &BuildEffectRowLayout,
    engine: &TrackedEngine,
) -> Interned<EffectRowLayout> {
    let mut operations = Vec::new();
    let mut resume_types = vec![MonoType::new_unit(engine)]; // starting a coroutine is always resumed with unit type

    for effect in key.row.effects() {
        let layout = engine.build_handler_layout(effect.clone()).await;

        for operation in layout.operations() {
            // The handler ABI starts with the environment, which a yield has
            // no use for.
            let signature = operation.signature();
            let arguments = engine.intern_unsized(signature.parameter_types()[1..].to_vec());
            operations.push(RowOperation {
                effect: effect.clone(),
                operation_id: operation.operation_id(),
                arguments: engine
                    .intern(MonoType::Aggregate(AggregateType::Tuple(Tuple::new(arguments)))),
            });

            let ReturnType::Value(resume_type) = signature.return_type() else {
                panic!("an operation handler returns a value")
            };
            if !resume_types.contains(resume_type) {
                resume_types.push(resume_type.clone());
            }
        }
    }

    engine.intern(EffectRowLayout { operations, resume_types })
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_EFFECT_ROW_LAYOUT_EXECUTOR: Registration<Config> =
    Registration::new::<BuildEffectRowLayout, BuildEffectRowLayoutExecutor>();

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

/// Where a value is stored in a `coroutine_frame_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, StableHash, Encode, Decode)]
pub enum FrameSlot {
    Prefix(FieldIndex),
    Start(FieldIndex),
    Variant { variant: FieldIndex, field: FieldIndex },
}

impl FrameSlot {
    /// The place of this slot in the frame stored at `frame`.
    #[must_use]
    pub fn project(self, frame: &Place, engine: &TrackedEngine) -> Place {
        frame.project(engine).with_projection(match self {
            Self::Prefix(field) => Projection::CoroutineFramePrefixField(field),
            Self::Start(field) => Projection::CoroutineFrameStartField(field),
            Self::Variant { variant, field } => {
                Projection::CoroutineFrameVariantField { variant, field }
            }
        })
    }
}

/// What a value stored in a coroutine that has not started comes from, in the
/// arguments of the `Coroutine` expression creating it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, StableHash, Encode, Decode)]
pub enum Upvar {
    /// The argument at this position of a suspended definition.
    Argument(usize),
    /// A capture in the environment of a suspended closure, its first
    /// argument.
    ClosureCapture(FieldIndex),
    /// An element of the argument tuple of a suspended closure, its second
    /// argument.
    ClosureArgument(FieldIndex),
}

impl Upvar {
    /// The place of this value, given the places of the `Coroutine`
    /// expression's arguments.
    pub fn place(self, arguments: &[Place], engine: &TrackedEngine) -> Place {
        match self {
            Self::Argument(index) => arguments[index].clone(),
            Self::ClosureCapture(field) => arguments[0].project(engine).environment_field(field),
            Self::ClosureArgument(field) => arguments[1].project(engine).tuple_field(field),
        }
    }
}

/// The frame of one concrete coroutine: where its layout places every value, in
/// terms of the fields of a `coroutine_frame_t`.
///
/// Field order comes from sorting [`Savable`]s rather than from the placement
/// map, so the generated struct does not depend on hash iteration order.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct CoroutineFrame {
    layout: Interned<CoroutineFrameLayout>,
    slots: FxHashMap<Savable, FrameSlot>,
    /// The block that ends in the suspension point of each variant.
    variants: Vec<IRBlockID>,
    upvars: Vec<(Upvar, FrameSlot)>,
}

impl CoroutineFrame {
    /// The fields of the frame.
    #[must_use]
    pub const fn layout(&self) -> &Interned<CoroutineFrameLayout> { &self.layout }

    /// The slot of `savable`, if the frame holds it.
    #[must_use]
    pub fn slot(&self, savable: Savable) -> Option<FrameSlot> { self.slots.get(&savable).copied() }

    /// The variant of the suspension point that terminates `block`, if the
    /// function still suspends there.
    #[must_use]
    pub fn variant_at(&self, block: IRBlockID) -> Option<FieldIndex> {
        let index = self.variants.iter().position(|variant| *variant == block)?;
        Some(FieldIndex::from_usize(index))
    }

    /// What is stored in a coroutine when it is created, with where.
    #[must_use]
    pub fn upvars(&self) -> impl ExactSizeIterator<Item = (Upvar, FrameSlot)> + '_ {
        self.upvars.iter().copied()
    }
}

/// Derives the frame of a concrete coroutine.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<CoroutineFrame>)]
#[extend(name = build_coroutine_frame, by_val)]
pub struct BuildCoroutineFrame {
    /// The coroutine whose frame is requested.
    pub instance: MonoCoroutineInstance,
}

#[executor(config = Config)]
async fn build_coroutine_frame_executor(
    key: &BuildCoroutineFrame,
    engine: &TrackedEngine,
) -> Interned<CoroutineFrame> {
    let mut solver = Solver::without_givens(engine.clone()).await;
    let body = CoroutineBody::of(key.instance.view(), &solver).await;
    let def_id = body.def_id();
    let source = engine.get_ir(def_id).await;
    let function_id = body.function_id(&source);
    let substitution = body.owner.substitution().clone();
    let layout = FrameLayout::compute(
        engine.clone(),
        &mut solver,
        def_id,
        function_id,
        substitution.clone(),
    )
    .await;

    // Every value in a canonical order, and every variant in the order of its
    // block, which is also the order of its state.
    let mut placements = layout.placements().collect::<Vec<_>>();
    placements.sort_unstable();
    let mut variants = layout.suspension_points().map(|point| point.block_id()).collect::<Vec<_>>();
    variants.sort_unstable();

    // Lay every value out in the part of the frame that its placement chose.
    let function = source.get_function(function_id);
    let mut prefix = Vec::new();
    let mut start = Vec::new();
    let mut variant_fields = vec![Vec::new(); variants.len()];
    let mut slots = FxHashMap::default();
    for (savable, placement) in placements {
        let ty = savable_type(engine, &source, function, def_id, function_id, savable).await;
        let ty = lower_type(&solver, &ty, &substitution).await;

        let slot = match placement {
            Placement::Prefix => {
                prefix.push(ty);
                FrameSlot::Prefix(FieldIndex::from_usize(prefix.len() - 1))
            }
            Placement::SuspensionLocal(SuspensionLocalPlacement::Start) => {
                start.push(ty);
                FrameSlot::Start(FieldIndex::from_usize(start.len() - 1))
            }
            Placement::SuspensionLocal(SuspensionLocalPlacement::SuspensionLocal(point)) => {
                let variant = variants
                    .binary_search(&point.block_id())
                    .expect("a placed suspension point should have a variant");
                variant_fields[variant].push(ty);
                FrameSlot::Variant {
                    variant: FieldIndex::from_usize(variant),
                    field: FieldIndex::from_usize(variant_fields[variant].len() - 1),
                }
            }
        };
        slots.insert(savable, slot);
    }

    let upvars = upvars(engine, &source, function, def_id, function_id, &slots).await;
    let layout = engine.intern(CoroutineFrameLayout::new(prefix, start, variant_fields));
    engine.intern(CoroutineFrame { layout, slots, variants, upvars })
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_COROUTINE_FRAME_EXECUTOR: Registration<Config> =
    Registration::new::<BuildCoroutineFrame, BuildCoroutineFrameExecutor>();

/// The type of a value that a frame holds, as the suspended function spells it.
async fn savable_type(
    engine: &TrackedEngine,
    source: &IRFunctionMap,
    function: &IRFunction,
    def_id: GlobalSymbolID,
    function_id: IRFunctionID,
    savable: Savable,
) -> Interned<Ty> {
    match savable {
        Savable::Expression(expression) => function.get_expression(expression).ty().clone(),
        Savable::Local(Local::Variable(variable)) => function.get_variable(variable).ty().clone(),
        Savable::Local(Local::Parameter(parameter)) => {
            engine.get_parameter_map(def_id).await[parameter].ty().clone()
        }
        Savable::Local(Local::ClosureParameter(parameter)) => {
            function.context().assert_as_closure_context().get_parameter(parameter).ty().clone()
        }
        Savable::Local(Local::Capture(capture)) => {
            source.get_capture(function_id, capture).storage_ty(engine)
        }
        Savable::Local(Local::OperationHandlerParameter(_)) => {
            panic!("the coroutine form of an operation handler is not lowered to MonoIR yet")
        }
        Savable::Awaitee(_) => {
            panic!("forwarding the operations of an awaited coroutine is not lowered to MonoIR yet")
        }
    }
}

/// Pairs every value a coroutine stores when it is created with its slot.
async fn upvars(
    engine: &TrackedEngine,
    source: &IRFunctionMap,
    function: &IRFunction,
    def_id: GlobalSymbolID,
    function_id: IRFunctionID,
    slots: &FxHashMap<Savable, FrameSlot>,
) -> Vec<(Upvar, FrameSlot)> {
    let slot_of = |local| slots[&Savable::Local(local)];

    // A definition stores its arguments as they are.
    let parameters = function.parameter_locals(def_id, engine).await;
    if function.context().capture_map_id().is_none() {
        return parameters
            .into_iter()
            .enumerate()
            .map(|(index, local)| (Upvar::Argument(index), slot_of(local)))
            .collect();
    }

    // `Def.call(closure, arguments)` takes the closure value and a tuple, which
    // the frame stores as the captures and parameters of the closure's body.
    let captures = source.captures(function_id).enumerate().map(|(index, (capture, _))| {
        (Upvar::ClosureCapture(FieldIndex::from_usize(index)), slot_of(Local::Capture(capture)))
    });
    let arguments = parameters.iter().enumerate().map(|(index, local)| {
        (Upvar::ClosureArgument(FieldIndex::from_usize(index)), slot_of(*local))
    });
    captures.chain(arguments).collect()
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

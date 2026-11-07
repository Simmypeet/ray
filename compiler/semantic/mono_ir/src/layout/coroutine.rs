use std::{collections::BTreeMap, ops::Index};

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_arena::{ID, OrderedArena};
use rayc_coroutine_frame_layout::{
    frame_layout::FrameLayout, savable::Savable, suspension_point::SuspensionPointID,
};
use rayc_hash::FxHashMap;
use rayc_ir::{
    address::Local,
    cfg::Terminator,
    get_ir,
    ir_expr::call::Call,
    ir_function::{IRFunction, IRFunctionID, IRFunctionMap},
    suspension_point::Waitee,
};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::parameter::get_parameter_map;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    subst::{Subst, Substitutable},
    ty::{Ty, application::View as ApplicationView},
};

use crate::{
    MonoCoroutineInstance, MonoEffectInstance,
    layout::{TagType, build_handler_layout as _},
    operand::Constant,
    ty::{AggregateType, EffectRow, MonoType, ReturnType, Tuple, lower_type},
};

/// Builds the [`FrameLayout`] of a concrete coroutine instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<FrameLayout>)]
#[extend(name = build_frame_layout, by_val)]
pub struct BuildFrameLayout {
    pub key: MonoCoroutineInstance,
}

#[executor(config = Config)]
async fn build_frame_layout_executor(
    key: &BuildFrameLayout,
    engine: &TrackedEngine,
) -> Interned<FrameLayout> {
    let mut solver = Solver::without_givens(engine.clone()).await;
    let body = key.key.body();

    let frame_layout = FrameLayout::compute(
        engine.clone(),
        &mut solver,
        body.def_id(),
        body.ir_function_id(),
        body.substitution().clone(),
    )
    .await;

    engine.intern(frame_layout)
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_FRAME_LAYOUT_EXECUTOR: Registration<Config> =
    Registration::new::<BuildFrameLayout, BuildFrameLayoutExecutor>();

/// What a coroutine is doing, as the tag of its frame tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameState {
    /// The coroutine has not been resumed yet.
    NotStarted,
    /// The coroutine has returned.
    Completed,
    /// The coroutine is suspended at the given suspension point.
    Suspended(SuspensionPointID),
}

impl FrameState {
    /// The states that are not a suspension.
    const NON_SUSPENDED_STATES: usize = 2;

    /// The type of the tag of a frame with the given layout.
    #[must_use]
    pub fn tag_type(layout: &FrameLayout) -> TagType {
        TagType::for_variant_count(layout.suspension_points().count() + Self::NON_SUSPENDED_STATES)
    }

    /// The tag of this state in a frame with the given layout. Suspension
    /// points are numbered in the order of their blocks.
    ///
    /// # Panics
    ///
    /// Panics if the function does not suspend at the suspension point.
    #[must_use]
    pub fn tag(self, layout: &FrameLayout) -> Constant {
        let tag = match self {
            Self::NotStarted => 0,
            Self::Completed => 1,
            Self::Suspended(point) => {
                assert!(layout.suspends_at(point), "only a live suspension point has a state");
                let earlier = layout.suspension_points().filter(|other| *other < point).count();
                earlier + Self::NON_SUSPENDED_STATES
            }
        };

        Self::tag_type(layout).constant(u64::try_from(tag).expect("a tag should fit in u64"))
    }
}

/// The type of every value that the frame of one concrete coroutine holds,
/// which is what [`FrameLayout`] leaves out because it does not lower types.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct FrameFieldTypes(BTreeMap<Savable, Interned<MonoType>>);

impl FrameFieldTypes {
    /// The type of `savable`.
    ///
    /// # Panics
    ///
    /// Panics if the frame does not hold `savable`.
    #[must_use]
    pub fn get(&self, savable: Savable) -> &Interned<MonoType> {
        self.0.get(&savable).expect("a value of the frame layout should have a type")
    }

    /// Iterates over the type of every value the frame holds.
    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Interned<MonoType>> { self.0.values() }
}

/// Derives the type of every value that the frame of a coroutine holds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<FrameFieldTypes>)]
#[extend(name = build_frame_field_types, by_val)]
pub struct BuildFrameFieldTypes {
    /// The coroutine whose frame is requested.
    pub instance: MonoCoroutineInstance,
}

#[executor(config = Config)]
async fn build_frame_field_types_executor(
    key: &BuildFrameFieldTypes,
    engine: &TrackedEngine,
) -> Interned<FrameFieldTypes> {
    let solver = Solver::without_givens(engine.clone()).await;
    let body = key.instance.body();
    let def_id = body.def_id();
    let source = engine.get_ir(def_id).await;
    let function_id = body.ir_function_id();
    let function = source.get_function(function_id);
    let substitution = body.substitution();
    let layout = engine.build_frame_layout(key.instance.clone()).await;

    // Lower the type of every value as the suspended function spells it.
    let mut types = BTreeMap::new();
    for (savable, _) in layout.placements() {
        let ty =
            savable_type(&solver, substitution, &source, function, def_id, function_id, savable)
                .await;
        types.insert(savable, lower_type(&solver, &ty, substitution).await);
    }

    engine.intern(FrameFieldTypes(types))
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_FRAME_FIELD_TYPES_EXECUTOR: Registration<Config> =
    Registration::new::<BuildFrameFieldTypes, BuildFrameFieldTypesExecutor>();

/// The type of a value that a frame holds, as the suspended function spells it.
async fn savable_type(
    solver: &Solver,
    substitution: &Subst,
    source: &IRFunctionMap,
    function: &IRFunction,
    def_id: GlobalSymbolID,
    function_id: IRFunctionID,
    savable: Savable,
) -> Interned<Ty> {
    let engine = solver.engine();
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
        Savable::Awaitee(point) => awaitee_type(solver, substitution, function, point).await,
    }
}

/// The type of what the function holds while it is suspended at `point`.
async fn awaitee_type(
    solver: &Solver,
    substitution: &Subst,
    function: &IRFunction,
    point: SuspensionPointID,
) -> Interned<Ty> {
    let engine = solver.engine();
    let expression = function
        .block_terminator(point.block_id())
        .and_then(Terminator::suspending_expr)
        .expect("an awaitee is held at a suspension point");

    match function.get_expression_as_waitee(expression) {
        // The coroutine a continuation resumes is already somewhere else, as
        // for a `handle`. It is the first argument of `Def.call`.
        Waitee::Call(call) if calls_continuation(solver, substitution, call).await => {
            function.get_expression(call.arguments()[0]).ty().clone()
        }
        // The callee runs in a frame stored in the frame of its caller.
        Waitee::Call(call) => {
            call.coroutine_ty(engine).await.expect("a call of an `extern def` does not suspend")
        }
        // The coroutine a `handle` resumes is already somewhere else, behind
        // the continuation.
        Waitee::Handle(handle) => function.get_expression(handle.continuation()).ty().clone(),
        Waitee::RunWith(_) => {
            panic!("forwarding the operations of a `run ... with` is not lowered to MonoIR yet")
        }
        Waitee::Perform => panic!("a `perform` holds nothing while it is suspended"),
    }
}

/// Whether `call` dispatches through the `Def` dictionary of a continuation.
async fn calls_continuation(solver: &Solver, substitution: &Subst, call: &Call) -> bool {
    let Some(dictionary) = call.target().dispatch_instance() else {
        return false;
    };
    let dictionary = dictionary.apply_subst_or_clone(substitution, solver.engine());
    let dictionary = solver.normalize(&dictionary).await;

    matches!(
        &*dictionary,
        Ty::Application(application)
            if matches!(application.view(), ApplicationView::ContinuationDefInstance(_))
    )
}

/// One operation of a concrete effect row.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct RowOperation {
    key: RowOperationKey,
    arguments: Interned<MonoType>,
    resume_type: Interned<MonoType>,
    discriminant_tag: usize,
}

pub type RowOperationID = ID<RowOperation>;

impl RowOperation {
    /// What identifies the operation in any row that has it, unlike its
    /// [`RowOperationID`], which only means something in this row.
    #[must_use]
    pub const fn key(&self) -> &RowOperationKey { &self.key }

    /// The tuple of the operation's arguments, which is what a coroutine
    /// yields when it performs the operation.
    #[must_use]
    pub const fn arguments(&self) -> &Interned<MonoType> { &self.arguments }

    /// The type of the operation's result, which a coroutine that performed it
    /// is resumed with.
    #[must_use]
    pub const fn resume_type(&self) -> &Interned<MonoType> { &self.resume_type }
}

/// A key for identifying a row operation
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct RowOperationKey {
    operation_id: GlobalSymbolID,
    mono_effect_instance: MonoEffectInstance,
}

impl RowOperationKey {
    #[must_use]
    pub const fn new(operation_id: GlobalSymbolID, effect: MonoEffectInstance) -> Self {
        Self { operation_id, mono_effect_instance: effect }
    }

    #[must_use]
    pub const fn operation_id(&self) -> GlobalSymbolID { self.operation_id }

    /// The effect the operation belongs to.
    #[must_use]
    pub const fn effect(&self) -> &MonoEffectInstance { &self.mono_effect_instance }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct ResumeVariant {
    r#type: Interned<MonoType>,
}

pub type ResumeVariantID = ID<ResumeVariant>;

impl ResumeVariant {
    /// The type of the value this variant holds.
    #[must_use]
    pub const fn ty(&self) -> &Interned<MonoType> { &self.r#type }
}

/// The members of the `coroutine_resume_t` and `coroutine_yield_t` of one
/// concrete effect row.
///
/// For more information, see [`CoroutineYield`] and [`CoroutineProgress`].
///
/// [`CoroutineYield`]: crate::ty::CoroutineYield
/// [`CoroutineProgress`]: crate::ty::CoroutineProgress
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct EffectRowLayout {
    // Ordered, since code generation iterates over the operations.
    row_operations: OrderedArena<RowOperation>,
    row_operation_ids_by_key: FxHashMap<RowOperationKey, RowOperationID>,
    resume_variants: Vec<ResumeVariant>,
}

impl EffectRowLayout {
    #[must_use]
    pub fn get_resume_variant_id(&self, resume_type: &Interned<MonoType>) -> ResumeVariantID {
        self.resume_variants
            .iter()
            .enumerate()
            .find(|(_, variant)| &variant.r#type == resume_type)
            .map(|(i, _)| ResumeVariantID::new(i as u64))
            .expect("should exist")
    }

    #[must_use]
    pub fn get_row_operation_id(&self, key: &RowOperationKey) -> RowOperationID {
        self.row_operation_ids_by_key.get(key).copied().expect("should exist")
    }

    /// Iterates over the distinct types a coroutine of this row can be resumed
    /// with. Unit is always first, since a coroutine starts with it.
    pub fn resume_variants(&self) -> impl Iterator<Item = (ResumeVariantID, &ResumeVariant)> {
        self.resume_variants
            .iter()
            .enumerate()
            .map(|(i, variant)| (ResumeVariantID::new(i as u64), variant))
    }

    /// Iterates over the operations of the row, in the order of their tags.
    #[must_use]
    pub fn row_operations(&self) -> impl ExactSizeIterator<Item = (RowOperationID, &RowOperation)> {
        self.row_operations.iter()
    }

    /// Iterates over the IDs of the operations, in the order of their tags.
    #[must_use]
    pub fn row_operation_ids(&self) -> impl ExactSizeIterator<Item = RowOperationID> + use<'_> {
        self.row_operations.ids()
    }

    /// The type of the tag of a `coroutine_yield_t`, which tells the operations
    /// of the row apart.
    #[must_use]
    pub fn yield_tag_type(&self) -> TagType {
        TagType::for_variant_count(self.row_operations.len())
    }

    /// The tag of `coroutine_yield_t` that tells the given operation.
    #[must_use]
    pub fn yield_tag(&self, operation: RowOperationID) -> Constant {
        let tag = self[operation].discriminant_tag;
        self.yield_tag_type().constant(u64::try_from(tag).expect("a tag should fit in u64"))
    }
}

impl Index<RowOperationID> for EffectRowLayout {
    type Output = RowOperation;

    fn index(&self, index: RowOperationID) -> &Self::Output { &self.row_operations[index] }
}

impl Index<ResumeVariantID> for EffectRowLayout {
    type Output = ResumeVariant;

    fn index(&self, index: ResumeVariantID) -> &Self::Output {
        &self.resume_variants[usize::try_from(index.index()).expect("index too large")]
    }
}

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
    let mut row_operations = OrderedArena::new();
    let mut row_operation_ids_by_key = FxHashMap::default();

    let mut resume_variants = vec![ResumeVariant { r#type: MonoType::new_unit(engine) }]; // starting a coroutine is always resumed with unit type

    for effect in key.row.effects() {
        let layout = engine.build_handler_layout(effect.clone()).await;

        for operation in layout.operations() {
            // A yield has no use for the environment of the handler ABI, so
            // it carries the source arguments only.
            let signature = operation.signature();
            let arguments = engine.intern_unsized(signature.parameter_types().to_vec());
            let ReturnType::Value(resume_type) = signature.return_type() else {
                panic!("an operation handler returns a value")
            };

            let tag = row_operations.len();
            let operation_key = RowOperationKey::new(operation.operation_id(), effect.clone());

            let row_operation_id = row_operations.insert(RowOperation {
                key: operation_key.clone(),
                arguments: engine
                    .intern(MonoType::Aggregate(AggregateType::Tuple(Tuple::new(arguments)))),
                resume_type: resume_type.clone(),
                discriminant_tag: tag,
            });
            row_operation_ids_by_key.insert(operation_key, row_operation_id);

            if !resume_variants.iter().any(|t| &t.r#type == resume_type) {
                resume_variants.push(ResumeVariant { r#type: resume_type.clone() });
            }
        }
    }

    engine.intern(EffectRowLayout { row_operations, row_operation_ids_by_key, resume_variants })
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_EFFECT_ROW_LAYOUT_EXECUTOR: Registration<Config> =
    Registration::new::<BuildEffectRowLayout, BuildEffectRowLayoutExecutor>();

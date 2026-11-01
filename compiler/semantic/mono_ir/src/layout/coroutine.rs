use std::ops::Index;

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_arena::{Arena, ID};
use rayc_coroutine_frame_layout::frame_layout::FrameLayout;
use rayc_hash::FxHashMap;
use rayc_ir::get_ir;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::application::ConcreteCoroutineView;

use crate::{
    MonoCoroutineInstance, MonoEffectInstance,
    layout::build_handler_layout as _,
    ty::{AggregateType, EffectRow, MonoType, ReturnType, Tuple},
};

/// Builds the [`FrameLayout`] of a concrete coroutine instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(Interned<FrameLayout>)]
pub struct BuildFrameLayout {
    pub key: MonoCoroutineInstance,
}

#[executor(config = Config)]
async fn build_frame_layout_executor(
    key: &BuildFrameLayout,
    engine: &TrackedEngine,
) -> Interned<FrameLayout> {
    let mut solver = Solver::without_givens(engine.clone()).await;
    let def_id = key.key.view().owner_def_id();
    let ir = engine.get_ir(def_id).await;

    let local_ir_function_id = match key.key.view() {
        ConcreteCoroutineView::Def(_) => ir.root_id(),
        ConcreteCoroutineView::Closure(closure_coroutine_view) => {
            ir.get_function_id_of_closure(closure_coroutine_view.closure_view().local_closure_id())
        }
    };

    let frame_layout = FrameLayout::compute(
        engine.clone(),
        &mut solver,
        def_id,
        local_ir_function_id,
        key.key.view().create_subst(engine).await,
    )
    .await;

    engine.intern(frame_layout)
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_FRAME_LAYOUT_EXECUTOR: Registration<Config> =
    Registration::new::<BuildFrameLayout, BuildFrameLayoutExecutor>();

/// One operation of a concrete effect row.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct RowOperation {
    arguments: Interned<MonoType>,
    resume_type: Interned<MonoType>,
    discriminant_tag: usize,
}

pub type RowOperationID = ID<RowOperation>;

impl RowOperation {
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

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct ResumeVariant {
    r#type: Interned<MonoType>,
}

pub type ResumeVariantID = ID<ResumeVariant>;

/// The members of the `coroutine_resume_t` and `coroutine_yield_t` of one
/// concrete effect row.
///
/// For more information, see [`CoroutineYield`] and [`CoroutineProgress`].
///
/// [`CoroutineYield`]: crate::ty::CoroutineYield   
/// [`CoroutineProgress`]: crate::ty::CoroutineProgress
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct EffectRowLayout {
    // Maps from an effect operation ID to its row operation.
    row_operations: Arena<RowOperation>,
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
    let mut row_operations = Arena::new();
    let mut row_operation_ids_by_key = FxHashMap::default();

    let mut resume_variants = vec![ResumeVariant { r#type: MonoType::new_unit(engine) }]; // starting a coroutine is always resumed with unit type

    for effect in key.row.effects() {
        let layout = engine.build_handler_layout(effect.clone()).await;

        for operation in layout.operations() {
            // The handler ABI starts with the environment, which a yield has
            // no use for.
            let signature = operation.signature();
            let arguments = engine.intern_unsized(signature.parameter_types()[1..].to_vec());
            let ReturnType::Value(resume_type) = signature.return_type() else {
                panic!("an operation handler returns a value")
            };

            let tag = row_operations.len();

            let row_operation_id = row_operations.insert(RowOperation {
                arguments: engine
                    .intern(MonoType::Aggregate(AggregateType::Tuple(Tuple::new(arguments)))),
                resume_type: resume_type.clone(),
                discriminant_tag: tag,
            });
            row_operation_ids_by_key.insert(
                RowOperationKey {
                    operation_id: operation.operation_id(),
                    mono_effect_instance: effect.clone(),
                },
                row_operation_id,
            );

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

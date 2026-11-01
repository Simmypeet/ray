//! How a function performs its effects, and the state that takes.

use qbice::storage::intern::Interned;
use rayc_coroutine_frame_layout::{
    frame_layout::{FrameLayout, Placement, SuspensionLocalPlacement},
    savable::Savable,
    suspension_point::SuspensionPointID,
};
use rayc_hash::FxHashMap;
use rayc_ir::{cfg::BlockID as IRBlockID, ir_function::IRFunctionID};
use rayc_mono_ir::{
    MonoCoroutineInstance, MonoEffectInstance,
    cfg::BlockID,
    layout::coroutine::{EffectRowLayout, FrameFieldTypes, FrameState},
    operand::{Constant, Operand},
    place::Place,
    signature::CoroutineSignature,
    ty::MonoType,
};
use rayc_qbice::TrackedEngine;

use super::emitter::Emitter;
use crate::context::Context;

/// Why a function has no handlers to pass on.
pub(crate) const NO_HANDLERS: &str = "a coroutine yields its operations rather than call a \
                                      handler, so nothing in one should need a handler";

/// The handler of each effect of a function, as places of that function.
#[derive(Default)]
pub(crate) struct EffectHandlers(FxHashMap<MonoEffectInstance, Place>);

impl EffectHandlers {
    fn insert(&mut self, effect: MonoEffectInstance, place: Place) { self.0.insert(effect, place); }

    fn get_place_of_effect_instance(&self, effect: &MonoEffectInstance) -> Place {
        self.0
            .get(effect)
            .unwrap_or_else(|| panic!("effect handler {effect:?} is not available"))
            .clone()
    }

    pub(crate) fn get_effect_instance_operand(&self, effect: &MonoEffectInstance) -> Operand {
        Operand::Copy(self.get_place_of_effect_instance(effect))
    }
}

/// What lowering a `coroutine_resume` function needs beyond a callback-style
/// function.
pub(crate) struct CoroutineForm {
    /// The frame, behind the `frame` parameter.
    frame_place: Place,
    /// The `args` parameter.
    resume_args: Place,
    /// The local every `perform` and `return` builds its result in.
    progress: Place,
    frame_layout: Interned<FrameLayout>,
    frame_field_types: Interned<FrameFieldTypes>,
    signature: Interned<CoroutineSignature>,
    row_layout: Interned<EffectRowLayout>,
    /// The block that continues each suspension point, which lowering the
    /// point fills.
    resume_blocks: FxHashMap<SuspensionPointID, BlockID>,
}

impl CoroutineForm {
    /// Reads the layouts of `instance`, and creates the locals and blocks that
    /// `coroutine_resume` always has.
    async fn new(
        context: &Context,
        source_id: IRFunctionID,
        emit: &mut Emitter<'_>,
        instance: &MonoCoroutineInstance,
    ) -> Self {
        let resolver = context.resolver();
        let signature = resolver.coroutine_signature(instance).await;
        let row_layout = resolver.effect_row_layout(signature.row()).await;
        let frame_layout = resolver.frame_layout(instance).await;
        let frame_field_types = resolver.frame_field_types(instance).await;

        let mut parameters = emit.parameter_ids();
        let frame_param = parameters.next().expect("should exist");
        let resume_param = parameters.next().expect("should exist");
        drop(parameters);

        let frame_place = emit.project(&emit.place(frame_param)).dereference();
        let resume_args = emit.place(resume_param);
        let progress = emit.new_progress_temporary_place(&signature);

        // Only a suspension point that direct control flow reaches is lowered.
        let mut resume_blocks = FxHashMap::default();
        for block in context.source_function(source_id).direct_reachables().blocks() {
            let point = SuspensionPointID::new(block);
            if frame_layout.suspends_at(point) {
                resume_blocks.insert(point, emit.create_block());
            }
        }

        Self {
            frame_place,
            resume_args,
            progress,
            frame_layout,
            frame_field_types,
            signature,
            row_layout,
            resume_blocks,
        }
    }

    pub(crate) const fn frame_place(&self) -> &Place { &self.frame_place }

    pub(crate) const fn resume_args_place(&self) -> &Place { &self.resume_args }

    pub(crate) const fn progress_place(&self) -> &Place { &self.progress }

    /// The tag that the frame holds in the given state.
    pub(crate) fn frame_state_tag(&self, state: FrameState) -> Constant {
        state.tag(&self.frame_layout)
    }

    pub(crate) const fn signature(&self) -> &Interned<CoroutineSignature> { &self.signature }

    pub(crate) const fn row_layout(&self) -> &Interned<EffectRowLayout> { &self.row_layout }

    /// The block that gets control after dispatching the state of the coroutine
    /// frame. This might not necessarily be the same source block
    pub(crate) fn dispatched_resume_block(&self, point: SuspensionPointID) -> BlockID {
        *self.resume_blocks.get(&point).expect("a lowered suspension point has a resume block")
    }

    /// The state of each suspension point with the block that continues it, in
    /// the order of the states.
    pub(crate) fn resume_points(&self) -> impl Iterator<Item = (Constant, BlockID)> {
        let mut points = self
            .resume_blocks
            .iter()
            .map(|(point, block)| (self.frame_state_tag(FrameState::Suspended(*point)), *block))
            .collect::<Vec<_>>();
        points.sort_unstable();
        points.into_iter()
    }

    /// The place of `savable` in the frame, if the frame holds it.
    pub(super) fn frame_place_of(&self, savable: Savable, engine: &TrackedEngine) -> Option<Place> {
        self.frame_layout
            .placement(savable)
            .map(|_| self.frame_place.project(engine).coroutine_frame_savable(savable))
    }

    /// Whether `savable` is in the start variant of the frame, which every
    /// suspension variant overlaps. Nothing holds such a value across a
    /// suspension point, or it would have been promoted to the prefix, so it
    /// can live in a local of the first run of `coroutine_resume`.
    pub(super) fn is_start_field(&self, savable: Savable) -> bool {
        self.frame_layout.placement(savable)
            == Some(Placement::SuspensionLocal(SuspensionLocalPlacement::Start))
    }

    /// The type of `savable`, which the frame holds.
    pub(super) fn field_type(&self, savable: Savable) -> &Interned<MonoType> {
        self.frame_field_types.get(savable)
    }

    /// The place of what the function holds while it is suspended at the
    /// suspension point that terminates `block`.
    pub(crate) fn awaitee_place(&self, block: IRBlockID, engine: &TrackedEngine) -> Place {
        self.frame_place_of(Savable::Awaitee(SuspensionPointID::new(block)), engine)
            .expect("a suspension point that waits for a coroutine holds it in the frame")
    }

    /// The suspension point that terminates `block`, if the function still
    /// suspends there.
    fn suspension_point(&self, block: IRBlockID) -> Option<SuspensionPointID> {
        let point = SuspensionPointID::new(block);
        self.frame_layout.suspends_at(point).then_some(point)
    }
}

/// How the function being lowered performs its effects.
pub(crate) enum LoweringStyle {
    /// An operation is a call of a callback in a handler record, which returns
    /// to the function. The function holds its values in locals.
    Callback(EffectHandlers),

    /// An operation returns from `coroutine_resume` to whatever resumed it. The
    /// function holds what it needs across a suspension in its frame.
    Coroutine(CoroutineForm),
}

impl LoweringStyle {
    /// The style of a function that performs its effects through handlers: the
    /// parameters after its source parameters hold them, or else the fields
    /// after the captures of `environment`.
    pub(super) fn new_callback(
        context: &Context,
        source_id: IRFunctionID,
        emit: &Emitter<'_>,
        environment: Option<&Place>,
    ) -> Self {
        let mut handlers = EffectHandlers::default();

        let abi = context.function_abi(source_id);

        if !abi.captures_effect_handlers() {
            // this means the effect handlers are passed as parameters after the
            // source parameters
            let parameters = emit.parameter_ids();
            let first = parameters.len() - abi.effects().len();

            for (effect, parameter) in abi.effects().cloned().zip(parameters.skip(first)) {
                handlers.insert(effect, emit.place(parameter));
            }
        }

        // Initialize effect handlers from the same environment ABI used by its
        // producer.
        if let Some(environment) = environment {
            let environment_abi = context.function_environment_abi(source_id);
            for (field, effect) in environment_abi.captured_effects_with_field_offset() {
                handlers.insert(effect.clone(), emit.project(environment).environment_field(field));
            }
        }

        Self::Callback(handlers)
    }

    /// The style of a function lowered into the `coroutine_resume` of
    /// `instance`.
    pub(super) async fn new_coroutine(
        context: &Context,
        source_id: IRFunctionID,
        emit: &mut Emitter<'_>,
        instance: &MonoCoroutineInstance,
    ) -> Self {
        Self::Coroutine(CoroutineForm::new(context, source_id, emit, instance).await)
    }

    pub(crate) const fn is_coroutine(&self) -> bool {
        match self {
            Self::Callback(_) => false,
            Self::Coroutine(_) => true,
        }
    }

    pub(crate) const fn handlers(&self) -> Option<&EffectHandlers> {
        match self {
            Self::Callback(handlers) => Some(handlers),
            Self::Coroutine(_) => None,
        }
    }

    pub(crate) fn handler_place(&self, effect: &MonoEffectInstance) -> Place {
        self.handlers().expect(NO_HANDLERS).get_place_of_effect_instance(effect)
    }

    pub(crate) fn get_effect_instance_handler_operand(
        &self,
        effect: &MonoEffectInstance,
    ) -> Operand {
        self.handlers().expect(NO_HANDLERS).get_effect_instance_operand(effect)
    }

    /// The form of a `coroutine_resume` function.
    pub(crate) const fn coroutine_form(&self) -> &CoroutineForm {
        match self {
            Self::Coroutine(form) => form,
            Self::Callback(_) => panic!("this function should be a `coroutine_resume`"),
        }
    }

    /// The suspension point that terminates `block`, if this is a
    /// `coroutine_resume` function that still suspends there.
    pub(crate) fn suspension_point(&self, block: IRBlockID) -> Option<SuspensionPointID> {
        match self {
            Self::Callback(_) => None,
            Self::Coroutine(form) => form.suspension_point(block),
        }
    }
}

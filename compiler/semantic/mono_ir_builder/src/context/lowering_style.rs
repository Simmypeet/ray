//! How a function performs its effects, and the state that takes.

use qbice::storage::intern::Interned;
use rayc_coroutine_frame_layout::{
    frame_layout::{FrameLayout, Placement, SuspensionLocalPlacement},
    savable::Savable,
    suspension_point::SuspensionPointID,
};
use rayc_hash::FxHashMap;
use rayc_ir::{cfg::BlockID as IRBlockID, ir_function::IRFunction};
use rayc_mono_ir::{
    MonoCoroutineInstance, MonoEffectInstance,
    cfg::BlockID,
    function::LocalID,
    layout::{
        EnvironmentLayout,
        coroutine::{EffectRowLayout, FrameFieldTypes, FrameState},
    },
    operand::{Constant, Operand},
    place::Place,
    signature::CoroutineSignature,
    ty::MonoType,
};
use rayc_qbice::TrackedEngine;

use crate::emitter::Emitter;

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

/// The frame of a coroutine, as a function lowered from the function it
/// suspends sees it: where each value the function holds lives in the frame,
/// and what the tag of the frame says.
///
/// Unlike [`CoroutineForm`], it needs nothing that only `coroutine_resume`
/// has, such as `args` or the resume entries.
pub(crate) struct Frame {
    /// The frame, behind the pointer parameter of the function.
    place: Place,
    layout: Interned<FrameLayout>,
    field_types: Interned<FrameFieldTypes>,
}

impl Frame {
    /// Reads the layouts of the frame of `instance`, which the parameter
    /// `pointer` points to.
    async fn new(emit: &Emitter<'_>, pointer: LocalID, instance: &MonoCoroutineInstance) -> Self {
        let resolver = emit.resolver();
        let layout = resolver.frame_layout(instance).await;
        let field_types = resolver.frame_field_types(instance).await;
        let place = emit.project(&emit.local_place(pointer)).dereference();

        // only a child frame is created and ended where its box can be
        if let Some(savable) = field_types.boxed().find(|savable| !savable.is_child_frame()) {
            unimplemented!(
                "the frame of {instance:?} boxes {savable:?} to break a recursion, but only the \
                 frame of a callee or of a `run ... with` can be boxed yet"
            );
        }

        Self { place, layout, field_types }
    }

    /// The place of the tag of the frame, which tells its state.
    ///
    /// ```c
    /// frame->state
    /// ```
    pub(crate) fn state_place(&self, engine: &TrackedEngine) -> Place {
        self.place.project(engine).coroutine_frame_state()
    }

    /// The tag that the frame holds in the given state.
    pub(crate) fn state_tag(&self, state: FrameState) -> Constant { state.tag(&self.layout) }

    /// The place of `savable` in the frame, if the frame holds it: its member,
    /// or where its member points if it is boxed.
    ///
    /// ```c
    /// frame->savable              // held by value
    /// *frame->savable             // boxed
    /// ```
    pub(super) fn place_of(&self, savable: Savable, engine: &TrackedEngine) -> Option<Place> {
        let member = self.member_place(savable, engine)?;

        Some(if self.field_types.is_boxed(savable) {
            member.project(engine).dereference()
        } else {
            member
        })
    }

    /// The member of `savable` in the frame, if the frame holds it.
    fn member_place(&self, savable: Savable, engine: &TrackedEngine) -> Option<Place> {
        self.layout
            .placement(savable)
            .map(|_| self.place.project(engine).coroutine_frame_savable(savable))
    }

    /// The member that points to the value of `savable` on the heap, if the
    /// frame boxes it. The value is allocated and freed through it.
    ///
    /// ```c
    /// frame->savable
    /// ```
    pub(crate) fn box_pointer_of(&self, savable: Savable, engine: &TrackedEngine) -> Option<Place> {
        if !self.field_types.is_boxed(savable) {
            return None;
        }

        self.member_place(savable, engine)
    }

    /// Whether `savable` is in the start variant of the frame, which every
    /// suspension variant overlaps. Nothing holds such a value across a
    /// suspension point, or it would have been promoted to the prefix, so it
    /// can live in a local of the first run of `coroutine_resume`.
    pub(super) fn is_start_field(&self, savable: Savable) -> bool {
        self.layout.placement(savable)
            == Some(Placement::SuspensionLocal(SuspensionLocalPlacement::Start))
    }

    /// The type of `savable`, which the frame holds, by value or in a box.
    pub(super) fn field_type(&self, savable: Savable) -> &Interned<MonoType> {
        self.field_types.value_type(savable)
    }

    /// Whether the function owns a coroutine that it waits for while it is
    /// suspended at `point`: the frame of a callee, or the frames of the body
    /// and the clauses of a `run ... with`.
    pub(crate) fn holds_child_frame(&self, point: SuspensionPointID) -> bool {
        self.layout.placement(Savable::ChildFrame(point)).is_some()
    }

    /// The place of the coroutine that the function waits for while it is
    /// suspended at `point`.
    pub(crate) fn child_frame_place(
        &self,
        point: SuspensionPointID,
        engine: &TrackedEngine,
    ) -> Place {
        self.place_of(Savable::ChildFrame(point), engine)
            .expect("a suspension point that waits for a coroutine holds it in the frame")
    }

    /// The suspension point that terminates `block`, if the function still
    /// suspends there.
    fn suspension_point(&self, block: IRBlockID) -> Option<SuspensionPointID> {
        let point = SuspensionPointID::new(block);
        self.layout.suspends_at(point).then_some(point)
    }

    /// Iterates over the suspension points where the function still suspends,
    /// in no particular order.
    pub(crate) fn suspension_points(&self) -> impl Iterator<Item = SuspensionPointID> + '_ {
        self.layout.suspension_points()
    }

    /// Iterates over the values that the frame holds outside its start
    /// variant: the only ones still there once the function has suspended,
    /// since the first suspension overwrites the start variant.
    pub(super) fn suspended_savables(&self) -> impl Iterator<Item = Savable> + '_ {
        self.layout
            .placements()
            .filter(|(savable, _)| !self.is_start_field(*savable))
            .map(|(savable, _)| savable)
    }
}

/// The block that the dispatch of `coroutine_resume` enters to continue one
/// suspension point, and whether lowering the point has filled it yet.
struct ResumeEntry {
    block: BlockID,
}

/// What lowering a `coroutine_resume` function needs beyond a callback-style
/// function.
pub(crate) struct CoroutineForm {
    /// The frame, behind the `frame` parameter.
    frame: Frame,
    /// The `args` parameter.
    resume_args: Place,
    signature: Interned<CoroutineSignature>,
    row_layout: Interned<EffectRowLayout>,
    /// The resume entry of each suspension point, which lowering the point
    /// fills.
    resume_entries: FxHashMap<SuspensionPointID, ResumeEntry>,
}

impl CoroutineForm {
    /// Reads the layouts of `instance`, and creates the resume entry of each
    /// suspension point of `function`, the function it suspends.
    async fn new(
        function: &IRFunction,
        emit: &mut Emitter<'_>,
        instance: &MonoCoroutineInstance,
    ) -> Self {
        let resolver = emit.resolver();
        let signature = resolver.coroutine_signature(instance).await;
        let row_layout = resolver.effect_row_layout(signature.row()).await;

        let mut parameters = emit.parameter_ids();
        let frame_param = parameters.next().expect("should exist");
        let resume_param = parameters.next().expect("should exist");
        drop(parameters);

        let frame = Frame::new(emit, frame_param, instance).await;
        let resume_args = emit.local_place(resume_param);

        // Only a suspension point that direct control flow reaches is lowered.
        let mut resume_entries = FxHashMap::default();
        for block in function.direct_reachables().blocks() {
            if let Some(point) = frame.suspension_point(block) {
                let entry = ResumeEntry { block: emit.create_block() };
                resume_entries.insert(point, entry);
            }
        }

        Self { frame, resume_args, signature, row_layout, resume_entries }
    }

    pub(crate) const fn resume_args_place(&self) -> &Place { &self.resume_args }

    pub(crate) const fn signature(&self) -> &Interned<CoroutineSignature> { &self.signature }

    pub(crate) const fn row_layout(&self) -> &Interned<EffectRowLayout> { &self.row_layout }

    /// Claims the resume entry of `point` for the lowering of that point to
    /// fill. Each point fills its entry exactly once.
    pub(crate) fn claim_resume_entry(&self, point: SuspensionPointID) -> BlockID {
        self.resume_entries
            .get(&point)
            .expect("a lowered suspension point has a resume entry")
            .block
    }

    /// The state of each suspension point with its resume entry.
    ///
    /// The dispatch reads them once every block is lowered, so by then every
    /// entry must have been filled.
    pub(crate) fn resume_points(&self) -> impl Iterator<Item = (Constant, BlockID)> {
        self.resume_entries.iter().map(|(point, entry)| {
            (self.frame.state_tag(FrameState::Suspended(*point)), entry.block)
        })
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

    /// The `coroutine_drop` of the function: it performs nothing, and only
    /// drops what the frame holds where the function is suspended.
    CoroutineDrop(Frame),
}

impl LoweringStyle {
    /// The style of a function that performs its effects through handlers:
    /// its handler-record parameters hold them, or else the handler records of
    /// `environment`, the layout and place of its environment.
    pub(super) fn new_callback(
        emit: &Emitter<'_>,
        environment: Option<(&Interned<EnvironmentLayout>, &Place)>,
    ) -> Self {
        let mut handlers = EffectHandlers::default();

        for (effect, parameter) in emit.handler_record_parameters() {
            handlers.insert(effect.clone(), emit.local_place(parameter));
        }

        // Only an operation handler's environment has handler records.
        if let Some((layout, environment)) = environment {
            for (record_id, record) in layout.handler_records() {
                let place = emit.project(environment).environment_handler_record(record_id);
                handlers.insert(record.effect().clone(), place);
            }
        }

        Self::Callback(handlers)
    }

    /// The style of `function` lowered into the `coroutine_resume` of
    /// `instance`, which suspends it.
    pub(super) async fn new_coroutine(
        function: &IRFunction,
        emit: &mut Emitter<'_>,
        instance: &MonoCoroutineInstance,
    ) -> Self {
        Self::Coroutine(CoroutineForm::new(function, emit, instance).await)
    }

    /// The style of the `coroutine_drop` of `instance`, whose only parameter
    /// points to the frame.
    pub(super) async fn new_coroutine_drop(
        emit: &Emitter<'_>,
        instance: &MonoCoroutineInstance,
    ) -> Self {
        let mut parameters = emit.parameter_ids();
        let frame_param = parameters.next().expect("`coroutine_drop` takes the frame");
        assert!(parameters.next().is_none(), "`coroutine_drop` only takes the frame");

        Self::CoroutineDrop(Frame::new(emit, frame_param, instance).await)
    }

    /// Whether the function is lowered from the function that a coroutine
    /// suspends, into its `coroutine_resume` or its `coroutine_drop`. Such a
    /// function has a frame, and no handlers.
    pub(crate) const fn is_coroutine(&self) -> bool {
        match self {
            Self::Callback(_) => false,
            Self::Coroutine(_) | Self::CoroutineDrop(_) => true,
        }
    }

    pub(crate) const fn handlers(&self) -> Option<&EffectHandlers> {
        match self {
            Self::Callback(handlers) => Some(handlers),
            Self::Coroutine(_) | Self::CoroutineDrop(_) => None,
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
            Self::Callback(_) | Self::CoroutineDrop(_) => {
                panic!("this function should be a `coroutine_resume`")
            }
        }
    }

    /// The frame of the coroutine that the function works on.
    pub(crate) const fn frame(&self) -> &Frame {
        match self {
            Self::Coroutine(form) => &form.frame,
            Self::CoroutineDrop(frame) => frame,
            Self::Callback(_) => panic!("only a function lowered in coroutine style has a frame"),
        }
    }

    /// The suspension point that terminates `block`, if this function has a
    /// frame and still suspends there.
    pub(crate) fn suspension_point(&self, block: IRBlockID) -> Option<SuspensionPointID> {
        match self {
            Self::Callback(_) => None,
            Self::Coroutine(form) => form.frame.suspension_point(block),
            Self::CoroutineDrop(frame) => frame.suspension_point(block),
        }
    }
}

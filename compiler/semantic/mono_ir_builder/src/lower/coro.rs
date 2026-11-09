use rayc_coroutine_frame_layout::savable::Savable;
use rayc_ir::{
    address::Local,
    get_ir,
    ir_closure::{CaptureID, CaptureOperands},
    ir_expr::{IRExprID, coroutine::Coroutine},
};
use rayc_mono_ir::{
    MonoCoroutineInstance,
    layout::coroutine::FrameState,
    operand::Operand,
    place::{FieldIndex, Place},
    rvalue::{AddressOf, Rvalue},
    ty::PointerMutability,
};
use rayc_qbice::TrackedEngine;

use crate::{context::Context, emitter::Emitter};

/// Where the frame of a coroutine that has not started holds its upvars: the
/// arguments of the call that the coroutine suspends.
///
/// Creating a frame and dropping one that has not started both read it, so
/// that they agree on where each argument is.
pub(super) enum Upvars {
    /// A definition is called with its parameters, and the frame holds each
    /// argument in the field of its parameter, in parameter order.
    Definition(Vec<Local>),

    /// `Def.call(closure, arguments)` takes the closure value and a tuple. The
    /// frame holds the captures of the closure in the fields of the captures,
    /// and the elements of the tuple in the fields of the parameters of the
    /// closure's body.
    Closure { captures: Vec<CaptureID>, parameters: Vec<Local> },
}

impl Upvars {
    /// Where the frame of `instance` holds its upvars.
    pub(super) async fn of(instance: &MonoCoroutineInstance, engine: &TrackedEngine) -> Self {
        let body = instance.body();
        let source = engine.get_ir(body.def_id()).await;
        let function_id = body.ir_function_id();
        let function = source.get_function(function_id);
        let parameters = function.parameter_locals(body.def_id(), engine).await;

        if function.is_closure() {
            let captures =
                source.get_captures_of_function(function_id).map(|(capture, _)| capture).collect();
            Self::Closure { captures, parameters }
        } else if function.is_def() {
            Self::Definition(parameters)
        } else {
            panic!("a `coro` must call a closure or a definition, not a {function:?}");
        }
    }
}

impl Emitter<'_> {
    /// Stores the arguments of a `coro` in a frame that has not started.
    ///
    /// A boxed expression is first written here, so its box is allocated
    /// just before. In theory, the expression is never boxed, since the store
    /// right after it ends it before any suspension point.
    ///
    /// ```c
    ///     frame->e = ray_alloc(sizeof(c_frame_t));   // boxed only
    ///     *frame->e = (c_frame_t){ .state = NOT_STARTED, ... };
    /// ```
    pub(super) async fn lower_coro(
        &mut self,
        coroutine: &Coroutine,
        expression_id: IRExprID,
        ctx: &Context<'_>,
    ) {
        self.allocate_frame_box(Savable::Expression(expression_id), ctx);
        let destination = ctx.expression_place(expression_id);
        let instance = ctx.expression_type(expression_id).assert_as_coroutine_frame().clone();
        let arguments =
            coroutine.arguments().iter().map(|argument| ctx.expression_place(*argument));

        self.initialize_coroutine(&destination, &instance, arguments).await;
    }

    /// Creates at `destination` a frame of `instance` that has not started,
    /// from the places of the arguments of its call.
    pub(super) async fn initialize_coroutine(
        &mut self,
        destination: &Place,
        instance: &MonoCoroutineInstance,
        mut arguments: impl ExactSizeIterator<Item = Place>,
    ) {
        self.store_not_started_state(destination, instance).await;

        // What the frame stores depends on the function it suspends.
        match Upvars::of(instance, self.engine()).await {
            // A definition stores its arguments as they are.
            Upvars::Definition(parameters) => {
                assert_eq!(parameters.len(), arguments.len(), "a `coro` passes every parameter");
                for (local, argument) in parameters.into_iter().zip(arguments) {
                    self.store_upvar(destination, local, argument);
                }
            }

            // The closure value and the tuple are split into the captures and
            // the parameters of the closure's body.
            Upvars::Closure { captures, parameters } => {
                let environment = arguments.next().expect("closure always has an environment");
                let tuple = arguments.next().expect("closure always has an argument tuple");

                assert!(
                    arguments.next().is_none(),
                    "closure always has two arguments: environment and argument tuple"
                );

                for capture in captures {
                    let value = self.project(&environment).environment_capture(capture);
                    self.store_upvar(destination, Local::Capture(capture), value);
                }

                for (index, local) in parameters.into_iter().enumerate() {
                    let value = self.project(&tuple).tuple_field(FieldIndex::from_usize(index));
                    self.store_upvar(destination, local, value);
                }
            }
        }
    }

    /// Creates at `destination` a frame of `instance`, the handled body of a
    /// `run ... with`, that has not started. The body owns its captures,
    /// whose operands are `captures`.
    ///
    /// ```c
    /// frame = { .state = NOT_STARTED, .capture0 = ..., ... };
    /// ```
    pub(super) async fn initialize_run_with_body(
        &mut self,
        destination: &Place,
        instance: &MonoCoroutineInstance,
        captures: &CaptureOperands,
        ctx: &Context<'_>,
    ) {
        self.store_not_started_state(destination, instance).await;

        for capture in captures.captures() {
            let value = ctx.expression_place(captures.get_operand_of(capture));
            self.store_upvar(destination, Local::Capture(capture), value);
        }
    }

    /// Creates at `destination` a frame of `instance`, the clause of an
    /// operation, that has not started. It points to `environment`, which
    /// the clauses share, and its parameters are the elements of the tuple
    /// `arguments`.
    ///
    /// ```c
    /// frame = { .state = NOT_STARTED, .environment = &environment, .param0 = arguments.elem0, ... };
    /// ```
    pub(super) async fn initialize_run_with_clause(
        &mut self,
        destination: &Place,
        instance: &MonoCoroutineInstance,
        environment: Place,
        arguments: &Place,
    ) {
        let engine = self.engine();
        self.store_not_started_state(destination, instance).await;

        let pointer = self.project(destination).coroutine_frame_savable(Savable::Environment);
        self.assign(
            pointer,
            Rvalue::AddressOf(AddressOf::new(environment, PointerMutability::Mut)),
        );

        let body = instance.body();
        let source = engine.get_ir(body.def_id()).await;
        let function = source.get_function(body.ir_function_id());
        let parameters = function.parameter_locals(body.def_id(), engine).await;
        for (index, local) in parameters.into_iter().enumerate() {
            let value = self.project(arguments).tuple_field(FieldIndex::from_usize(index));
            self.store_upvar(destination, local, value);
        }
    }

    /// Stores in the frame at `destination`, of `instance`, that it has not
    /// started.
    async fn store_not_started_state(
        &mut self,
        destination: &Place,
        instance: &MonoCoroutineInstance,
    ) {
        let layout = self.resolver().frame_layout(instance).await;
        self.store_constant(
            self.project(destination).coroutine_frame_state(),
            FrameState::NotStarted.tag(&layout),
        );
    }

    /// Stores what `local` starts with in the frame at `frame`.
    fn store_upvar(&mut self, frame: &Place, local: Local, value: Place) {
        let field = self.upvar_field(frame, local);
        self.store(field, Operand::Copy(value));
    }

    /// The field of the frame at `frame` that holds `local`, a parameter or a
    /// capture, while the frame has not started.
    pub(super) fn upvar_field(&self, frame: &Place, local: Local) -> Place {
        self.project(frame).coroutine_frame_savable(Savable::Local(local))
    }
}

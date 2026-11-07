use rayc_coroutine_frame_layout::savable::Savable;
use rayc_ir::{
    address::Local,
    get_ir,
    ir_expr::{IRExprID, coroutine::Coroutine},
};
use rayc_mono_ir::{
    MonoCoroutineInstance,
    layout::coroutine::FrameState,
    operand::Operand,
    place::{FieldIndex, Place},
};

use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    /// Stores the arguments of a `coro` in a frame that has not started.
    pub(super) async fn lower_coro(
        &mut self,
        coroutine: &Coroutine,
        expression_id: IRExprID,
        ctx: &Context<'_>,
    ) {
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
        let engine = self.engine();
        let resolver = self.resolver();
        let layout = resolver.frame_layout(instance).await;

        self.store_constant(
            self.project(destination).coroutine_frame_state(),
            FrameState::NotStarted.tag(&layout),
        );

        // What the frame stores depends on the function it suspends.
        let body = resolver.coroutine_body(instance).await;
        let source = engine.get_ir(body.def_id()).await;
        let function_id = body.function_id(&source);
        let function = source.get_function(function_id);
        let parameters = function.parameter_locals(body.def_id(), engine).await;

        // `Def.call(closure, arguments)` takes the closure value and a
        // tuple, which the frame stores as the captures and
        // parameters of the closure's body.
        if function.is_closure() {
            let environment = arguments.next().expect("closure always has an environment");
            let tuple = arguments.next().expect("closure always has an argument tuple");

            assert!(
                arguments.next().is_none(),
                "closure always has two arguments: environment and argument tuple"
            );

            for (capture, _) in source.get_captures_of_function(function_id) {
                let value = self.project(&environment).environment_capture(capture);
                self.store_upvar(destination, Local::Capture(capture), value);
            }

            for (index, local) in parameters.into_iter().enumerate() {
                let value = self.project(&tuple).tuple_field(FieldIndex::from_usize(index));
                self.store_upvar(destination, local, value);
            }
        } else if function.is_def() {
            // A definition stores its arguments as they are.
            assert_eq!(parameters.len(), arguments.len(), "a `coro` passes every parameter");
            for (local, argument) in parameters.into_iter().zip(arguments) {
                self.store_upvar(destination, local, argument.clone());
            }
        } else {
            panic!("a `coro` must call a closure or a definition, not a {function:?}");
        }
    }

    /// Stores what `local` starts with in the frame at `frame`.
    fn store_upvar(&mut self, frame: &Place, local: Local, value: Place) {
        let field = self.project(frame).coroutine_frame_savable(Savable::Local(local));
        self.store(field, Operand::Copy(value));
    }
}

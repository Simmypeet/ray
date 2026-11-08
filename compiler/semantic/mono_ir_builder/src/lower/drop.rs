//! Lowers drops. A generated nominal `Drop.drop` has no source function, so
//! what does not read the source function is available on the [`Emitter`].

use qbice::storage::intern::Interned;
use rayc_ir::{
    address::Local,
    cfg::{AddressDrop, ExprDiscard},
    ir_closure::CaptureID,
};
use rayc_mono_ir::{
    MonoCoroutineInstance,
    cfg::{Switch, Terminator},
    layout::coroutine::FrameState,
    operand::Operand,
    place::{FieldIndex, Place},
};
use rayc_type::ty::Ty;

use super::coro::Upvars;
use crate::{context::Context, emitter::Emitter, resolver::InstanceCallable};

impl Emitter<'_> {
    /// Drops the unused value of an expression statement with the dictionary
    /// selected during type checking.
    pub(super) async fn lower_expr_discard(&mut self, discard: &ExprDiscard, ctx: &Context<'_>) {
        // `Drop.drop` returns unit, which is itself unused, so every call
        // writes to one scratch temporary.
        let destination = self.new_unit_place();

        let value = ctx.expression_place(discard.expression());
        self.lower_drop(value, discard.drop_instance(), destination).await;
    }

    /// Drops the value held in a place with the dictionary selected by drop
    /// elaboration. The place is passed to the drop directly, so no
    /// temporary holds the moved value.
    pub(super) async fn lower_address_drop(&mut self, drop: &AddressDrop, ctx: &Context<'_>) {
        // `Drop.drop` returns unit, which is unused, so every call writes to
        // one scratch temporary.
        let destination = self.new_unit_place();

        let value = self.lower_address(drop.address(), ctx);
        self.lower_drop(value, drop.drop_instance(), destination).await;
    }
}

impl Emitter<'_> {
    /// Drops `value` with the implementation selected by `dictionary`. Every
    /// emitted call writes its unit result to `destination`, and passes on
    /// `handlers` to what it calls.
    pub(crate) async fn lower_drop(
        &mut self,
        value: Place,
        dictionary: &Interned<Ty>,
        destination: Place,
    ) {
        match self.resolver().resolve_drop_call(dictionary).await {
            InstanceCallable::Definition(callee) => {
                let arguments = vec![Operand::Copy(value)];
                self.lower_call_with_args(callee.into(), arguments, Vec::new(), destination).await;
            }
            InstanceCallable::TupleDrop(element_instances) => {
                Box::pin(self.lower_tuple_drop(&value, &element_instances, destination)).await;
            }
            InstanceCallable::ClosureDrop(captures) => {
                Box::pin(self.lower_closure_drop(&value, &captures, destination)).await;
            }
            InstanceCallable::NominalDrop(instance) => {
                self.lower_nominal_drop_call(instance, vec![Operand::Copy(value)], destination);
            }
            InstanceCallable::CoroutineDrop { coroutine, upvars } => {
                Box::pin(self.lower_coroutine_drop(&value, &coroutine, &upvars, destination)).await;
            }
            InstanceCallable::NoOp => {}
            InstanceCallable::Closure(_, _) | InstanceCallable::Continuation(_) => {
                panic!("a Drop dictionary cannot call a closure or a continuation")
            }
        }
    }

    /// Expands a built-in tuple `Drop` call into the selected element calls.
    pub(crate) async fn lower_tuple_drop(
        &mut self,
        tuple: &Place,
        element_instances: &[Interned<Ty>],
        destination: Place,
    ) {
        // Destroy fields in reverse declaration order, matching scope teardown.
        for (index, element_instance) in element_instances.iter().enumerate().rev() {
            let field = self.project(tuple).tuple_field(FieldIndex::from_usize(index));
            self.lower_drop(field, element_instance, destination.clone()).await;
        }
    }

    /// Expands a built-in closure `Drop` call into the selected capture calls,
    /// one per capture with its `Drop` dictionary.
    pub(crate) async fn lower_closure_drop(
        &mut self,
        closure: &Place,
        captures: &[(CaptureID, Interned<Ty>)],
        destination: Place,
    ) {
        // A closure value is its inline environment, whose fields are the
        // captures. Destroy them in reverse order, matching tuple Drop.
        for (capture, capture_instance) in captures.iter().rev() {
            let capture = self.project(closure).environment_capture(*capture);
            self.lower_drop(capture, capture_instance, destination.clone()).await;
        }
    }

    /// Expands the built-in `Drop` call of the frame at `frame`, of
    /// `coroutine`, whose upvars have the `Drop` dictionaries `upvars`. What
    /// the frame holds depends on its state, so the drop switches on it, and
    /// leaves the block after the switch selected.
    ///
    /// Only a frame that has not started holds its upvars, which are dropped
    /// with the dictionaries of the `coro` that created it. A started frame is
    /// not dropped yet: what it holds leaks.
    ///
    /// ```c
    ///     switch (frame.state) {
    ///     case NOT_STARTED: goto drop_upvars;
    ///     default:          goto done;
    ///     }
    ///
    /// drop_upvars:
    ///     d1.drop(frame.param1);          // the last argument first
    ///     d0.drop(frame.param0);
    ///     goto done;
    ///
    /// done:                               // left selected
    /// ```
    pub(crate) async fn lower_coroutine_drop(
        &mut self,
        frame: &Place,
        coroutine: &MonoCoroutineInstance,
        upvars: &[Interned<Ty>],
        destination: Place,
    ) {
        let layout = self.resolver().frame_layout(coroutine).await;
        let state = Operand::Copy(self.project(frame).coroutine_frame_state());
        let not_started = FrameState::NotStarted.tag(&layout);

        // Only a frame that has not started drops its upvars.
        let drop_upvars = self.create_block();
        let done = self.create_block();
        let targets = vec![(not_started, drop_upvars)];
        self.set_terminator(Terminator::Switch(Switch::new(state, targets, done)));

        // A drop may leave another block selected, so the jump to `done` ends
        // whichever block is selected after the upvars.
        self.select_block(drop_upvars);
        self.drop_not_started_upvars(frame, coroutine, upvars, destination).await;
        self.set_terminator(Terminator::Goto(done));

        self.select_block(done);
    }

    /// Drops the upvars of the frame at `frame`, of `coroutine`, which has not
    /// started, with their dictionaries `upvars`. The last argument goes
    /// first, as when a function drops its parameters.
    async fn drop_not_started_upvars(
        &mut self,
        frame: &Place,
        coroutine: &MonoCoroutineInstance,
        upvars: &[Interned<Ty>],
        destination: Place,
    ) {
        match Upvars::of(coroutine, self.engine()).await {
            Upvars::Definition(parameters) => {
                assert_eq!(
                    parameters.len(),
                    upvars.len(),
                    "a coroutine of a definition has a `Drop` dictionary per parameter"
                );
                for (local, dictionary) in parameters.into_iter().zip(upvars).rev() {
                    let field = self.upvar_field(frame, local);
                    self.lower_drop(field, dictionary, destination.clone()).await;
                }
            }

            // The dictionaries are those of the closure and of the argument
            // tuple, whose captures and elements the frame holds as fields of
            // their own. The tuple is the last argument, so the parameters go
            // before the captures.
            Upvars::Closure { captures, parameters } => {
                let [closure, tuple] = upvars else {
                    panic!("`Def.call` takes the closure and a tuple of the arguments")
                };

                let InstanceCallable::TupleDrop(elements) =
                    self.resolver().resolve_drop_call(tuple).await
                else {
                    panic!("an argument tuple is dropped by the built-in tuple instance")
                };
                assert_eq!(
                    parameters.len(),
                    elements.len(),
                    "the argument tuple has an element per parameter of the closure"
                );
                for (local, dictionary) in parameters.into_iter().zip(&elements).rev() {
                    let field = self.upvar_field(frame, local);
                    self.lower_drop(field, dictionary, destination.clone()).await;
                }

                let InstanceCallable::ClosureDrop(capture_drops) =
                    self.resolver().resolve_drop_call(closure).await
                else {
                    panic!("a closure is dropped by the built-in closure instance")
                };
                assert!(
                    capture_drops.iter().map(|(capture, _)| *capture).eq(captures),
                    "the closure instance drops the captures that the frame holds"
                );
                for (capture, dictionary) in capture_drops.iter().rev() {
                    let field = self.upvar_field(frame, Local::Capture(*capture));
                    self.lower_drop(field, dictionary, destination.clone()).await;
                }
            }
        }
    }
}

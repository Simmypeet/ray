//! The box of a value that holds the frame of its own coroutine, or of one
//! that holds it in turn, which the frame would otherwise hold by value
//! forever.
//!
//! Three kinds of value can be boxed: a child frame, the storage of a `coro`,
//! and the `Coroutine` expression that a `coro` stores there. In theory, the
//! expression is never boxed: `coro` stores it right after it is evaluated,
//! so it never lives across a suspension point and the frame never holds it.
//! Its box is handled anyway, so that nothing depends on how `coro` is
//! lowered. A box lives as
//! long as the value in it: it is allocated just before the value is first
//! written, and freed once the value ends. Between the two, the member of the
//! value points to it, and every place of the value is read through it.
//!
//! | Value | Allocated | Freed |
//! |---|---|---|
//! | child frame | when the call or the `run ... with` starts | when the callee or the body returns, or when the owner is abandoned |
//! | `coro` storage | at the store that writes it | after its `AddressDrop` |
//! | `Coroutine` expression | when it is evaluated | after the store that moves it, or after its `ExprDiscard` |
//!
//! ```c
//!     frame->child_p = ray_alloc(sizeof(callee_frame_t));    // allocate
//!     *frame->child_p = (callee_frame_t){ ... };
//!     ...
//!     ray_free(frame->child_p);                               // free
//! ```

use rayc_coroutine_frame_layout::savable::Savable;
use rayc_ir::ir_expr::IRExprID;
use rayc_mono_ir::{
    operand::Operand,
    place::Place,
    ty::{MonoType, PointerMutability},
};

use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    /// Allocates the box of `savable` in the selected block, if the frame
    /// boxes it, and returns the member that points to it.
    ///
    /// ```c
    ///     frame->savable = ray_alloc(sizeof(value_t));
    /// ```
    pub(in crate::lower) fn allocate_frame_box(
        &mut self,
        savable: Savable,
        ctx: &Context<'_>,
    ) -> Option<Place> {
        let pointer = ctx.frame_box(savable)?;
        self.heap_allocate(pointer.clone(), ctx.raw_frame_field_type(savable));
        Some(pointer)
    }

    /// Frees the box of `savable` in the selected block, if the frame boxes
    /// it. Whatever it held must already be done with.
    ///
    /// ```c
    ///     ray_free(frame->savable);
    /// ```
    pub(in crate::lower) fn free_frame_box(&mut self, savable: Savable, ctx: &Context<'_>) {
        if let Some(pointer) = ctx.frame_box(savable) {
            self.heap_free(pointer);
        }
    }

    /// Copies the member that points to the box of `expression` into a new
    /// local, if the frame boxes it, and returns that local.
    ///
    /// Only the `Coroutine` expression of a `coro` can be boxed. Its only use
    /// is the store that the `coro` emits right after it, which moves it out
    /// of the box and then frees the box through this local.
    ///
    /// In theory, this never finds a box: the expression never lives across a
    /// suspension point, so the frame never holds it.
    ///
    /// ```c
    ///     e_box = frame->e;
    /// ```
    pub(in crate::lower) fn take_expression_box(
        &mut self,
        expression: IRExprID,
        ctx: &Context<'_>,
    ) -> Option<Place> {
        let savable = Savable::Expression(expression);
        let member = ctx.frame_box(savable)?;
        assert!(
            ctx.source_expression(expression).kind().as_coroutine().is_some(),
            "only the `Coroutine` expression of a `coro` holds a frame, so only it can be boxed"
        );

        let pointer_type = MonoType::new_pointer(
            ctx.raw_frame_field_type(savable),
            PointerMutability::Mut,
            self.engine(),
        );
        let pointer = self.new_temporary_place(pointer_type);
        self.store(pointer.clone(), Operand::Copy(member));
        Some(pointer)
    }
}

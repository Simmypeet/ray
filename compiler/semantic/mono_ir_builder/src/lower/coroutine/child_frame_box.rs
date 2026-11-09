//! The box of a child frame that holds the frame of its own coroutine, or of
//! one that holds it in turn, which the frame would otherwise hold by value
//! forever.
//!
//! The box lives as long as the child frame: it is allocated just before the
//! child frame is created, and freed once the child is done with it, whether
//! it completes or is abandoned. Between the two, the member of the slot
//! points to it, and every place of the child frame is read through it.
//!
//! ```c
//!     frame->child_p = ray_alloc(sizeof(callee_frame_t));    // allocate
//!     *frame->child_p = (callee_frame_t){ ... };
//!     ...
//!     ray_free(frame->child_p);                               // free
//! ```

use rayc_coroutine_frame_layout::suspension_point::SuspensionPointID;
use rayc_mono_ir::place::Place;

use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    /// Allocates the box of the child frame of `point` in the selected block,
    /// if the frame boxes it, and returns the member that points to it.
    ///
    /// ```c
    ///     frame->child_p = ray_alloc(sizeof(child_frame_t));
    /// ```
    pub(in crate::lower) fn allocate_child_frame_box(
        &mut self,
        point: SuspensionPointID,
        ctx: &Context<'_>,
    ) -> Option<Place> {
        let pointer = ctx.child_frame_box(point)?;
        self.heap_allocate(pointer.clone(), ctx.child_frame_type(point));
        Some(pointer)
    }

    /// Frees the box of the child frame of `point` in the selected block, if
    /// the frame boxes it. Whatever it held must already be done with.
    ///
    /// ```c
    ///     ray_free(frame->child_p);
    /// ```
    pub(in crate::lower) fn free_child_frame_box(
        &mut self,
        point: SuspensionPointID,
        ctx: &Context<'_>,
    ) {
        if let Some(pointer) = ctx.child_frame_box(point) {
            self.heap_free(pointer);
        }
    }
}

//! Lowering that only a `coroutine_resume` function does: the dispatch on the
//! state of the frame, what it returns, and `perform`.
//!
//! ```c
//! coroutine_progress_t coroutine_resume(coroutine_frame_t *frame, coroutine_resume_t args) {
//!     switch (frame->state) {                 // dispatch.rs
//!     case NOT_STARTED:   goto entry;
//!     case SUSPENDED_AT_p: goto resume_entry_p;
//!     }
//!
//! entry:
//!     ...
//!     frame->state = SUSPENDED_AT_p;          // yield.rs, through progress.rs
//!     progress = { YIELD, { op_tag, args } };
//!     return progress;
//!
//! resume_entry_p:                             // filled by the lowering of `p`
//!     result = args.member;
//!     goto after_p;
//!
//! after_p:
//!     ...
//!     frame->state = COMPLETED;               // progress.rs
//!     progress = { RETURN, value };
//!     return progress;
//! }
//! ```
//!
//! A suspension point that waits for another coroutine fills its resume entry
//! in `poll.rs` instead.

mod dispatch;
mod perform_yield;
mod progress;

pub(super) use progress::YieldArguments;

//! Contains the definition of the [`AbortJoinHandle`] type.

use std::{
    pin::Pin,
    task::{Context, Poll},
};

use tokio::task::JoinHandle;

use crate::panic_propagate::PanicPropagate;

/// A handle to a spawned task that aborts the task when dropped.
///
/// Awaiting the handle returns the result of the task; a panic thrown from
/// the task is resumed with its original payload.
#[derive(Debug)]
#[must_use = "dropping the handle aborts the task"]
pub struct AbortJoinHandle<T> {
    handle: JoinHandle<T>,
}

impl<T: Send + 'static> AbortJoinHandle<T> {
    /// Spawns a new task, returning the handle to it.
    pub fn spawn<F: Future<Output = T> + Send + 'static>(future: F) -> Self {
        Self { handle: tokio::spawn(future) }
    }
}

impl<T> Drop for AbortJoinHandle<T> {
    fn drop(&mut self) { self.handle.abort(); }
}

impl<T> Future for AbortJoinHandle<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.handle).poll(cx).map(|result| {
            result.panic_propagate().expect("the task is aborted only when the handle is dropped")
        })
    }
}

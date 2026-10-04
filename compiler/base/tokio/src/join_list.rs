//! Contains the definition of the [`JoinList`] type.

use std::collections::VecDeque;

use crate::abort_join_handle::AbortJoinHandle;

#[cfg(test)]
mod test;

/// A collection of spawned tasks that are joined in the order they were
/// spawned, regardless of the order they finish in.
///
/// The tasks that haven't been joined are aborted when the list is dropped.
#[derive(Debug)]
pub struct JoinList<T> {
    handles: VecDeque<AbortJoinHandle<T>>,
}

impl<T> Default for JoinList<T> {
    fn default() -> Self { Self { handles: VecDeque::new() } }
}

impl<T: Send + 'static> JoinList<T> {
    /// Creates a new empty [`JoinList`].
    #[must_use]
    pub fn new() -> Self { Self::default() }

    /// Spawns a new task, which will be joined after all the tasks spawned
    /// before it.
    pub fn spawn<F: Future<Output = T> + Send + 'static>(&mut self, future: F) {
        self.handles.push_back(AbortJoinHandle::spawn(future));
    }

    /// Awaits the earliest spawned task that hasn't been joined, returning
    /// its result, or `None` if every task has been joined.
    ///
    /// A panic thrown from the task is resumed with its original payload.
    pub async fn next(&mut self) -> Option<T> {
        // the task stays in the list until it completes, so that it is still
        // aborted if this future is dropped while waiting
        let value = self.handles.front_mut()?.await;
        self.handles.pop_front();

        Some(value)
    }
}

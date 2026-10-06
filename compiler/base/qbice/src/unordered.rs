//! Contains the utilities for calling independent queries concurrently.

use qbice::Query;
use rayc_extend::extend;
use rayc_tokio::{
    abort_join_handle::AbortJoinHandle, chunk::chunk_size_for_tasks, join_list::JoinList,
};

use crate::TrackedEngine;

/// A group of queries called by the currently computing query, none of which
/// is called depending on the value of another.
///
/// The queries called while the group is alive are recorded as unordered
/// callees, which the engine repairs concurrently rather than one after
/// another in the order they were called.
#[derive(Debug)]
pub struct UnorderedCalleeGroup<'e> {
    engine: &'e TrackedEngine,
}

impl<'e> UnorderedCalleeGroup<'e> {
    /// Starts a group that lasts until the returned value is dropped.
    ///
    /// # Safety
    ///
    /// While the group is alive, the computing query must not decide whether
    /// to call a query based on the value of another query of the group. See
    /// [`qbice::TrackedEngine::start_unordered_callee_group`] for details.
    ///
    /// # Panics
    ///
    /// Panics if called outside of an executor, or while another group of the
    /// same computing query is alive.
    #[must_use]
    pub unsafe fn start(engine: &'e TrackedEngine) -> Self {
        // SAFETY: upheld by the caller
        unsafe { engine.start_unordered_callee_group() };

        Self { engine }
    }
}

impl Drop for UnorderedCalleeGroup<'_> {
    fn drop(&mut self) {
        // SAFETY: the group was started when this value was created
        unsafe { self.engine.end_unordered_callee_group() };
    }
}

/// Queries all of the given keys concurrently, returning their values in the
/// order of the keys.
///
/// The queries are recorded as an [`UnorderedCalleeGroup`], so the computing
/// query must not call any other query until the returned future completes.
///
/// # Panics
///
/// Panics if called outside of an executor.
#[extend]
pub async fn query_all<Q: Query>(
    self: &TrackedEngine,
    keys: impl ExactSizeIterator<Item = Q>,
) -> Vec<Q::Value> {
    let len = keys.len();
    if len == 0 {
        return Vec::new();
    }

    // SAFETY: the keys are all known before any of them is queried, so none
    // of them is queried depending on the value of another
    let _group = unsafe { UnorderedCalleeGroup::start(self) };

    // queries the keys in chunks, one task per chunk; each chunk is taken
    // straight out of the iterator so that the keys are allocated only once
    let chunk_size = chunk_size_for_tasks(len);
    let mut keys = keys.peekable();
    let mut tasks = JoinList::new();
    while keys.peek().is_some() {
        let engine = self.clone();
        let chunk = keys.by_ref().take(chunk_size).collect::<Vec<_>>();

        tasks.spawn(async move {
            let mut values = Vec::with_capacity(chunk.len());
            for key in &chunk {
                values.push(engine.query(key).await);
            }

            values
        });
    }

    // joins the chunks in the order they were spawned, which is the order of
    // the keys
    let mut values = Vec::with_capacity(len);
    while let Some(chunk) = tasks.next().await {
        values.extend(chunk);
    }

    values
}

/// Queries the given key in a task of its own, so that it runs in parallel
/// with the other queries called by the computing query in the meantime.
///
/// Calling queries in parallel makes the order they are called in arbitrary,
/// so this is meant to be used within an [`UnorderedCalleeGroup`].
#[extend]
pub async fn query_in_task<Q: Query>(self: &TrackedEngine, key: Q) -> Q::Value {
    let engine = self.clone();

    AbortJoinHandle::spawn(async move { engine.query(&key).await }).await
}

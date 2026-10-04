//! Contains the utility for chunking slices for Tokio tasks.

use std::slice::Chunks;

use rayc_extend::extend;

/// Returns the number of items each Tokio task should process, given the
/// total number of items to distribute over the tasks.
#[must_use]
pub fn chunk_size_for_tasks(len: usize) -> usize {
    // chunk the items to avoid spawning too many tasks
    // at once, targeting 4x the number of available
    let num_threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get) * 4;

    len.div_ceil(num_threads).max(1)
}

/// Chunks a slice into manageable pieces for Tokio tasks.
#[extend]
pub fn chunk_for_tasks<'a, T: 'a>(self: &'a [T]) -> Chunks<'a, T> {
    self.chunks(chunk_size_for_tasks(self.len()))
}

//! Finds which candidates need storage at the same point of a coroutine, and
//! so must not share memory.

use std::hash::Hash;

use rayc_hash::FxHashMap;
use rayc_ir::cfg::BlockID;
use rayc_transitive_closure::BitSet;

use crate::{
    candidate_index::CandidateIndex, layout_context::LayoutContext, savable::Savable,
    suspension_point::SuspensionPointID,
};

/// Every set of one index has one bit per candidate, so two of them always
/// have the same size.
const SAME_INDEX: &str = "sets of one candidate index have the same size";

/// Which candidates of an instance need storage at the same point of the IR
/// function.
///
/// Two such candidates must not share memory: if they were in overlapping
/// variants, writing one could overwrite the other while it is still needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageConflicts {
    index: CandidateIndex,

    /// Row `x`, bit `y`: the candidates numbered `x` and `y` need storage at
    /// some common point. A row also has its own bit, which means nothing.
    rows: Vec<BitSet>,
}

impl StorageConflicts {
    /// Returns whether the distinct values `a` and `b` need storage at some
    /// common point. A value that is not a candidate conflicts with nothing.
    #[must_use]
    pub fn conflicts(&self, a: Savable, b: Savable) -> bool {
        let (Some(a), Some(b)) = (self.index.number_of(a), self.index.number_of(b)) else {
            return false;
        };

        a != b && self.rows[a].get(b).expect(SAME_INDEX)
    }

    /// Returns the candidates to move to the prefix so that no two candidates
    /// of different variants that need storage at a common point are left,
    /// in ascending order.
    ///
    /// `variant_of` gives the suspension point whose variant holds a
    /// candidate.
    pub fn promotions(
        &self,
        variant_of: impl FnMut(Savable) -> SuspensionPointID,
    ) -> impl Iterator<Item = Savable> + '_ {
        let variant_of = self.index.values().map(variant_of).collect::<Vec<_>>();
        let promoted = promote_conflicts(&self.rows, &variant_of);

        self.index
            .values()
            .enumerate()
            .filter(move |(number, _)| promoted.get(*number).expect(SAME_INDEX))
            .map(|(_, value)| value)
    }
}

/// Returns the numbers of the candidates to move to the prefix so that no two
/// conflicting candidates of different variants are left.
///
/// `rows` are the rows of [`StorageConflicts`] and `variant_of[x]` is the
/// variant of candidate `x`. This is rustc's greedy rule: of each pair still
/// in conflict, the candidate with more conflicts is promoted, and a tie goes
/// to the smaller number.
fn promote_conflicts<V: Copy + Eq + Hash>(rows: &[BitSet], variant_of: &[V]) -> BitSet {
    debug_assert_eq!(rows.len(), variant_of.len(), "one variant per candidate");
    let candidates = rows.len();

    // The candidates of each variant. Two candidates of one variant are
    // separate fields of it, so they never conflict.
    let mut in_variant = FxHashMap::<V, BitSet>::default();
    for (number, variant) in variant_of.iter().enumerate() {
        in_variant
            .entry(*variant)
            .or_insert_with(|| BitSet::new(candidates))
            .set(number)
            .expect(SAME_INDEX);
    }

    // The conflicts that count: each row without its own variant, which also
    // drops the row's own bit. They are counted once, before any promotion.
    let others = rows
        .iter()
        .zip(variant_of)
        .map(|(row, variant)| {
            let mut others = row.clone();
            others.difference(&in_variant[variant]).expect(SAME_INDEX);
            others
        })
        .collect::<Vec<_>>();
    let counts = others.iter().map(BitSet::count_ones).collect::<Vec<_>>();

    // Each pair once, by ascending `(x, y)`, so the result is the same in
    // every run.
    let mut promoted = BitSet::new(candidates);
    for (x, others_of_x) in others.iter().enumerate() {
        for y in others_of_x.ones().filter(|&y| y > x) {
            // `x` was promoted by an earlier pair of its row, which resolves
            // the rest of them
            if promoted.get(x).expect(SAME_INDEX) {
                break;
            }

            // resolved by an earlier promotion of `y`
            if promoted.get(y).expect(SAME_INDEX) {
                continue;
            }

            let loser = if counts[x] >= counts[y] { x } else { y };
            promoted.set(loser).expect(SAME_INDEX);
        }
    }

    promoted
}

/// Builds the rows of [`StorageConflicts`] from the storage sets of the points
/// of a function, one set at a time.
#[derive(Debug)]
struct ConflictRecorder {
    rows: Vec<BitSet>,

    /// The last set recorded. Every two of its members are already in `rows`.
    recorded: BitSet,
}

impl ConflictRecorder {
    fn new(index: &CandidateIndex) -> Self {
        Self { rows: vec![index.empty_set(); index.len()], recorded: index.empty_set() }
    }

    /// Records that every two members of `storage` need storage at the same
    /// point.
    fn record(&mut self, storage: &BitSet) {
        // Every pair of a subset of `recorded` is a pair of `recorded`, so it
        // is already in `rows`. Most points add no candidate to the set of
        // the point after them, so most points stop here.
        if storage.is_subset(&self.recorded).expect(SAME_INDEX) {
            return;
        }

        // OR-ing the whole set into the row of each member records every
        // pair of members, in both directions.
        for member in storage.ones() {
            self.rows[member].or(storage).expect(SAME_INDEX);
        }

        self.recorded = storage.clone();
    }
}

impl LayoutContext<'_> {
    /// Finds which candidates of `index` need storage at the same point of
    /// the IR function.
    ///
    /// A candidate needs storage at a point when it is live just before or
    /// just after it, written there, or may be borrowed just before or just
    /// after it. An awaitee needs storage at its own suspension point.
    ///
    /// `suspends_at` tells whether the function still suspends at a
    /// suspension point in this instance. Such a terminator is two moments,
    /// see [`Self::record_suspension`].
    #[must_use]
    pub fn storage_conflicts(
        &self,
        index: CandidateIndex,
        suspends_at: impl Fn(SuspensionPointID) -> bool,
    ) -> StorageConflicts {
        let mut recorder = ConflictRecorder::new(&index);

        // Every point belongs to exactly one block and its storage set reads
        // only that block's facts, so the blocks are independent.
        for (block_id, _) in self.ir_function().traverse() {
            let suspends = suspends_at(SuspensionPointID::new(block_id));
            self.record_block(&index, block_id, suspends, &mut recorder);
        }

        StorageConflicts { index, rows: recorder.rows }
    }

    /// Records the storage set of every point of `block_id`, from the
    /// terminator back to the first instruction. `suspends` tells whether
    /// its terminator is a suspension point of this instance.
    fn record_block(
        &self,
        index: &CandidateIndex,
        block_id: BlockID,
        suspends: bool,
        recorder: &mut ConflictRecorder,
    ) {
        let live = self.live_candidates_before(index, block_id);
        let borrowed = self.borrowed_candidates_before(index, block_id);
        let terminator_idx = live.len() - 1;

        // The terminator: two moments at a suspension, otherwise one. What
        // it needs after it is what is live at the exit and its results.
        let results = self
            .ir_function()
            .block_terminator(block_id)
            .into_iter()
            .flat_map(|terminator| terminator.evaluated_expressions().map(Savable::Expression));
        let mut after = self.live_candidates_at_exit(index, block_id);
        index.extend_set(&mut after, results);
        if suspends {
            Self::record_suspension(
                index,
                Savable::Awaitee(SuspensionPointID::new(block_id)),
                &live[terminator_idx],
                after,
                &borrowed[terminator_idx],
                recorder,
            );
        } else {
            // what it reads, what it writes and what is live after it, all
            // at once
            let mut storage = after;
            storage.or(&live[terminator_idx]).expect(SAME_INDEX);
            storage.or(&borrowed[terminator_idx]).expect(SAME_INDEX);
            recorder.record(&storage);
        }

        // The instructions, last to first: live or borrowed on either side of
        // the instruction, or written by it.
        for (point, instruction) in self.ir_function().block_instructions_with_points_rev(block_id)
        {
            let idx = point.instruction_idx();

            let mut storage = index.set_of(Savable::written_by(instruction));
            for set in [&live[idx], &live[idx + 1], &borrowed[idx], &borrowed[idx + 1]] {
                storage.or(set).expect(SAME_INDEX);
            }
            recorder.record(&storage);
        }
    }

    /// Records the suspension that terminates `block_id` as two moments, so
    /// that its awaitee and its results may share memory:
    ///
    /// - suspending: what is live `before` it is read and the awaitee is built;
    /// - resuming: the awaitee is done, and what is live `after` it, which
    ///   holds the results, is written.
    ///
    /// What may be `borrowed` is pointed at during both.
    ///
    /// This is sound only if the state machine finishes with the awaitee
    /// before it writes a result: a result is returned by value and then
    /// stored, as in `frame.v_next.r = resume(&frame.v_this.awaitee)`, never
    /// written through a pointer into the frame while the awaitee runs.
    fn record_suspension(
        index: &CandidateIndex,
        awaitee: Savable,
        before: &BitSet,
        mut after: BitSet,
        borrowed: &BitSet,
        recorder: &mut ConflictRecorder,
    ) {
        // resuming, recorded first since the walk goes backwards
        after.or(borrowed).expect(SAME_INDEX);
        recorder.record(&after);

        // suspending
        let mut suspending = before.clone();
        suspending.or(borrowed).expect(SAME_INDEX);
        // NOTE: It's fine to directly add the awaitee without checking whether
        // if the suspension point has an awaitee. It's fine because if it
        // doesn't have an awaitee, then the CandidateIndex will filter it out
        // and not include it in the BitSet.
        index.extend_set(&mut suspending, std::iter::once(awaitee));
        recorder.record(&suspending);
    }
}

#[cfg(test)]
mod tests;

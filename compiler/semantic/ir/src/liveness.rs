//! Backward liveness analyses of an IR function.
//!
//! A value is **use-live** at a point when some path from that point uses it,
//! and **drop-live** when its value is not used again on any path, but is
//! still dropped by a non-no-op `Drop` instance on some path. Each value has
//! one live mode: a use subsumes a drop, since a use keeps everything in its
//! type alive anyway.
//!
//! The borrow checker needs the distinction because a drop keeps fewer
//! regions of the dropped type alive than a use does.
//!
//! - [`local`] computes the liveness of locals: variables and captures.
//! - [`expr`] computes the liveness of expression values.
//!
//! Each analysis is solved per block. [`LiveRanges`] then records, for every
//! value, the points at which it is live, so that the liveness of one value
//! at one point can be looked up without replaying its block.

use std::{
    collections::{BTreeMap, btree_map::Entry},
    convert::Infallible,
    hash::Hash,
    ops::Range,
};

use rayc_hash::FxHashMap;

use crate::{
    cfg::{BlockID, Instruction, Point},
    dataflow::JoinLattice,
};

pub mod expr;
pub mod local;

/// The reason a value is live at one point of a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveMode {
    /// The value is only dropped later by a non-no-op `Drop` instance.
    Drop,
    /// The value may be used later, which subsumes a later drop.
    Use,
}

/// The values live at one point of a function and their live modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSet<T> {
    live: BTreeMap<T, LiveMode>,
}

impl<T> Default for LiveSet<T> {
    fn default() -> Self { Self { live: BTreeMap::new() } }
}

impl<T: Ord + Copy> LiveSet<T> {
    /// Returns whether the current value of `value` may be used later.
    #[must_use]
    pub fn is_use_live(&self, value: T) -> bool { self.live.get(&value) == Some(&LiveMode::Use) }

    /// Returns whether the current value of `value` may be dropped later
    /// without being used first.
    #[must_use]
    pub fn is_drop_live(&self, value: T) -> bool { self.live.get(&value) == Some(&LiveMode::Drop) }

    /// Returns the use-live values, in order.
    pub fn use_live(&self) -> impl Iterator<Item = T> + '_ {
        self.live.iter().filter_map(|(&value, &mode)| (mode == LiveMode::Use).then_some(value))
    }

    /// Returns the drop-live values, in order.
    pub fn drop_live(&self) -> impl Iterator<Item = T> + '_ {
        self.live.iter().filter_map(|(&value, &mode)| (mode == LiveMode::Drop).then_some(value))
    }
}

/// Receives the effects of an instruction on liveness, while the state moves
/// backward from just after the instruction to just before it.
///
/// The transfer functions of the analyses are written against this trait, so
/// that the dataflow solver and [`LiveRanges`] replay the same effects.
pub(crate) trait LiveEffects<T> {
    /// Records a use of `value`, which subsumes a later drop of it.
    fn mark_used(&mut self, value: T);

    /// Records a drop of `value`, unless it is used later anyway.
    fn mark_dropped(&mut self, value: T);

    /// Records a definition of `value`, before which its new value is dead.
    fn mark_defined(&mut self, value: T);
}

impl<T: Ord> LiveEffects<T> for LiveSet<T> {
    fn mark_used(&mut self, value: T) { self.live.insert(value, LiveMode::Use); }

    fn mark_dropped(&mut self, value: T) { self.live.entry(value).or_insert(LiveMode::Drop); }

    fn mark_defined(&mut self, value: T) { self.live.remove(&value); }
}

impl<T, D> JoinLattice<D, Infallible> for LiveSet<T>
where
    T: Ord + Copy + Send + Sync,
    D: Sync + ?Sized,
{
    async fn join(&mut self, other: &Self, _dataflow_problem_ctx: &D) -> Result<bool, Infallible> {
        let mut changed = false;

        // A local used on any path is use-live, which subsumes being dropped
        // on another path.
        for (&value, &mode) in &other.live {
            match self.live.entry(value) {
                Entry::Vacant(entry) => {
                    entry.insert(mode);
                    changed = true;
                }
                Entry::Occupied(mut entry) => {
                    if *entry.get() == LiveMode::Drop && mode == LiveMode::Use {
                        entry.insert(LiveMode::Use);
                        changed = true;
                    }
                }
            }
        }

        Ok(changed)
    }
}

/// Finds how the effects of one instruction touch one value: whether they
/// use it, only drop it, or neither.
struct UseProbe<T> {
    value: T,
    mode: Option<LiveMode>,
}

impl<T> UseProbe<T> {
    const fn new(value: T) -> Self { Self { value, mode: None } }
}

impl<T: PartialEq> LiveEffects<T> for UseProbe<T> {
    fn mark_used(&mut self, value: T) {
        if value == self.value {
            self.mode = Some(LiveMode::Use);
        }
    }

    fn mark_dropped(&mut self, value: T) {
        if value == self.value && self.mode.is_none() {
            self.mode = Some(LiveMode::Drop);
        }
    }

    // A definition makes the previous value dead, rather than using it.
    fn mark_defined(&mut self, _value: T) {}
}

/// A run of consecutive points of one block at which a value keeps one live
/// mode.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveRange {
    /// The instruction indices of the points of the run, half-open as in
    /// `start..end`, so a run of the single point `3` is `3..4`.
    points: Range<usize>,

    mode: LiveMode,
}

/// The points at which each value of a function is live, and in which mode.
///
/// A value is live at a point when it is live just before the instruction
/// there. The point one past the last instruction of a block stands for its
/// terminator.
///
/// Each value is stored as runs of points per block, so a value live through
/// a whole block takes a single run however long the block is.
#[derive(Debug, Clone)]
pub struct LiveRanges<T> {
    /// For each reachable block, the runs of each value live somewhere in it,
    /// sorted by position and disjoint.
    blocks: FxHashMap<BlockID, FxHashMap<T, Vec<LiveRange>>>,
}

impl<T: Copy + Eq + Hash> LiveRanges<T> {
    /// Returns the mode in which `value` is live at `point`, or `None` when
    /// it is dead there or the block of `point` is unreachable.
    #[must_use]
    pub fn live_mode(&self, value: T, point: Point) -> Option<LiveMode> {
        let ranges = self.blocks.get(&point.block_id())?.get(&value)?;
        let index = point.instruction_idx();

        // The runs are sorted and disjoint, so the first run that ends after
        // `index` is the only one that can contain it.
        let range = ranges.get(ranges.partition_point(|range| range.points.end <= index))?;
        range.points.contains(&index).then_some(range.mode)
    }
}

impl<T> LiveRanges<T> {
    /// Collects the runs of each reachable block; see [`block_live_ranges`].
    fn from_blocks(
        blocks: impl IntoIterator<Item = (BlockID, FxHashMap<T, Vec<LiveRange>>)>,
    ) -> Self {
        Self { blocks: blocks.into_iter().collect() }
    }
}

/// Records the runs of each value of one block while its instructions are
/// replayed backward, one [`LiveEffects`] at a time.
///
/// Only the values an instruction touches are looked at, so a value live
/// across the whole block costs nothing until the walk reaches its start.
struct RangeRecorder<T> {
    /// The instruction index whose effects are being replayed. The state
    /// moves from just before `point + 1` to just before `point`.
    point: usize,

    /// For each value live just before `point + 1`, its mode and the end of
    /// its current run, one past the run's last point.
    open: FxHashMap<T, (LiveMode, usize)>,

    /// The new mode of each value touched at `point`, where `None` means
    /// dead. Several effects of one instruction may touch the same value, so
    /// runs are only split once the instruction is done; see
    /// [`Self::finish_point`].
    touched: FxHashMap<T, Option<LiveMode>>,

    /// The finished runs of each value, from the end of the block backward.
    closed: FxHashMap<T, Vec<LiveRange>>,
}

impl<T: Copy + Eq + Hash> RangeRecorder<T> {
    /// Starts recording a block whose terminator is at `terminator`, with
    /// the values in `exit` live at its exit.
    ///
    /// The exit is not a point of its own. Its runs start out ending just
    /// after the terminator, and the terminator's effects decide whether they
    /// cover it.
    fn new(exit: &LiveSet<T>, terminator: usize) -> Self {
        Self {
            point: terminator,
            open: exit.live.iter().map(|(&value, &mode)| (value, (mode, terminator + 1))).collect(),
            touched: FxHashMap::default(),
            closed: FxHashMap::default(),
        }
    }

    /// Returns the mode of `value` after the effects replayed so far.
    fn current_mode(&self, value: T) -> Option<LiveMode> {
        match self.touched.get(&value) {
            Some(&mode) => mode,
            None => self.open.get(&value).map(|&(mode, _)| mode),
        }
    }

    /// Ends the replay of the effects at `point`, splitting the runs of the
    /// values whose mode changed there, and moves to the previous point.
    fn finish_point(&mut self) {
        let point = self.point;
        for (value, mode) in self.touched.drain() {
            let previous = self.open.get(&value).map(|&(mode, _)| mode);
            if previous == mode {
                continue;
            }

            // The effects at `point` decide the mode at `point` itself, so the
            // previous run only covers the points after it. A run from the
            // exit that the terminator changes covers no point at all.
            if let Some((previous, end)) = self.open.remove(&value) {
                let points = point + 1..end;
                if !points.is_empty() {
                    self.closed
                        .entry(value)
                        .or_default()
                        .push(LiveRange { points, mode: previous });
                }
            }

            // The new run covers `point` and extends backward from it.
            if let Some(mode) = mode {
                self.open.insert(value, (mode, point + 1));
            }
        }

        self.point = point.saturating_sub(1);
    }

    /// Ends the recording at the start of the block and returns the runs of
    /// each value, sorted by position.
    fn finish(mut self) -> FxHashMap<T, Vec<LiveRange>> {
        for (value, (mode, end)) in self.open.drain() {
            self.closed.entry(value).or_default().push(LiveRange { points: 0..end, mode });
        }

        // The runs were finished from the end of the block backward.
        for ranges in self.closed.values_mut() {
            ranges.reverse();
        }
        self.closed
    }
}

impl<T: Copy + Eq + Hash> LiveEffects<T> for RangeRecorder<T> {
    fn mark_used(&mut self, value: T) { self.touched.insert(value, Some(LiveMode::Use)); }

    fn mark_dropped(&mut self, value: T) {
        if self.current_mode(value).is_none() {
            self.touched.insert(value, Some(LiveMode::Drop));
        }
    }

    fn mark_defined(&mut self, value: T) { self.touched.insert(value, None); }
}

/// Returns the runs of each value live somewhere in a block.
///
/// The block's values live at its exit are `exit`. The walk goes backward:
/// `transfer_terminator` replays the effects of the terminator, and
/// `transfer` those of each instruction.
fn block_live_ranges<T: Copy + Eq + Hash>(
    exit: &LiveSet<T>,
    instructions: &[Instruction],
    transfer_terminator: impl FnOnce(&mut RangeRecorder<T>),
    mut transfer: impl FnMut(&Instruction, &mut RangeRecorder<T>),
) -> FxHashMap<T, Vec<LiveRange>> {
    let mut recorder = RangeRecorder::new(exit, instructions.len());

    transfer_terminator(&mut recorder);
    recorder.finish_point();

    for instruction in instructions.iter().rev() {
        transfer(instruction, &mut recorder);
        recorder.finish_point();
    }

    recorder.finish()
}

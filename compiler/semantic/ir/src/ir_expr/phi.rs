use std::cmp::Ordering;

use qbice::{Decode, Encode, StableHash};
use rayc_hash::FxHashMap;

use crate::{cfg::BlockID, ir_expr::IRExprID};

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct Phi {
    incoming: FxHashMap<BlockID, IRExprID>,
}

impl Phi {
    #[must_use]
    pub const fn new(incoming: FxHashMap<BlockID, IRExprID>) -> Self { Self { incoming } }

    #[must_use]
    pub fn value_from(&self, predecessor: BlockID) -> Option<IRExprID> {
        self.incoming.get(&predecessor).copied()
    }

    pub fn incoming(&self) -> impl Iterator<Item = (BlockID, IRExprID)> + '_ {
        self.incoming.iter().map(|(block, value)| (*block, *value))
    }

    /// Makes the value that flows in from `predecessor` flow in from
    /// `split_block` instead, after the edge between them was split.
    ///
    /// When `predecessor` still reaches this phi's block through another
    /// edge, as when both arms of a conditional target it, the value flows in
    /// from both blocks.
    pub(crate) fn split_incoming(
        &mut self,
        predecessor: BlockID,
        split_block: BlockID,
        predecessor_still_incoming: bool,
    ) {
        let value = if predecessor_still_incoming {
            self.value_from(predecessor)
        } else {
            self.incoming.remove(&predecessor)
        };

        if let Some(value) = value {
            assert!(
                self.incoming.insert(split_block, value).is_none(),
                "a new split block cannot already be a phi predecessor"
            );
        }
    }

    #[must_use]
    pub fn len(&self) -> usize { self.incoming.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.incoming.is_empty() }

    fn sorted_incoming(&self) -> Vec<(BlockID, IRExprID)> {
        let mut incoming: Vec<_> = self.incoming().collect();
        incoming.sort_unstable();
        incoming
    }
}

impl PartialOrd for Phi {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> { Some(self.cmp(other)) }
}

impl Ord for Phi {
    fn cmp(&self, other: &Self) -> Ordering { self.sorted_incoming().cmp(&other.sorted_incoming()) }
}

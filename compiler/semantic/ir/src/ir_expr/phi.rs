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

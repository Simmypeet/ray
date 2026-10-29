//! Contains the definition of [`TransitiveClosure`]

use getset::{CopyGetters, Getters};
use qbice::{Decode, Encode, StableHash};

/// Used for efficiently representing an array of true/false values.
///
/// Instead of representing each boolean value as a byte, we can represent 64
/// boolean values as a single 64-bit integer.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct BitSet {
    bits: Vec<u64>,
    size: usize,
}

impl BitSet {
    /// Creates a new dynamic bitset with the given size.
    #[must_use]
    pub fn new(size: usize) -> Self {
        let bits = vec![0; size.div_ceil(64)];
        Self { bits, size }
    }

    /// Set the bit at the given index to `true`
    ///
    /// # Returns
    ///
    /// Returns `None` if the index is out of bounds, otherwise returns `Some`
    /// with `true` if there was a change in the bitset.
    #[must_use]
    pub fn set(&mut self, index: usize) -> Option<bool> {
        if index >= self.size {
            return None;
        }

        let word = index / 64;
        let bit = index % 64;

        let old = self.bits[word];
        self.bits[word] = old | (1 << bit);

        Some(old != self.bits[word])
    }

    /// Set all bits to 0
    pub fn clear(&mut self) {
        for word in &mut self.bits {
            *word = 0;
        }
    }

    /// Set the bit at the given index to `false`
    ///
    /// # Returns
    ///
    /// Returns `None` if the index is out of bounds, otherwise returns `Some`
    /// with `true` if there was a change in the bitset.
    #[must_use]
    pub fn unset(&mut self, index: usize) -> Option<bool> {
        if index >= self.size {
            return None;
        }

        let word = index / 64;
        let bit = index % 64;

        let old = self.bits[word];
        self.bits[word] = old & !(1 << bit);

        Some(old != self.bits[word])
    }

    /// Get the value of the bit at the given index
    ///
    /// # Returns
    ///
    /// Returns `None` if the index is out of bounds, otherwise returns `Some`
    /// with the value of the bit.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<bool> {
        if index >= self.size {
            return None;
        }

        let word = index / 64;
        let bit = index % 64;
        Some((self.bits[word] & (1 << bit)) != 0)
    }

    /// Perform a bitwise OR operation with another bitset
    ///
    /// # Returns
    ///
    /// Returns `None` if the bitsets have different sizes, otherwise returns
    /// `Some` with `true` if there was a change in the bitset.
    #[must_use]
    pub fn or(&mut self, other: &Self) -> Option<bool> {
        if self.size != other.size {
            return None;
        }

        let mut changed = false;
        for (i, word) in other.bits.iter().enumerate() {
            let old = self.bits[i];
            self.bits[i] = old | word;

            changed |= old != self.bits[i];
        }

        Some(changed)
    }

    /// Perform a bitwise AND operation with another bitset
    ///
    /// # Returns
    ///
    /// Returns `None` if the bitsets have different sizes, otherwise returns
    /// `Some` with `true` if there was a change in the bitset.
    #[must_use]
    pub fn and(&mut self, other: &Self) -> Option<bool> {
        if self.size != other.size {
            return None;
        }

        let mut changed = false;
        for (i, word) in other.bits.iter().enumerate() {
            let old = self.bits[i];
            self.bits[i] = old & word;
            changed |= old != self.bits[i];
        }

        Some(changed)
    }

    /// Perform a bitwise AND NOT operation with another bitset, clearing every
    /// bit that is set in `other`
    ///
    /// # Returns
    ///
    /// Returns `None` if the bitsets have different sizes, otherwise returns
    /// `Some` with `true` if there was a change in the bitset.
    #[must_use]
    pub fn difference(&mut self, other: &Self) -> Option<bool> {
        if self.size != other.size {
            return None;
        }

        let mut changed = false;
        for (i, word) in other.bits.iter().enumerate() {
            let old = self.bits[i];
            self.bits[i] = old & !word;
            changed |= old != self.bits[i];
        }

        Some(changed)
    }

    /// Checks if every bit set in this bitset is also set in `other`
    ///
    /// # Returns
    ///
    /// Returns `None` if the bitsets have different sizes.
    #[must_use]
    pub fn is_subset(&self, other: &Self) -> Option<bool> {
        if self.size != other.size {
            return None;
        }

        Some(self.bits.iter().zip(&other.bits).all(|(mine, theirs)| mine & !theirs == 0))
    }

    /// Returns the number of bits set to 1
    #[must_use]
    pub fn count_ones(&self) -> usize {
        self.bits.iter().map(|word| word.count_ones() as usize).sum()
    }

    /// Iterates over the indices of the bits set to 1, in ascending order
    pub fn ones(&self) -> impl Iterator<Item = usize> + '_ {
        self.bits.iter().enumerate().flat_map(|(word_index, &word)| {
            let mut remaining = word;

            std::iter::from_fn(move || {
                if remaining == 0 {
                    return None;
                }

                let bit = remaining.trailing_zeros() as usize;

                // clear the lowest set bit, which is the one just found
                remaining &= remaining - 1;

                Some(word_index * 64 + bit)
            })
        })
    }

    /// Checks if any bit is set to 1
    #[allow(unused)]
    #[must_use]
    pub fn is_not_zero(&mut self) -> bool { self.bits.iter().any(|word| *word != 0) }
}

/// A data structure to compute the transitive closure of a directed graph.
///
/// The transitive closure has a fixed size of nodes and is computed using the
/// Warshall's adjacency matrix algorithm.
///
/// Since most of the compiler's analysis is based on the directed graph
/// problem, this data structure is used to compute the transitive closure of
/// the graph and check if there is a path between two nodes efficiently.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Getters, CopyGetters, StableHash, Encode, Decode)]
pub struct TransitiveClosure {
    /// The number of nodes in the graph
    #[get_copy = "pub"]
    size: usize,

    closure: Vec<BitSet>,
}

impl TransitiveClosure {
    /// Creates a new transitive closure matrix from a list of edges and the
    /// size of the matrix.
    #[allow(clippy::needless_range_loop, clippy::needless_pass_by_value)]
    #[must_use]
    pub fn new(
        edges: impl IntoIterator<Item = (usize, usize)> + Clone,
        size: usize,
        include_self_loops: bool,
    ) -> Option<Self> {
        let mut transitive_closure = vec![BitSet::new(size); size];

        if include_self_loops {
            for i in 0..size {
                transitive_closure[i].set(i)?;
            }
        }

        let mut result = Self { size, closure: transitive_closure };

        let mut changed = true;
        while changed {
            changed = false;

            for (from, to) in edges.clone() {
                changed |= result.closure[from].set(to)?;

                // skip redundant `or`ing

                let Ok([left, right]) = result.closure.get_disjoint_mut([from, to]) else {
                    continue;
                };

                changed |= left.or(right)?;
            }
        }

        Some(result)
    }

    /// Checks if there is a path from the `from` vertex to the `to` vertex.
    /// index.
    #[must_use]
    pub fn has_path(&self, from: usize, to: usize) -> Option<bool> {
        self.closure.get(from)?.get(to)
    }

    /// Returns an iterator over the vertices reachable from the given vertex
    /// index.
    #[must_use]
    pub fn reachable_from(&self, from: usize) -> Option<impl Iterator<Item = usize> + '_ + Clone> {
        if from >= self.size {
            return None;
        }

        Some((0..self.size).filter(move |&to| self.closure[from].get(to).unwrap()))
    }
}

// Unit tests
#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn transitive_closure_basic() {
        let relation = TransitiveClosure::new([(0, 1), (1, 2)], 3, false).unwrap();

        // Check paths
        assert!(relation.has_path(0, 1).unwrap());
        assert!(relation.has_path(0, 2).unwrap());
        assert!(relation.has_path(1, 2).unwrap());

        // no self-loops
        assert!(!relation.has_path(0, 0).unwrap());
        assert!(!relation.has_path(1, 1).unwrap());
        assert!(!relation.has_path(2, 2).unwrap());
    }

    #[test]
    fn reachable_vertices() {
        let relation =
            TransitiveClosure::new([(0, 1), (1, 2), (2, 3), (4, 5), (4, 6)], 7, false).unwrap();

        // Check reachable vertices
        let reachable = relation.reachable_from(0).unwrap().collect::<HashSet<_>>();
        assert_eq!(reachable, [1, 2, 3].into_iter().collect());

        let reachable = relation.reachable_from(4).unwrap().collect::<HashSet<_>>();
        assert_eq!(reachable, [5, 6].into_iter().collect());

        let reachable = relation.reachable_from(5).unwrap().collect::<HashSet<_>>();
        assert_eq!(reachable, HashSet::new());

        // no self-loops
        for i in 0..7 {
            let reachable = relation.reachable_from(i).unwrap().collect::<HashSet<_>>();
            assert!(!reachable.contains(&i));
            assert!(!relation.has_path(i, i).unwrap());
        }
    }

    /// Sets the bits of `indices` in a new bitset of `size` bits.
    fn bit_set(size: usize, indices: &[usize]) -> BitSet {
        let mut bits = BitSet::new(size);
        for &index in indices {
            bits.set(index).unwrap();
        }
        bits
    }

    #[test]
    fn ones_lists_set_bits_across_words_in_order() {
        let bits = bit_set(130, &[129, 0, 64, 63]);

        assert_eq!(bits.ones().collect::<Vec<_>>(), [0, 63, 64, 129]);
        assert_eq!(bits.count_ones(), 4);
    }

    #[test]
    fn is_subset_checks_every_word() {
        let small = bit_set(130, &[1, 70]);
        let large = bit_set(130, &[1, 70, 128]);

        assert_eq!(small.is_subset(&large), Some(true));
        assert_eq!(large.is_subset(&small), Some(false));
        assert_eq!(small.is_subset(&BitSet::new(64)), None);
    }

    #[test]
    fn difference_clears_only_the_bits_of_other() {
        let mut bits = bit_set(130, &[3, 65, 129]);

        assert_eq!(bits.difference(&bit_set(130, &[65, 100])), Some(true));
        assert_eq!(bits.ones().collect::<Vec<_>>(), [3, 129]);
        assert_eq!(bits.difference(&bit_set(130, &[65])), Some(false));
    }

    #[test]
    fn complex_edge() {
        let relation = TransitiveClosure::new([(0, 1), (2, 5), (1, 2)], 6, false).unwrap();

        // Verify paths
        assert!(relation.has_path(0, 5).unwrap());
    }

    #[test]
    fn looped_relation() {
        let relation =
            TransitiveClosure::new([(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 0)], 6, false)
                .unwrap();

        let all_nodes = (0..6).collect::<HashSet<_>>();
        for i in 0..6 {
            let reachable = relation.reachable_from(i).unwrap().collect::<HashSet<_>>();

            assert_eq!(reachable, all_nodes);
            assert!(relation.has_path(i, i).unwrap());
        }
    }
}

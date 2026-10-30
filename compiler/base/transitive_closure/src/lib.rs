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
    /// Returns `true` if there was a change in the bitset.
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of bounds.
    pub fn set(&mut self, index: usize) -> bool {
        let (word, mask) = self.locate(index);

        let old = self.bits[word];
        self.bits[word] = old | mask;

        old != self.bits[word]
    }

    /// Set all bits to 0
    pub fn clear(&mut self) {
        for word in &mut self.bits {
            *word = 0;
        }
    }

    /// Set the bit at the given index to `false`
    ///
    /// Returns `true` if there was a change in the bitset.
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of bounds.
    pub fn unset(&mut self, index: usize) -> bool {
        let (word, mask) = self.locate(index);

        let old = self.bits[word];
        self.bits[word] = old & !mask;

        old != self.bits[word]
    }

    /// Get the value of the bit at the given index
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of bounds.
    #[must_use]
    pub fn get(&self, index: usize) -> bool {
        let (word, mask) = self.locate(index);

        self.bits[word] & mask != 0
    }

    /// Perform a bitwise OR operation with another bitset
    ///
    /// Returns `true` if there was a change in the bitset.
    ///
    /// # Panics
    ///
    /// Panics if the bitsets have different sizes.
    pub fn or(&mut self, other: &Self) -> bool { self.combine(other, |mine, theirs| mine | theirs) }

    /// Perform a bitwise AND operation with another bitset
    ///
    /// Returns `true` if there was a change in the bitset.
    ///
    /// # Panics
    ///
    /// Panics if the bitsets have different sizes.
    pub fn and(&mut self, other: &Self) -> bool {
        self.combine(other, |mine, theirs| mine & theirs)
    }

    /// Perform a bitwise AND NOT operation with another bitset, clearing every
    /// bit that is set in `other`
    ///
    /// Returns `true` if there was a change in the bitset.
    ///
    /// # Panics
    ///
    /// Panics if the bitsets have different sizes.
    pub fn difference(&mut self, other: &Self) -> bool {
        self.combine(other, |mine, theirs| mine & !theirs)
    }

    /// Checks if every bit set in this bitset is also set in `other`
    ///
    /// # Panics
    ///
    /// Panics if the bitsets have different sizes.
    #[must_use]
    pub fn is_subset(&self, other: &Self) -> bool {
        self.assert_same_size(other);

        self.bits.iter().zip(&other.bits).all(|(mine, theirs)| mine & !theirs == 0)
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

    /// Returns the word that holds the bit at `index`, and the mask of the bit
    /// within it.
    fn locate(&self, index: usize) -> (usize, u64) {
        assert!(index < self.size, "bit index {index} is out of bounds for {} bits", self.size);

        (index / 64, 1 << (index % 64))
    }

    /// Replaces every word of this bitset with `op` of it and the same word of
    /// `other`, and returns whether any bit changed.
    fn combine(&mut self, other: &Self, op: impl Fn(u64, u64) -> u64) -> bool {
        self.assert_same_size(other);

        let mut changed = false;
        for (mine, &theirs) in self.bits.iter_mut().zip(&other.bits) {
            let old = *mine;
            *mine = op(old, theirs);
            changed |= old != *mine;
        }

        changed
    }

    fn assert_same_size(&self, other: &Self) {
        assert_eq!(self.size, other.size, "the bitsets have different sizes");
    }
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
    ///
    /// # Panics
    ///
    /// Panics if an edge refers to a vertex that is not below `size`.
    #[allow(clippy::needless_range_loop, clippy::needless_pass_by_value)]
    #[must_use]
    pub fn new(
        edges: impl IntoIterator<Item = (usize, usize)> + Clone,
        size: usize,
        include_self_loops: bool,
    ) -> Self {
        let mut transitive_closure = vec![BitSet::new(size); size];

        if include_self_loops {
            for i in 0..size {
                transitive_closure[i].set(i);
            }
        }

        let mut result = Self { size, closure: transitive_closure };

        let mut changed = true;
        while changed {
            changed = false;

            for (from, to) in edges.clone() {
                assert!(
                    from < size && to < size,
                    "the edge ({from}, {to}) is out of bounds for {size} vertices"
                );

                changed |= result.closure[from].set(to);

                // skip redundant `or`ing

                let Ok([left, right]) = result.closure.get_disjoint_mut([from, to]) else {
                    continue;
                };

                changed |= left.or(right);
            }
        }

        result
    }

    /// Checks if there is a path from the `from` vertex to the `to` vertex.
    ///
    /// # Panics
    ///
    /// Panics if either vertex is not below [`Self::size`].
    #[must_use]
    pub fn has_path(&self, from: usize, to: usize) -> bool { self.row(from).get(to) }

    /// Returns an iterator over the vertices reachable from the given vertex
    /// index.
    ///
    /// # Panics
    ///
    /// Panics if `from` is not below [`Self::size`].
    pub fn reachable_from(&self, from: usize) -> impl Iterator<Item = usize> + '_ + Clone {
        let row = self.row(from);

        (0..self.size).filter(move |&to| row.get(to))
    }

    /// Returns the vertices reachable from `from`.
    fn row(&self, from: usize) -> &BitSet {
        assert!(from < self.size, "vertex {from} is out of bounds for {} vertices", self.size);

        &self.closure[from]
    }
}

// Unit tests
#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn transitive_closure_basic() {
        let relation = TransitiveClosure::new([(0, 1), (1, 2)], 3, false);

        // Check paths
        assert!(relation.has_path(0, 1));
        assert!(relation.has_path(0, 2));
        assert!(relation.has_path(1, 2));

        // no self-loops
        assert!(!relation.has_path(0, 0));
        assert!(!relation.has_path(1, 1));
        assert!(!relation.has_path(2, 2));
    }

    #[test]
    fn reachable_vertices() {
        let relation = TransitiveClosure::new([(0, 1), (1, 2), (2, 3), (4, 5), (4, 6)], 7, false);

        // Check reachable vertices
        let reachable = relation.reachable_from(0).collect::<HashSet<_>>();
        assert_eq!(reachable, [1, 2, 3].into_iter().collect());

        let reachable = relation.reachable_from(4).collect::<HashSet<_>>();
        assert_eq!(reachable, [5, 6].into_iter().collect());

        let reachable = relation.reachable_from(5).collect::<HashSet<_>>();
        assert_eq!(reachable, HashSet::new());

        // no self-loops
        for i in 0..7 {
            let reachable = relation.reachable_from(i).collect::<HashSet<_>>();
            assert!(!reachable.contains(&i));
            assert!(!relation.has_path(i, i));
        }
    }

    /// Sets the bits of `indices` in a new bitset of `size` bits.
    fn bit_set(size: usize, indices: &[usize]) -> BitSet {
        let mut bits = BitSet::new(size);
        for &index in indices {
            bits.set(index);
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

        assert!(small.is_subset(&large));
        assert!(!large.is_subset(&small));
    }

    #[test]
    fn difference_clears_only_the_bits_of_other() {
        let mut bits = bit_set(130, &[3, 65, 129]);

        assert!(bits.difference(&bit_set(130, &[65, 100])));
        assert_eq!(bits.ones().collect::<Vec<_>>(), [3, 129]);
        assert!(!bits.difference(&bit_set(130, &[65])));
    }

    #[test]
    #[should_panic(expected = "bit index 64 is out of bounds for 64 bits")]
    fn index_out_of_bounds_panics() { BitSet::new(64).set(64); }

    #[test]
    #[should_panic(expected = "the bitsets have different sizes")]
    fn size_mismatch_panics() { BitSet::new(130).or(&BitSet::new(64)); }

    #[test]
    fn complex_edge() {
        let relation = TransitiveClosure::new([(0, 1), (2, 5), (1, 2)], 6, false);

        // Verify paths
        assert!(relation.has_path(0, 5));
    }

    #[test]
    fn looped_relation() {
        let relation =
            TransitiveClosure::new([(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 0)], 6, false);

        let all_nodes = (0..6).collect::<HashSet<_>>();
        for i in 0..6 {
            let reachable = relation.reachable_from(i).collect::<HashSet<_>>();

            assert_eq!(reachable, all_nodes);
            assert!(relation.has_path(i, i));
        }
    }
}

use rayc_transitive_closure::BitSet;

use super::promote_conflicts;

/// Builds the conflict rows of `candidates` candidates the way the walk
/// records them: each pair in both directions, and every row with its own
/// bit.
fn rows(candidates: usize, pairs: &[(usize, usize)]) -> Vec<BitSet> {
    let mut rows = vec![BitSet::new(candidates); candidates];
    for (number, row) in rows.iter_mut().enumerate() {
        row.set(number);
    }
    for &(x, y) in pairs {
        rows[x].set(y);
        rows[y].set(x);
    }
    rows
}

/// Runs the promotion and lists the promoted numbers in ascending order.
fn promoted(rows: &[BitSet], variant_of: &[char]) -> Vec<usize> {
    promote_conflicts(rows, variant_of).ones().collect()
}

// input: 0 in variant a, 1 in variant b
// premise: no conflict
// output: nothing promoted
#[test]
fn no_conflict_promotes_nothing() {
    assert_eq!(promoted(&rows(2, &[]), &['a', 'b']), Vec::<usize>::new());
}

// input: 0 and 1 both in variant a
// premise: 0 conflicts with 1
// output: nothing promoted, they are separate fields of one variant
#[test]
fn conflict_within_one_variant_promotes_nothing() {
    assert_eq!(promoted(&rows(2, &[(0, 1)]), &['a', 'a']), Vec::<usize>::new());
}

// input: 0 in variant a, 1 in variant b
// premise: 0 conflicts with 1, one conflict each
// output: 0 promoted, the tie goes to the smaller number
#[test]
fn tie_promotes_smaller_candidate() {
    assert_eq!(promoted(&rows(2, &[(0, 1)]), &['a', 'b']), vec![0]);
}

// input: 0 in a, 1 in b, 2 in c
// premise: 2 conflicts with 0 and with 1
// output: only 2 promoted, which resolves both pairs
#[test]
fn candidate_with_more_conflicts_is_promoted() {
    assert_eq!(promoted(&rows(3, &[(0, 2), (1, 2)]), &['a', 'b', 'c']), vec![2]);
}

// input: 0 in a, 1 in b, 2 in c, 3 in d, 4 in e
// premise: pairs (0, 1), (0, 3), (2, 4), (3, 4); 0, 3 and 4 have two
//          conflicts, 1 and 2 have one
// output: 0 and 4 promoted; (3, 4) is resolved by 4 when it is visited, so 3
//         stays although it would lose the tie
#[test]
fn pair_resolved_by_earlier_promotion_is_skipped() {
    let rows = rows(5, &[(0, 1), (0, 3), (2, 4), (3, 4)]);

    assert_eq!(promoted(&rows, &['a', 'b', 'c', 'd', 'e']), vec![0, 4]);
}

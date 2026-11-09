use super::{Component, EXACT_LIMIT};

/// A component of `member_count` members with one slot per `(owner, targets)`,
/// numbered in order, each of weight 1.
fn component(member_count: usize, slots: &[(usize, &[usize])]) -> Component {
    let mut component = Component::new(member_count);
    for (owner, targets) in slots {
        component.add_slot(*owner, targets.iter().copied(), 1);
    }
    component
}

/// The set of slot numbers `boxed`, as the solver's working representation.
fn boxed_set(component: &Component, boxed: &[usize]) -> Vec<bool> {
    let mut set = vec![false; component.slots.len()];
    for slot in boxed {
        set[*slot] = true;
    }
    set
}

/// Asserts that `boxed` breaks every cycle of `component`, and that each of
/// its slots is needed: unboxing any one of them leaves a cycle.
fn assert_breaks_every_cycle_minimally(component: &Component, boxed: &[usize]) {
    let set = boxed_set(component, boxed);
    assert!(component.is_acyclic(&set), "{boxed:?} leaves a cycle");

    for slot in boxed {
        let mut without = set.clone();
        without[*slot] = false;
        assert!(!component.is_acyclic(&without), "slot {slot} of {boxed:?} is not needed");
    }
}

/// Input: one member with a slot that holds itself.
/// Premise: a self-recursive call is a cycle of one slot.
/// Output: that slot is boxed.
#[test]
fn self_holding_slot_is_boxed() {
    let component = component(1, &[(0, &[0])]);

    assert_eq!(component.break_cycles(), [0]);
}

/// Input: one member with two slots that each hold itself.
/// Premise: two recursive calls are two cycles, and a slot breaks only its
/// own.
/// Output: both slots are boxed.
#[test]
fn every_parallel_self_holding_slot_is_boxed() {
    let component = component(1, &[(0, &[0]), (0, &[0])]);

    assert_eq!(component.break_cycles(), [0, 1]);
}

/// Input: two members that each hold the other.
/// Premise: either slot alone breaks the only cycle.
/// Output: exactly one slot is boxed.
#[test]
fn two_cycle_boxes_one_slot() {
    let component = component(2, &[(0, &[1]), (1, &[0])]);

    let boxed = component.break_cycles();

    assert_eq!(boxed.len(), 1);
    assert_breaks_every_cycle_minimally(&component, &boxed);
}

/// Input: the cycle 0 → 1 → 2 → 0, and the chord 0 → 2.
/// Premise: the slot 2 → 0 is on both cycles, 0 → 1 → 2 → 0 and 0 → 2 → 0.
/// Output: only the slot 2 → 0 is boxed.
#[test]
fn three_cycle_with_chord_boxes_the_slot_on_both_cycles() {
    let component = component(3, &[(0, &[1]), (1, &[2]), (2, &[0]), (0, &[2])]);

    assert_eq!(component.break_cycles(), [2]);
}

/// Input: member 0 holds members 1 and 2 in one slot, as a `run ... with`
/// holds its body and its clause, and each of them holds member 0.
/// Premise: boxing the one slot of member 0 breaks both cycles at once.
/// Output: only that slot is boxed, rather than the two slots that hold
/// member 0.
#[test]
fn slot_with_several_targets_is_boxed_once_for_all() {
    let component = component(3, &[(0, &[1, 2]), (1, &[0]), (2, &[0])]);

    assert_eq!(component.break_cycles(), [0]);
}

/// Input: two members that each hold the other, where boxing 0 → 1 costs 1
/// and boxing 1 → 0 costs 5.
/// Premise: either slot breaks the cycle.
/// Output: the lighter slot, 0 → 1, is boxed.
#[test]
fn lighter_slot_is_boxed() {
    let mut component = Component::new(2);
    component.add_slot(0, [1], 1);
    component.add_slot(1, [0], 5);

    assert_eq!(component.break_cycles(), [0]);
}

/// Input: a ring of more members than the exact solver takes, with a chord
/// from every even member to the member two ahead.
/// Premise: the greedy solver is used.
/// Output: every cycle is broken, and no boxed slot is needed by none.
#[test]
fn greedy_choice_above_the_exact_limit_breaks_every_cycle_minimally() {
    let member_count = EXACT_LIMIT + 4;
    let mut component = Component::new(member_count);
    for member in 0..member_count {
        component.add_slot(member, [(member + 1) % member_count], 1);
        if member % 2 == 0 {
            component.add_slot(member, [(member + 2) % member_count], 1);
        }
    }

    let boxed = component.break_cycles();

    assert_breaks_every_cycle_minimally(&component, &boxed);
}

/// Input: many small components, generated from a fixed seed, with slots of
/// one to three targets and weights of one to three.
/// Premise: trying every set of slots finds the lightest one that breaks
/// every cycle.
/// Output: the exact solver's choice breaks every cycle and weighs as little
/// as that lightest set. The greedy solver's choice breaks every cycle with no
/// slot that is not needed.
#[test]
fn exact_choice_weighs_as_little_as_any_choice() {
    let mut random = Random(0x5eed);

    for _ in 0..500 {
        let member_count = 1 + random.below(5);
        let slot_count = 1 + random.below(9);
        let mut component = Component::new(member_count);
        for _ in 0..slot_count {
            let owner = random.below(member_count);
            let targets = (0..=random.below(3)).map(|_| random.below(member_count));
            component.add_slot(owner, targets.collect::<Vec<_>>(), 1 + random.below(3) as u64);
        }

        let exact = component.break_cycles_exactly();
        assert!(component.is_acyclic(&boxed_set(&component, &exact)));
        assert_eq!(weight_of(&component, &exact), lightest_weight_by_trying_all(&component));

        let greedy = component.break_cycles_greedily();
        assert_breaks_every_cycle_minimally(&component, &greedy);
    }
}

/// The weight of the slots `boxed`.
fn weight_of(component: &Component, boxed: &[usize]) -> u64 {
    boxed.iter().map(|slot| component.slots[*slot].weight).sum()
}

/// The lightest weight of a set of slots that breaks every cycle, found by
/// trying every set.
fn lightest_weight_by_trying_all(component: &Component) -> u64 {
    let slot_count = component.slots.len();
    (0..1_u32 << slot_count)
        .map(|set| (0..slot_count).filter(|slot| set & (1 << slot) != 0).collect::<Vec<_>>())
        .filter(|boxed| component.is_acyclic(&boxed_set(component, boxed)))
        .map(|boxed| weight_of(component, &boxed))
        .min()
        .expect("boxing every slot breaks every cycle")
}

/// A small deterministic generator, so the generated cases are the same on
/// every run.
struct Random(u64);

impl Random {
    /// A number in `0..bound`.
    fn below(&mut self, bound: usize) -> usize {
        // the constants of Knuth's MMIX linear congruential generator
        self.0 =
            self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        usize::try_from((self.0 >> 33) % bound as u64).expect("the number is below a usize")
    }
}

//! Chooses the slots to box in one strongly connected component of the frame
//! graph: a set of slots that leaves no cycle once they are removed, with the
//! lowest total weight among such sets.
//!
//! This is the minimum feedback arc set problem, which is NP-hard in general.
//! The components of real programs have a few members, so a component of up to
//! [`EXACT_LIMIT`] members is solved exactly. A larger one is solved greedily.
//!
//! The solver knows nothing of coroutines. The members of the component are
//! numbered `0..n`, and the slots are numbered in the order they are added.
//! The answer depends only on these numbers, so the caller makes it
//! deterministic by numbering in a deterministic order.

use std::cmp::Reverse;

/// The most members that a component may have to be solved exactly.
///
/// The exact solver takes `O(2ⁿ · n · slots)` time and `O(2ⁿ)` memory.
const EXACT_LIMIT: usize = 16;

/// A slot of a member, which holds the frames of its targets.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Slot {
    owner: usize,
    /// The targets inside the component. A target outside it is on no cycle.
    targets: Vec<usize>,
    /// How much boxing the slot costs.
    weight: u64,
}

/// One strongly connected component of the frame graph, as the solver sees
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Component {
    member_count: usize,
    slots: Vec<Slot>,
}

impl Component {
    /// Creates a component of `member_count` members without slots.
    pub(super) const fn new(member_count: usize) -> Self {
        Self { member_count, slots: Vec::new() }
    }

    /// Adds a slot of `owner` that holds the frames of `targets`, whose boxing
    /// costs `weight`, and returns its number.
    ///
    /// # Panics
    ///
    /// Panics if the owner or a target is not a member.
    pub(super) fn add_slot(
        &mut self,
        owner: usize,
        targets: impl IntoIterator<Item = usize>,
        weight: u64,
    ) -> usize {
        let targets = targets.into_iter().collect::<Vec<_>>();
        assert!(
            std::iter::once(owner).chain(targets.iter().copied()).all(|m| m < self.member_count),
            "a slot connects members of the component"
        );

        self.slots.push(Slot { owner, targets, weight });
        self.slots.len() - 1
    }

    /// Chooses the slots to box, and returns their numbers in increasing
    /// order.
    ///
    /// Every cycle passes through a chosen slot. Up to [`EXACT_LIMIT`]
    /// members, no other such choice weighs less.
    pub(super) fn break_cycles(&self) -> Vec<usize> {
        if self.member_count <= EXACT_LIMIT {
            self.break_cycles_exactly()
        } else {
            self.break_cycles_greedily()
        }
    }

    /// Finds the cheapest choice.
    ///
    /// A choice breaks every cycle exactly when some order of the members puts
    /// the targets of every unchosen slot after its owner. So the cheapest
    /// choice is that of the cheapest order, where an order costs the weight
    /// of its backward slots: those with a target at or before their owner.
    ///
    /// The cost of a member depends only on the set of members placed up to
    /// it, so a dynamic program over those sets finds the cheapest order.
    fn break_cycles_exactly(&self) -> Vec<usize> {
        // the targets of each slot as a set of members
        let target_sets = self
            .slots
            .iter()
            .map(|slot| slot.targets.iter().fold(0_u32, |set, target| set | (1 << target)))
            .collect::<Vec<_>>();
        let slots_of = self.slots_by_owner();
        let (target_sets, slots_of) = (&target_sets, &slots_of);
        let backward_slots = |member: usize, placed: u32| {
            slots_of[member].iter().copied().filter(move |slot| target_sets[*slot] & placed != 0)
        };

        // `cost[placed]` is the cheapest order of the members of `placed`, which
        // is reached by placing `last[placed]` after the others
        let all = (1_u32 << self.member_count) - 1;
        let mut cost = vec![u64::MAX; (all as usize) + 1];
        let mut last = vec![0; (all as usize) + 1];
        cost[0] = 0;
        for placed in 0..=all {
            let base = cost[placed as usize];
            if base == u64::MAX {
                continue;
            }

            for member in (0..self.member_count).filter(|member| placed & (1 << member) == 0) {
                let next = placed | (1 << member);
                let added = backward_slots(member, next).map(|slot| self.slots[slot].weight);
                let next_cost = base + added.sum::<u64>();

                // the first cheapest order is kept, so the choice is
                // deterministic
                if next_cost < cost[next as usize] {
                    cost[next as usize] = next_cost;
                    last[next as usize] = member;
                }
            }
        }

        // walk the cheapest order backward, boxing the backward slots
        let mut boxed = Vec::new();
        let mut placed = all;
        while placed != 0 {
            let member = last[placed as usize];
            boxed.extend(backward_slots(member, placed));
            placed &= !(1 << member);
        }

        boxed.sort_unstable();
        boxed
    }

    /// Finds a choice that is not always the cheapest, in polynomial time.
    ///
    /// Slots are boxed one at a time until no cycle is left. Then every boxed
    /// slot that a later choice made unnecessary is unboxed.
    fn break_cycles_greedily(&self) -> Vec<usize> {
        let mut boxed = vec![false; self.slots.len()];
        let mut boxing_order = Vec::new();

        // box a slot of the cyclic component with the smallest member, until
        // none is left
        while let Some(component) = self
            .components(&boxed)
            .into_iter()
            .filter(|component| self.is_cyclic(component, &boxed))
            .min_by_key(|component| component[0])
        {
            let slot = self.cheapest_slot_inside(&component, &boxed);
            boxed[slot] = true;
            boxing_order.push(slot);
        }

        // unbox, latest first, every slot whose cycles are all broken by
        // another
        for slot in boxing_order.into_iter().rev() {
            boxed[slot] = false;
            if !self.is_acyclic(&boxed) {
                boxed[slot] = true;
            }
        }

        (0..self.slots.len()).filter(|slot| boxed[*slot]).collect()
    }

    /// The unboxed slot of `component` that the greedy solver boxes next: the
    /// lightest, then the one with the most targets inside the component, then
    /// the first.
    fn cheapest_slot_inside(&self, component: &[usize], boxed: &[bool]) -> usize {
        let mut inside = vec![false; self.member_count];
        for member in component {
            inside[*member] = true;
        }

        (0..self.slots.len())
            .filter(|slot| !boxed[*slot] && inside[self.slots[*slot].owner])
            .map(|slot| {
                let targets_inside =
                    self.slots[slot].targets.iter().filter(|target| inside[**target]).count();
                (slot, targets_inside)
            })
            .filter(|(_, targets_inside)| *targets_inside != 0)
            .min_by_key(|(slot, targets_inside)| {
                (self.slots[*slot].weight, Reverse(*targets_inside), *slot)
            })
            .map(|(slot, _)| slot)
            .expect("a cyclic component has a slot between its members")
    }

    /// Whether the component of the graph without the boxed slots is on a
    /// cycle: it has several members, or its member holds itself.
    fn is_cyclic(&self, component: &[usize], boxed: &[bool]) -> bool {
        match component {
            [member] => self.slots.iter().zip(boxed).any(|(slot, boxed)| {
                !boxed && slot.owner == *member && slot.targets.contains(member)
            }),
            _ => true,
        }
    }

    /// Whether no cycle is left once the boxed slots are removed.
    ///
    /// Members are removed while one has no unremoved predecessor. A cycle
    /// keeps all of its members, a slot that holds its owner included.
    fn is_acyclic(&self, boxed: &[bool]) -> bool {
        let successors = self.successors(boxed);
        let mut predecessor_count = vec![0_usize; self.member_count];
        for target in successors.iter().flatten() {
            predecessor_count[*target] += 1;
        }

        let mut ready = (0..self.member_count)
            .filter(|member| predecessor_count[*member] == 0)
            .collect::<Vec<_>>();
        let mut removed = 0;
        while let Some(member) = ready.pop() {
            removed += 1;
            for target in &successors[member] {
                predecessor_count[*target] -= 1;
                if predecessor_count[*target] == 0 {
                    ready.push(*target);
                }
            }
        }

        removed == self.member_count
    }

    /// The strongly connected components of the graph without the boxed
    /// slots, each with its members in increasing order.
    ///
    /// This is Tarjan's algorithm, with an explicit stack instead of
    /// recursion, so that a large component cannot overflow the call stack.
    fn components(&self, boxed: &[bool]) -> Vec<Vec<usize>> {
        let successors = self.successors(boxed);
        let mut index = vec![None; self.member_count];
        let mut low_link = vec![0; self.member_count];
        let mut on_stack = vec![false; self.member_count];
        let mut stack = Vec::new();
        let mut next_index = 0;
        let mut components = Vec::new();

        for root in 0..self.member_count {
            if index[root].is_some() {
                continue;
            }

            // each frame of the walk is a member, with the position of the
            // next successor to visit
            let mut walk = vec![(root, 0)];
            index[root] = Some(next_index);
            low_link[root] = next_index;
            next_index += 1;
            stack.push(root);
            on_stack[root] = true;

            while let Some(&(member, position)) = walk.last() {
                // visit the next successor
                if let Some(&successor) = successors[member].get(position) {
                    walk.last_mut().expect("the walk is not empty").1 += 1;
                    match index[successor] {
                        None => {
                            index[successor] = Some(next_index);
                            low_link[successor] = next_index;
                            next_index += 1;
                            stack.push(successor);
                            on_stack[successor] = true;
                            walk.push((successor, 0));
                        }
                        Some(successor_index) if on_stack[successor] => {
                            low_link[member] = low_link[member].min(successor_index);
                        }
                        Some(_) => {}
                    }
                    continue;
                }

                // every successor is visited: return to the parent
                walk.pop();
                if let Some(&(parent, _)) = walk.last() {
                    low_link[parent] = low_link[parent].min(low_link[member]);
                }

                // a member that reaches nothing earlier starts a component
                if Some(low_link[member]) == index[member] {
                    let mut component = Vec::new();
                    loop {
                        let popped = stack.pop().expect("the component is on the stack");
                        on_stack[popped] = false;
                        component.push(popped);
                        if popped == member {
                            break;
                        }
                    }
                    component.sort_unstable();
                    components.push(component);
                }
            }
        }

        components
    }

    /// The successors of every member through the unboxed slots, once for
    /// each slot that holds them.
    fn successors(&self, boxed: &[bool]) -> Vec<Vec<usize>> {
        let mut successors = vec![Vec::new(); self.member_count];
        for (slot, boxed) in self.slots.iter().zip(boxed) {
            if !boxed {
                successors[slot.owner].extend(slot.targets.iter().copied());
            }
        }
        successors
    }

    /// The numbers of the slots of every member.
    fn slots_by_owner(&self) -> Vec<Vec<usize>> {
        let mut slots_of = vec![Vec::new(); self.member_count];
        for (number, slot) in self.slots.iter().enumerate() {
            slots_of[slot.owner].push(number);
        }
        slots_of
    }
}

#[cfg(test)]
mod tests;

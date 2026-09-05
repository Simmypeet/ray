use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{poly_var::GlobalPolyVarID, trait_ref::TraitRef, ty::Ty};

use super::InstanceResolutionError;

/// The initial maximum nesting of canonical goals in one root search.
pub const DEFAULT_MAX_DEPTH: usize = 64;

/// The initial maximum number of global candidates tried in one root search.
pub const DEFAULT_MAX_CANDIDATE_VISITS: usize = 4096;

/// Limits that guarantee termination of instance resolution.
///
/// These limits are the current general termination policy. Future versions
/// may replace or supplement them with size-change checks, tabled resolution,
/// or declaration restrictions such as Paterson conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceResolutionLimits {
    max_depth: usize,
    max_candidate_visits: usize,
}

impl InstanceResolutionLimits {
    #[must_use]
    pub const fn new(max_depth: usize, max_candidate_visits: usize) -> Self {
        Self { max_depth, max_candidate_visits }
    }
}

impl Default for InstanceResolutionLimits {
    fn default() -> Self { Self::new(DEFAULT_MAX_DEPTH, DEFAULT_MAX_CANDIDATE_VISITS) }
}

/// The candidate premise that caused a recursive goal to be entered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceResolutionEdge {
    instance_id: GlobalSymbolID,
    given_parameter: GlobalPolyVarID,
}

impl InstanceResolutionEdge {
    #[must_use]
    pub const fn new(instance_id: GlobalSymbolID, given_parameter: GlobalPolyVarID) -> Self {
        Self { instance_id, given_parameter }
    }

    #[must_use]
    pub const fn instance_id(&self) -> GlobalSymbolID { self.instance_id }

    #[must_use]
    pub const fn given_parameter(&self) -> GlobalPolyVarID { self.given_parameter }
}

/// One canonical goal in a diagnostic search trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceResolutionFrame {
    goal: TraitRef,
    introduced_by: Option<InstanceResolutionEdge>,
}

impl InstanceResolutionFrame {
    #[must_use]
    pub const fn goal(&self) -> &TraitRef { &self.goal }

    #[must_use]
    pub const fn introduced_by(&self) -> Option<InstanceResolutionEdge> { self.introduced_by }
}

/// An exact cycle, including the repeated goal as the final frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceResolutionCycle {
    path: Vec<InstanceResolutionFrame>,
}

impl InstanceResolutionCycle {
    #[must_use]
    pub fn path(&self) -> &[InstanceResolutionFrame] { &self.path }
}

/// The hard limit that stopped a search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceResolutionLimit {
    Depth { limit: usize },
    CandidateVisits { limit: usize, candidate: GlobalSymbolID },
}

/// A state-level failure produced before or while trying a candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceResolutionStateError {
    Cycle(InstanceResolutionCycle),
    Limit { limit: InstanceResolutionLimit, recent_goals: Vec<InstanceResolutionFrame> },
}

/// A complete instance-resolution result eligible for memoization.
pub type InstanceResolutionResult = Result<Interned<Ty>, InstanceResolutionError>;

/// The result of attempting to enter a canonical goal.
#[derive(Debug, PartialEq, Eq)]
pub enum EnteredInstanceGoal {
    Memoized(InstanceResolutionResult),
    Active(ActiveInstanceGoal),
}

/// A token proving that a goal was pushed onto the active stack.
#[derive(Debug, PartialEq, Eq)]
pub struct ActiveInstanceGoal {
    depth: usize,
    goal: TraitRef,
}

#[derive(Debug)]
struct ActiveGoalFrame {
    diagnostic: InstanceResolutionFrame,
    memo: bool,
}

/// State shared by the complete recursive proof attempt for one body.
#[derive(Debug)]
pub struct InstanceResolutionState {
    limits: InstanceResolutionLimits,
    remaining_candidate_visits: usize,
    active_goals: Vec<ActiveGoalFrame>,
    memo: FxHashMap<TraitRef, InstanceResolutionResult>,
}

impl InstanceResolutionState {
    #[must_use]
    pub fn new(limits: InstanceResolutionLimits) -> Self {
        Self {
            limits,
            remaining_candidate_visits: limits.max_candidate_visits,
            active_goals: Vec::new(),
            memo: FxHashMap::default(),
        }
    }

    /// Returns a cached completion, diagnoses an exact cycle, or pushes the
    /// canonical goal onto the active stack.
    pub fn enter_goal(
        &mut self,
        goal: TraitRef,
        introduced_by: Option<InstanceResolutionEdge>,
    ) -> Result<EnteredInstanceGoal, InstanceResolutionStateError> {
        // retrieve the existing cached result
        if let Some(resolution) = self.memo.get(&goal) {
            return Ok(EnteredInstanceGoal::Memoized(resolution.clone()));
        }

        // detect the cycles
        if let Some(cycle_start) =
            self.active_goals.iter().position(|frame| frame.diagnostic.goal == goal)
        {
            // all the resolution frames between the cycle will be marked as non-memoizable,
            // since the cycle affects the resolution of all of them
            for frame in &mut self.active_goals[cycle_start + 1..] {
                frame.memo = false;
            }

            let mut path = self.active_goals[cycle_start..]
                .iter()
                .map(|frame| frame.diagnostic.clone())
                .collect::<Vec<_>>();
            path.push(InstanceResolutionFrame { goal, introduced_by });
            return Err(InstanceResolutionStateError::Cycle(InstanceResolutionCycle { path }));
        }

        // checks if we exhausted the depth limit, and if so, mark all active goals as
        // non-memoizable
        if self.active_goals.len() >= self.limits.max_depth {
            for frame in &mut self.active_goals {
                frame.memo = false;
            }
            let mut recent_goals =
                self.active_goals.iter().map(|frame| frame.diagnostic.clone()).collect::<Vec<_>>();
            recent_goals.push(InstanceResolutionFrame { goal, introduced_by });

            return Err(InstanceResolutionStateError::Limit {
                limit: InstanceResolutionLimit::Depth { limit: self.limits.max_depth },
                recent_goals,
            });
        }

        // resets the fuel if this is the root goal, so that we can resolve another goal
        // with a fresh budget of candidate visits
        if self.active_goals.is_empty() {
            self.remaining_candidate_visits = self.limits.max_candidate_visits;
        }

        let active = ActiveInstanceGoal { depth: self.active_goals.len(), goal: goal.clone() };
        self.active_goals.push(ActiveGoalFrame {
            diagnostic: InstanceResolutionFrame { goal, introduced_by },
            memo: true,
        });

        Ok(EnteredInstanceGoal::Active(active))
    }

    /// Consumes one unit of shared candidate fuel before a global head is
    /// tried.
    pub fn visit_candidate(
        &mut self,
        candidate: GlobalSymbolID,
    ) -> Result<(), InstanceResolutionStateError> {
        assert!(!self.active_goals.is_empty(), "candidate visits require an active goal");
        if self.remaining_candidate_visits == 0 {
            for frame in &mut self.active_goals {
                frame.memo = false;
            }
            return Err(InstanceResolutionStateError::Limit {
                limit: InstanceResolutionLimit::CandidateVisits {
                    limit: self.limits.max_candidate_visits,
                    candidate,
                },
                recent_goals: self
                    .active_goals
                    .iter()
                    .map(|frame| frame.diagnostic.clone())
                    .collect(),
            });
        }

        self.remaining_candidate_visits -= 1;
        Ok(())
    }

    /// Pops a goal and stores its completion if the active search remained
    /// memoizable.
    pub fn complete_goal(
        &mut self,
        active: ActiveInstanceGoal,
        resolution: InstanceResolutionResult,
    ) {
        let ActiveInstanceGoal { depth, goal } = active;
        let frame = self.pop_goal(depth, &goal);
        if frame.memo {
            self.memo.insert(goal, resolution);
        }
    }

    /// Pops a goal whose result must not be memoized.
    pub fn leave_goal(&mut self, active: ActiveInstanceGoal) {
        let ActiveInstanceGoal { depth, goal } = active;
        self.pop_goal(depth, &goal);
    }

    fn pop_goal(&mut self, depth: usize, goal: &TraitRef) -> ActiveGoalFrame {
        assert_eq!(depth + 1, self.active_goals.len(), "instance goals must leave in stack order");
        let frame = self.active_goals.pop().expect("an active instance goal must be present");
        assert_eq!(
            frame.diagnostic.goal, *goal,
            "the active instance goal token must match the stack"
        );
        frame
    }
}

impl Default for InstanceResolutionState {
    fn default() -> Self { Self::new(InstanceResolutionLimits::default()) }
}

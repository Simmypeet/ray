use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{reduce::Reduce, trait_ref::TraitRef, ty::Ty};

use crate::{
    Solver,
    instance_resolution::{
        InstanceResolutionCycle, InstanceResolutionEdge, InstanceResolutionError,
        InstanceResolutionFrame, InstanceResolutionLimit, InstanceResolutionResult,
    },
};

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

/// The result of attempting to enter a canonical goal.
#[derive(Debug, PartialEq, Eq)]
enum EnteredInstanceGoal {
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
pub(crate) struct InstanceResolutionState {
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
    fn enter_goal(
        &mut self,
        goal: TraitRef,
        introduced_by: Option<InstanceResolutionEdge>,
    ) -> Result<EnteredInstanceGoal, InstanceResolutionError> {
        // retrieve the existing cached result
        if let Some(resolution) = self.memo.get(&goal) {
            return Ok(EnteredInstanceGoal::Memoized(resolution.clone()));
        }

        // detect the cycles
        if let Some(cycle_start) =
            self.active_goals.iter().position(|frame| *frame.diagnostic.goal() == goal)
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
            path.push(InstanceResolutionFrame::new(goal, introduced_by));

            return Err(InstanceResolutionError::Cycle(InstanceResolutionCycle::new(path)));
        }

        // checks if we exhausted the depth limit, and if so, mark all active goals as
        // non-memoizable
        if self.active_goals.len() >= self.limits.max_depth {
            for frame in &mut self.active_goals {
                frame.memo = false;
            }
            let mut recent_goals =
                self.active_goals.iter().map(|frame| frame.diagnostic.clone()).collect::<Vec<_>>();

            recent_goals.push(InstanceResolutionFrame::new(goal, introduced_by));

            return Err(InstanceResolutionError::Limit {
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
            diagnostic: InstanceResolutionFrame::new(goal, introduced_by),
            memo: true,
        });

        Ok(EnteredInstanceGoal::Active(active))
    }

    /// Consumes one unit of shared candidate fuel before a global head is
    /// tried.
    fn visit_candidate(
        &mut self,
        candidate: GlobalSymbolID,
    ) -> Result<(), InstanceResolutionError> {
        assert!(!self.active_goals.is_empty(), "candidate visits require an active goal");
        if self.remaining_candidate_visits == 0 {
            for frame in &mut self.active_goals {
                frame.memo = false;
            }
            return Err(InstanceResolutionError::Limit {
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
    fn complete_goal(&mut self, active: ActiveInstanceGoal, resolution: InstanceResolutionResult) {
        let ActiveInstanceGoal { depth, goal } = active;
        let frame = self.pop_goal(depth, &goal);
        if frame.memo {
            self.memo.insert(goal, resolution);
        }
    }

    fn pop_goal(&mut self, depth: usize, goal: &TraitRef) -> ActiveGoalFrame {
        assert_eq!(depth + 1, self.active_goals.len(), "instance goals must leave in stack order");
        let frame = self.active_goals.pop().expect("an active instance goal must be present");
        assert_eq!(
            frame.diagnostic.goal(),
            goal,
            "the active instance goal token must match the stack"
        );
        frame
    }
}

impl Solver {
    pub(crate) async fn resolve_instance_from(
        &mut self,
        required: TraitRef,
        introduced_by: Option<InstanceResolutionEdge>,
    ) -> Result<Interned<Ty>, InstanceResolutionError> {
        let required = required.normalize(self.engine()).await;
        if required.contains_inference() {
            return Err(InstanceResolutionError::NotReady(required));
        }
        if required.contains_error() {
            return Err(InstanceResolutionError::ContainsError(required));
        }

        let active = match self.enter_instance_goal(required.clone(), introduced_by)? {
            EnteredInstanceGoal::Memoized(result) => return result,
            EnteredInstanceGoal::Active(active) => active,
        };

        let result = self.search_active_goal(&required).await;
        self.complete_instance_goal(active, result.clone());
        result
    }

    fn enter_instance_goal(
        &mut self,
        goal: TraitRef,
        introduced_by: Option<InstanceResolutionEdge>,
    ) -> Result<EnteredInstanceGoal, InstanceResolutionError> {
        self.instance_resolution.enter_goal(goal, introduced_by)
    }

    fn complete_instance_goal(
        &mut self,
        active: ActiveInstanceGoal,
        result: InstanceResolutionResult,
    ) {
        self.instance_resolution.complete_goal(active, result);
    }

    pub(crate) fn visit_instance_candidate(
        &mut self,
        candidate: GlobalSymbolID,
    ) -> Result<(), InstanceResolutionError> {
        self.instance_resolution.visit_candidate(candidate)
    }
}

impl Default for InstanceResolutionState {
    fn default() -> Self { Self::new(InstanceResolutionLimits::default()) }
}

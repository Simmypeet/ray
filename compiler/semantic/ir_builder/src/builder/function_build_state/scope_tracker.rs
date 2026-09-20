use rayc_ir::scope::ScopeID;

use super::super::Builder;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeKind {
    Lexical,
    Temporary,
}

#[derive(Clone, Copy)]
struct ActiveScope {
    id: ScopeID,
    kind: ScopeKind,
}

/// Tracks the scopes that are active at the current lowering position.
pub(super) struct ScopeTracker {
    active: Vec<ActiveScope>,
}

impl ScopeTracker {
    pub(super) fn new(root: ScopeID) -> Self {
        Self { active: vec![ActiveScope { id: root, kind: ScopeKind::Lexical }] }
    }

    pub(super) const fn depth(&self) -> usize { self.active.len() }

    pub(super) fn current(&self) -> ScopeID {
        self.active.last().expect("an IR scope should always be active").id
    }

    pub(super) fn current_lexical(&self) -> ScopeID {
        self.active
            .iter()
            .rev()
            .find(|scope| scope.kind == ScopeKind::Lexical)
            .expect("a lexical IR scope should always be active")
            .id
    }

    pub(super) fn push(&mut self, id: ScopeID, kind: ScopeKind) {
        self.active.push(ActiveScope { id, kind });
    }

    pub(super) fn pop(&mut self) -> ScopeID {
        assert!(self.active.len() > 1, "the root IR scope should remain active while lowering");
        self.active.pop().expect("a nested IR scope should be active").id
    }

    fn scopes_from(&self, depth: usize) -> impl DoubleEndedIterator<Item = ScopeID> + '_ {
        assert!(depth <= self.active.len(), "scope unwind depth should be active");
        self.active[depth..].iter().map(|scope| scope.id)
    }
}

impl Builder {
    pub(crate) fn unwind_scopes_from(&mut self, depth: usize) {
        let function_id = self.building_function.ir_function_id;
        let block_id = self.building_function.current_block;
        let scopes = self.building_function.scopes.scopes_from(depth).rev();
        let ir_functions = &mut self.ir_functions;

        for scope_id in scopes {
            ir_functions.push_scope_pop_instruction(function_id, block_id, scope_id);
        }
    }

    pub(crate) fn unwind_all_scopes(&mut self) { self.unwind_scopes_from(0); }
}

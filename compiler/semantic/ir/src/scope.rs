use qbice::{Decode, Encode, StableHash};
use rayc_arena::{Arena, ID};

use crate::ir_variable::IRVariableID;

/// A group of scopes introduced at the same point in the enclosing scope.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub enum ScopeChild {
    One(ScopeID),
    Branch(Vec<ScopeID>),
}

/// A lexical scope in lowered IR.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct Scope {
    children: Vec<ScopeChild>,
    declared_variables: Vec<IRVariableID>,
}

impl Scope {
    #[must_use]
    const fn new() -> Self { Self { children: Vec::new(), declared_variables: Vec::new() } }

    /// Returns child scopes in the order in which they were introduced.
    #[must_use]
    pub fn children(&self) -> &[ScopeChild] { &self.children }

    /// Iterates over variables declared directly in this scope.
    #[must_use]
    pub fn declared_variables(&self) -> impl ExactSizeIterator<Item = IRVariableID> + '_ {
        self.declared_variables.iter().copied()
    }
}

pub type ScopeID = ID<Scope>;

/// The lexical scopes belonging to one IR function.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct ScopeMap {
    scopes: Arena<Scope>,
    root: ScopeID,
}

impl Default for ScopeMap {
    fn default() -> Self { Self::new() }
}

impl ScopeMap {
    #[must_use]
    pub fn new() -> Self {
        let mut scopes = Arena::default();
        let root = scopes.insert(Scope::new());

        Self { scopes, root }
    }

    #[must_use]
    pub const fn root_id(&self) -> ScopeID { self.root }

    #[must_use]
    pub fn get_scope(&self, id: ScopeID) -> &Scope {
        self.scopes.get(id).expect("IR scope should exist")
    }

    /// Adds one scope as the next child of `parent`.
    #[must_use]
    pub fn insert_scope(&mut self, parent: ScopeID) -> ScopeID {
        let _ = self.get_scope(parent);
        let child = self.scopes.insert(Scope::new());
        self.get_scope_mut(parent).children.push(ScopeChild::One(child));
        child
    }

    /// Adds mutually exclusive scopes as the next children of `parent`.
    #[must_use]
    pub fn insert_branch(&mut self, parent: ScopeID, branch_count: usize) -> Vec<ScopeID> {
        let _ = self.get_scope(parent);
        assert!(branch_count > 0, "an IR scope branch should contain at least one scope");

        let children: Vec<ScopeID> =
            (0..branch_count).map(|_| self.scopes.insert(Scope::new())).collect();
        self.get_scope_mut(parent).children.push(ScopeChild::Branch(children.clone()));
        children
    }

    pub(crate) fn register_variable(&mut self, scope_id: ScopeID, variable_id: IRVariableID) {
        self.get_scope_mut(scope_id).declared_variables.push(variable_id);
    }

    fn get_scope_mut(&mut self, id: ScopeID) -> &mut Scope {
        self.scopes.get_mut(id).expect("IR scope should exist")
    }
}

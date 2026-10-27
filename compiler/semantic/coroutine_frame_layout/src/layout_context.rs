use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_type::{
    subst::{Subst, Substitutable},
    ty::Ty,
};

/// Stores the state required to compute the coroutine frame layout
#[derive(Debug)]
pub struct LayoutContext {
    subst: Subst,
    engine: TrackedEngine,
    solver: Solver,
}

impl LayoutContext {
    #[must_use]
    pub async fn latest_type(&mut self, ty: &Interned<Ty>) -> Interned<Ty> {
        let substed = ty.apply_subst_or_clone(&self.subst, &self.engine);
        self.solver.normalize(&substed).await
    }

    #[must_use]
    pub const fn engine(&self) -> &TrackedEngine { &self.engine }
}

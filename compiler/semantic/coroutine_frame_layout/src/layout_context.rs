use qbice::storage::intern::Interned;
use rayc_ir::{
    address::Local,
    ir_function::{IRFunction, IRFunctionID},
};
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    subst::{Subst, Substitutable},
    ty::Ty,
};

/// Stores the state required to compute the coroutine frame layout
#[derive(Debug)]
pub struct LayoutContext<'a> {
    subst: Subst,
    engine: TrackedEngine,

    def_id: GlobalSymbolID,
    local_ir_function_id: IRFunctionID,

    ir_function: &'a IRFunction,
}

impl<'a> LayoutContext<'a> {
    #[must_use]
    pub async fn latest_type(&self, ty: &Interned<Ty>, solver: &mut Solver) -> Interned<Ty> {
        let substed = ty.apply_subst_or_clone(&self.subst, &self.engine);
        solver.normalize(&substed).await
    }

    #[must_use]
    pub const fn engine(&self) -> &TrackedEngine { &self.engine }

    #[must_use]
    pub async fn parameter_locals(&self) -> Vec<Local> {
        self.ir_function.parameter_locals(self.def_id, &self.engine).await
    }

    pub const fn ir_function(&self) -> &'a IRFunction { self.ir_function }
}

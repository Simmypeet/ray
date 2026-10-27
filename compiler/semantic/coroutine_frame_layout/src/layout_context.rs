use qbice::storage::intern::Interned;
use rayc_ir::{
    address::Local,
    borrowed_locals::MaybeBorrowedLocals,
    cfg::BlockID,
    ir_function::IRFunction,
    liveness::{expr::ExprLiveness, local::LocalLiveness},
};
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    subst::{Subst, Substitutable},
    ty::Ty,
};

use crate::savable::Savable;

/// Stores the state required to compute the coroutine frame layout
#[derive(Debug)]
pub struct LayoutContext<'a> {
    subst: Subst,
    engine: TrackedEngine,

    def_id: GlobalSymbolID,

    local_liveness: LocalLiveness,
    expr_liveness: ExprLiveness,
    maybe_borrowed_locals: MaybeBorrowedLocals,

    ir_function: &'a IRFunction,
}

impl<'a> LayoutContext<'a> {
    #[must_use]
    pub async fn new(
        subst: Subst,
        engine: TrackedEngine,
        def_id: GlobalSymbolID,
        ir_function: &'a IRFunction,
    ) -> Self {
        let local_liveness = LocalLiveness::compute(ir_function).await;
        let expr_liveness = ExprLiveness::compute(ir_function).await;
        let maybe_borrowed_locals = MaybeBorrowedLocals::compute(ir_function).await;

        Self {
            subst,
            engine,
            def_id,
            local_liveness,
            expr_liveness,
            maybe_borrowed_locals,
            ir_function,
        }
    }

    #[must_use]
    pub(crate) async fn latest_type(&self, ty: &Interned<Ty>, solver: &mut Solver) -> Interned<Ty> {
        let substed = ty.apply_subst_or_clone(&self.subst, &self.engine);
        solver.normalize(&substed).await
    }

    #[must_use]
    pub(crate) const fn engine(&self) -> &TrackedEngine { &self.engine }

    #[must_use]
    pub(crate) async fn parameter_locals(&self) -> Vec<Local> {
        self.ir_function.parameter_locals(self.def_id, &self.engine).await
    }

    pub(crate) const fn ir_function(&self) -> &'a IRFunction { self.ir_function }

    pub(crate) fn savables_at(&self, block_id: BlockID) -> impl Iterator<Item = Savable> + '_ {
        let saved_locals = self
            .local_liveness
            .block_exit(block_id)
            .expect("should exist")
            .live()
            .map(Savable::Local);
        let saved_exprs = self
            .expr_liveness
            .block_exit(block_id)
            .expect("should exist")
            .live()
            .map(Savable::Expression);
        let saved_borrowed_locals = self
            .maybe_borrowed_locals
            .block_exit(block_id)
            .expect("should exist")
            .iter()
            .map(Savable::Local);

        saved_locals.chain(saved_exprs).chain(saved_borrowed_locals)
    }
}

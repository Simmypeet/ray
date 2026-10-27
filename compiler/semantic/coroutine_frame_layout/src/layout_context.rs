use qbice::storage::intern::Interned;
use rayc_ir::{
    address::Local,
    borrowed_locals::MaybeBorrowedLocals,
    cfg::BlockID,
    ir_function::{IRFunction, IRFunctionID, IRFunctionMap},
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
    ir_function_map: &'a IRFunctionMap,
}

impl<'a> LayoutContext<'a> {
    #[must_use]
    pub async fn new(
        subst: Subst,
        engine: TrackedEngine,
        def_id: GlobalSymbolID,
        local_ir_function_id: IRFunctionID,
        ir_function_map: &'a IRFunctionMap,
    ) -> Self {
        let ir_function = ir_function_map.get_function(local_ir_function_id);

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
            ir_function_map,
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

    pub(crate) fn capture_savables(&self) -> Option<impl Iterator<Item = Savable> + 'a> {
        self.ir_function.context().capture_map_id().map(|capture_map_id| {
            self.ir_function_map
                .get_capture_map(capture_map_id)
                .iter()
                .map(|(id, _)| Savable::Local(Local::Capture(id)))
        })
    }

    pub(crate) const fn borrows_captures(&self) -> bool {
        self.ir_function.context().borrows_captures()
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

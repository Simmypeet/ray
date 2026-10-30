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
use rayc_transitive_closure::BitSet;
use rayc_type::{
    subst::{Subst, Substitutable},
    ty::Ty,
};

use crate::{candidate_index::CandidateIndex, savable::Savable};

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

    /// Returns the values held across the suspension point that terminates
    /// `block_id`, each once.
    ///
    /// The expressions the terminator evaluates are not held: the waitee is
    /// consumed by it, and the values it defines on its outgoing edges do not
    /// exist yet while it is suspended.
    pub(crate) fn savables_at(&self, block_id: BlockID) -> impl Iterator<Item = Savable> + '_ {
        let terminator = self
            .ir_function
            .block_terminator(block_id)
            .expect("a suspension point is a terminator");
        let borrowed_locals =
            self.maybe_borrowed_locals.block_exit(block_id).expect("should exist");

        // a local may be both live and borrowed, so the live ones that are
        // borrowed are left to the borrowed locals
        let saved_locals = self
            .local_liveness
            .block_exit(block_id)
            .expect("should exist")
            .live()
            .filter(|local| !borrowed_locals.contains(*local))
            .map(Savable::Local);
        let saved_exprs = self
            .expr_liveness
            .block_exit(block_id)
            .expect("should exist")
            .live()
            .filter(|expr| !terminator.evaluated_expressions().any(|defined| defined == *expr))
            .map(Savable::Expression);
        let saved_borrowed_locals = borrowed_locals.iter().map(Savable::Local);

        saved_locals.chain(saved_exprs).chain(saved_borrowed_locals)
    }

    /// Returns, for each point of `block_id` in order, the candidates of
    /// `index` live just before it: locals and expression values, use-live or
    /// drop-live. The last entry is the terminator's point.
    pub(crate) fn live_candidates_before(
        &self,
        index: &CandidateIndex,
        block_id: BlockID,
    ) -> Vec<BitSet> {
        let points = self.ir_function.block_instructions(block_id).len() + 1;
        let mut before = vec![index.empty_set(); points];

        self.local_liveness.for_each_point_rev(self.ir_function, block_id, |point, live| {
            index.extend_set(&mut before[point.instruction_idx()], live.live().map(Savable::Local));
        });
        self.expr_liveness.for_each_point_rev(self.ir_function, block_id, |point, live| {
            index.extend_set(
                &mut before[point.instruction_idx()],
                live.live().map(Savable::Expression),
            );
        });

        before
    }

    /// Returns the candidates of `index` live at the exit of `block_id`, on
    /// some edge out of it.
    pub(crate) fn live_candidates_at_exit(
        &self,
        index: &CandidateIndex,
        block_id: BlockID,
    ) -> BitSet {
        let traversed = "traversed blocks are reachable";
        let locals = self.local_liveness.block_exit(block_id).expect(traversed).live();
        let exprs = self.expr_liveness.block_exit(block_id).expect(traversed).live();

        index.set_of(locals.map(Savable::Local).chain(exprs.map(Savable::Expression)))
    }

    /// Returns, for each point of `block_id` in order, the candidates of
    /// `index` that may be borrowed just before it. The last entry is the
    /// terminator's point; a terminator borrows nothing, so it is also the
    /// exit of the block.
    pub(crate) fn borrowed_candidates_before(
        &self,
        index: &CandidateIndex,
        block_id: BlockID,
    ) -> Vec<BitSet> {
        let points = self.ir_function.block_instructions(block_id).len() + 1;
        let mut before = Vec::with_capacity(points);

        self.maybe_borrowed_locals.for_each_point(self.ir_function, block_id, |_, borrowed| {
            before.push(index.set_of(borrowed.iter().map(Savable::Local)));
        });

        before
    }
}

use qbice::storage::intern::Interned;
use rayc_ir::{
    address::Local,
    cfg::{BlockID, Conditional, Point, Terminator},
    ir_expr::{IRExpr, IRExprID, IRExprKind, load::Load, ref_of::RefOf},
    ir_function::{FunctionID, IRFunctionMap},
};
use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
use rayc_qbice::{TrackedEngine, create_minimal_engine};
use rayc_solver::Solver;
use rayc_source_file::GlobalSourceID;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::{Mutability, Primitive, Ty, lifetime::Lifetime};

use super::LiveLoans;
use crate::{
    constraint::{LoanID, LocalizedConstraints},
    region_liveness::RegionLiveness,
    renumber::Renumbering,
    variance::LifetimeVariances,
};

/// Builds the root function of an [`IRFunctionMap`] one instruction at a
/// time, with erased lifetimes as type inference leaves them.
struct FunctionBuilder {
    engine: TrackedEngine,
    functions: IRFunctionMap,
    function_id: FunctionID,
}

impl FunctionBuilder {
    async fn new() -> Self {
        let engine = create_minimal_engine().await;
        let functions = IRFunctionMap::new(GlobalSymbolID::default());
        let function_id = functions.root_id();
        Self { engine, functions, function_id }
    }

    fn int32(&self) -> Interned<Ty> { Ty::new_primitive(Primitive::Int32, &self.engine) }

    /// Returns `&'_ int32`, with an erased lifetime.
    fn reference(&self) -> Interned<Ty> {
        let lifetime = Ty::new_lifetime(Lifetime::Erased, &self.engine);
        Ty::new_reference(lifetime, self.int32(), Mutability::Immutable, &self.engine)
    }

    fn variable(&mut self, ty: Interned<Ty>) -> Local {
        let scope_id = self.functions.root_scope_id(self.function_id);
        Local::Variable(self.functions.create_variable_in_scope(
            self.function_id,
            scope_id,
            ty,
            test_span(),
        ))
    }

    fn entry(&self) -> BlockID { self.functions.entry_block(self.function_id) }

    fn block(&mut self) -> BlockID { self.functions.create_block(self.function_id) }

    fn next_point(&self, block_id: BlockID) -> Point {
        let instruction_idx =
            self.functions.get_function(self.function_id).block_instructions(block_id).len();
        Point::builder().block_id(block_id).instruction_idx(instruction_idx).build()
    }

    /// Evaluates an expression of kind `kind` and type `ty` at the end of
    /// `block_id`.
    fn evaluate(
        &mut self,
        block_id: BlockID,
        kind: impl Into<IRExprKind>,
        ty: Interned<Ty>,
    ) -> IRExprID {
        let expression =
            self.functions.insert_expression(self.function_id, IRExpr::new(kind, test_span(), ty));
        self.functions.push_expression(self.function_id, block_id, expression);
        expression
    }

    /// Stores `value` into the whole of `local` at the end of `block_id`,
    /// and returns the point of the store.
    fn store(&mut self, block_id: BlockID, local: Local, value: IRExprID) -> Point {
        let point = self.next_point(block_id);
        let address = local.to_address(&self.engine);
        self.functions.push_store(self.function_id, block_id, address, value, test_span());
        point
    }

    /// Writes a fresh `int32` into `local`, as `local = ..`, and returns the
    /// point of the store.
    fn write(&mut self, block_id: BlockID, local: Local) -> Point {
        let value = self.evaluate(block_id, IRExprKind::Error, self.int32());
        self.store(block_id, local, value)
    }

    /// Stores `&place` into `local`, as `local = &place`, and returns the
    /// point of the borrow.
    fn borrow_into(&mut self, block_id: BlockID, local: Local, place: Local) -> Point {
        let point = self.next_point(block_id);
        let address = place.to_address(&self.engine);
        let reference = self.evaluate(block_id, RefOf::new(address), self.reference());
        self.store(block_id, local, reference);
        point
    }

    /// Loads the reference held in `local` and stores it into `into`, as
    /// `into = local`.
    fn copy(&mut self, block_id: BlockID, into: Local, local: Local) {
        let address = local.to_address(&self.engine);
        let value = self.evaluate(block_id, Load::new(address), self.reference());
        self.store(block_id, into, value);
    }

    /// Reads the reference held in `local`, as `use(local)`.
    fn read(&mut self, block_id: BlockID, local: Local) {
        let address = local.to_address(&self.engine);
        let value = self.evaluate(block_id, Load::new(address), self.reference());
        let no_op = Ty::new_no_op_drop_instance(self.reference(), &self.engine);
        self.functions.push_expr_discard(self.function_id, block_id, value, no_op);
    }

    fn terminate(&mut self, block_id: BlockID, terminator: Terminator) {
        self.functions.set_terminator(self.function_id, block_id, terminator);
    }

    /// Runs the borrow checker up to the loan liveness, and returns the loan
    /// borrowed at each of `borrows` together with the live loans.
    async fn live_loans<const N: usize>(mut self, borrows: [Point; N]) -> ([LoanID; N], LiveLoans) {
        let _ = Renumbering::renumber(&mut self.functions, &self.engine).await;
        let variances = LifetimeVariances::compute(&self.functions, &self.engine).await;
        let mut solver = Solver::without_givens(self.engine.clone()).await;

        let function = self.functions.get_function(self.function_id);
        let constraints = LocalizedConstraints::collect(function, None, &mut solver).await;
        let liveness = RegionLiveness::compute(&self.functions, self.function_id).await;
        let live_loans = LiveLoans::compute(function, &constraints, &liveness, &variances);

        let loans = borrows.map(|point| {
            constraints
                .loans()
                .find_map(|(loan_id, loan)| (loan.point() == point).then_some(loan_id))
                .expect("a borrow should issue a loan")
        });
        (loans, live_loans)
    }
}

fn test_span() -> RelativeSpan {
    RelativeSpan {
        start: RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ROOT_BRANCH_ID },
        end: RelativeLocation { offset: 1, mode: OffsetMode::End, relative_to: ROOT_BRANCH_ID },
        source_id: GlobalSourceID::default(),
    }
}

/// The points of the running example of "Polonius revisited, part 2".
struct RunningExample {
    /// `p = &x`, which issues `L0`.
    borrow_x: Point,
    /// `q = &y`, which issues `L1`.
    borrow_y: Point,
    /// `x = ..` in `bb2`, after `p = q`.
    x_after_reassign: Point,
    /// `y = ..` in `bb3`, where only `L0` flows on.
    y_on_else_path: Point,
    /// `y = ..` in `bb4`, before `use(p)`.
    y_before_use: Point,
}

/// Builds the running example:
///
/// ```text
/// bb1: p = &x; y = ..; q = &y; if c then bb2 else bb3
/// bb2: p = q; x = ..; jump bb4
/// bb3: y = ..; jump bb4
/// bb4: y = ..; use(p); return
/// ```
fn running_example(builder: &mut FunctionBuilder) -> RunningExample {
    let x = builder.variable(builder.int32());
    let y = builder.variable(builder.int32());
    let p = builder.variable(builder.reference());
    let q = builder.variable(builder.reference());
    let bb1 = builder.entry();
    let bb2 = builder.block();
    let bb3 = builder.block();
    let bb4 = builder.block();

    let borrow_x = builder.borrow_into(bb1, p, x);
    builder.write(bb1, y);
    let borrow_y = builder.borrow_into(bb1, q, y);
    let bool_ty = Ty::new_primitive(Primitive::Bool, &builder.engine);
    let condition = builder.evaluate(bb1, IRExprKind::Error, bool_ty);
    builder.terminate(bb1, Terminator::Conditional(Conditional::new(condition, bb2, bb3)));

    builder.copy(bb2, p, q);
    let x_after_reassign = builder.write(bb2, x);
    builder.terminate(bb2, Terminator::Jump(bb4));

    let y_on_else_path = builder.write(bb3, y);
    builder.terminate(bb3, Terminator::Jump(bb4));

    let y_before_use = builder.write(bb4, y);
    builder.read(bb4, p);
    builder.terminate(bb4, Terminator::Return(None));

    RunningExample { borrow_x, borrow_y, x_after_reassign, y_on_else_path, y_before_use }
}

// input: liveness of `L1` at `y = ..` in `bb4`
// premise: the running example: `L1` flows from `q` into `p` in `bb2`, and
//          `p` is used after `y = ..` in `bb4`
// output: `L1` is live
#[tokio::test]
async fn loan_is_live_where_it_flows_into_a_region_used_later() {
    let mut builder = FunctionBuilder::new().await;
    let example = running_example(&mut builder);
    let ([_, l1], live_loans) = builder.live_loans([example.borrow_x, example.borrow_y]).await;

    assert!(live_loans.is_live(l1, example.y_before_use));
}

// input: liveness of `L1` at `y = ..` in `bb3`
// premise: the running example: `q` is dead in `bb3`, and `p` there only
//          holds `L0`
// output: `L1` is not live, although `bb3` reaches the use of `p` in `bb4`
#[tokio::test]
async fn loan_is_not_live_on_a_path_where_no_live_region_holds_it() {
    let mut builder = FunctionBuilder::new().await;
    let example = running_example(&mut builder);
    let ([_, l1], live_loans) = builder.live_loans([example.borrow_x, example.borrow_y]).await;

    assert!(!live_loans.is_live(l1, example.y_on_else_path));
}

// input: liveness of `L0` at `x = ..` in `bb2`
// premise: the running example: `p = q` overwrites `p`, the only holder of
//          `L0`, at the start of `bb2`
// output: `L0` is not live
#[tokio::test]
async fn loan_is_not_live_after_its_holder_is_overwritten() {
    let mut builder = FunctionBuilder::new().await;
    let example = running_example(&mut builder);
    let ([l0, _], live_loans) = builder.live_loans([example.borrow_x, example.borrow_y]).await;

    assert!(!live_loans.is_live(l0, example.x_after_reassign));
}

// input: liveness of `L0` at `y = ..` in `bb3`
// premise: the running example: `p` still holds `L0` in `bb3`, and is used
//          in `bb4`
// output: `L0` is live
#[tokio::test]
async fn loan_stays_live_across_blocks_while_its_holder_is_live() {
    let mut builder = FunctionBuilder::new().await;
    let example = running_example(&mut builder);
    let ([l0, _], live_loans) = builder.live_loans([example.borrow_x, example.borrow_y]).await;

    assert!(live_loans.is_live(l0, example.y_on_else_path));
}

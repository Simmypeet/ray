//! Builders of small IR functions, and the borrow checker's analyses of them,
//! shared by the unit tests of this crate.

use qbice::storage::intern::Interned;
use rayc_ir::{
    address::{Address, Local, Projection},
    cfg::{BlockID, Conditional, Point, Terminator},
    ir_expr::{IRExpr, IRExprID, IRExprKind, load::Load, ref_of::RefOf},
    ir_function::{FunctionID, IRFunctionMap},
    scope::ScopeID,
};
use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
use rayc_qbice::{TrackedEngine, create_minimal_engine};
use rayc_solver::Solver;
use rayc_source_file::GlobalSourceID;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::{Integer, Mutability, Primitive, Ty, lifetime::Lifetime};

use crate::{
    active_loans::LoanActivity,
    constraint::{LoanID, LocalizedConstraints},
    live_loans::LiveLoans,
    region_liveness::RegionLiveness,
    renumber::Renumbering,
    variance::LifetimeVariances,
};

/// Builds the root function of an [`IRFunctionMap`] one instruction at a
/// time, with erased lifetimes as type inference leaves them.
pub struct FunctionBuilder {
    engine: TrackedEngine,
    functions: IRFunctionMap,
    function_id: FunctionID,
}

impl FunctionBuilder {
    pub async fn new() -> Self {
        let engine = create_minimal_engine().await;
        let functions = IRFunctionMap::new(GlobalSymbolID::default());
        let function_id = functions.root_id();
        Self { engine, functions, function_id }
    }

    pub fn int32(&self) -> Interned<Ty> {
        Ty::new_primitive(Primitive::Integer(Integer::Int32), &self.engine)
    }

    pub fn bool(&self) -> Interned<Ty> { Ty::new_primitive(Primitive::Bool, &self.engine) }

    /// Returns `(int32, int32)`.
    pub fn pair(&self) -> Interned<Ty> {
        Ty::new_tuple(self.engine.intern_unsized([self.int32(), self.int32()]), &self.engine)
    }

    /// Returns `&'_ int32`, with an erased lifetime.
    pub fn reference(&self) -> Interned<Ty> {
        let lifetime = Ty::new_lifetime(Lifetime::Erased, &self.engine);
        Ty::new_reference(lifetime, self.int32(), Mutability::Immutable, &self.engine)
    }

    /// Declares a variable of type `ty` in the root scope.
    pub fn variable(&mut self, ty: Interned<Ty>) -> Local {
        let scope_id = self.functions.root_scope_id(self.function_id);
        self.variable_in(scope_id, ty)
    }

    /// Declares a variable of type `ty` in `scope_id`.
    pub fn variable_in(&mut self, scope_id: ScopeID, ty: Interned<Ty>) -> Local {
        Local::Variable(self.functions.create_variable_in_scope(
            self.function_id,
            scope_id,
            ty,
            test_span(),
        ))
    }

    /// Creates a scope nested in the root scope.
    pub fn scope(&mut self) -> ScopeID {
        let root = self.functions.root_scope_id(self.function_id);
        self.functions.insert_scope(self.function_id, root)
    }

    /// Returns the address of the whole of `local`.
    pub fn address(&self, local: Local) -> Address { local.to_address(&self.engine) }

    /// Returns the address of the tuple element `index` of `local`.
    pub fn element(&self, local: Local, index: usize) -> Address {
        self.address(local).projected(Projection::Tuple(index), &self.engine)
    }

    pub fn entry(&self) -> BlockID { self.functions.entry_block(self.function_id) }

    pub fn block(&mut self) -> BlockID { self.functions.create_block(self.function_id) }

    /// Returns the point of the next instruction appended to `block_id`.
    pub fn next_point(&self, block_id: BlockID) -> Point {
        let instruction_idx =
            self.functions.get_function(self.function_id).block_instructions(block_id).len();
        Point::builder().block_id(block_id).instruction_idx(instruction_idx).build()
    }

    pub fn push_scope(&mut self, block_id: BlockID, scope_id: ScopeID) {
        self.functions.push_scope_push_instruction(self.function_id, block_id, scope_id);
    }

    /// Ends `scope_id`, and returns the point where it ends.
    pub fn pop_scope(&mut self, block_id: BlockID, scope_id: ScopeID) -> Point {
        let point = self.next_point(block_id);
        self.functions.push_scope_pop_instruction(self.function_id, block_id, scope_id);
        point
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

    /// Stores `value` into `address` at the end of `block_id`, and returns
    /// the point of the store.
    fn store(&mut self, block_id: BlockID, address: Address, value: IRExprID) -> Point {
        let point = self.next_point(block_id);
        self.functions.push_store(self.function_id, block_id, address, value, test_span());
        point
    }

    /// Writes a fresh value of type `ty` into `address`, as `address = ..`,
    /// and returns the point of the store.
    pub fn write(&mut self, block_id: BlockID, address: Address, ty: Interned<Ty>) -> Point {
        let value = self.evaluate(block_id, IRExprKind::Error, ty);
        self.store(block_id, address, value)
    }

    /// Stores `&place` into `local`, as `local = &place`, and returns the
    /// point of the borrow.
    pub fn borrow_into(&mut self, block_id: BlockID, local: Local, place: Address) -> Point {
        let point = self.next_point(block_id);
        let reference = self.evaluate(block_id, RefOf::new(place), self.reference());
        let address = self.address(local);
        self.store(block_id, address, reference);
        point
    }

    /// Loads the reference held in `local` and stores it into `into`, as
    /// `into = local`.
    pub fn copy(&mut self, block_id: BlockID, into: Local, local: Local) {
        let value = self.evaluate(block_id, Load::new(self.address(local)), self.reference());
        let address = self.address(into);
        self.store(block_id, address, value);
    }

    /// Reads the reference held in `local`, as `use(local)`.
    pub fn read(&mut self, block_id: BlockID, local: Local) {
        let value = self.evaluate(block_id, Load::new(self.address(local)), self.reference());
        let no_op = Ty::new_no_op_drop_instance(self.reference(), &self.engine);
        self.functions.push_expr_discard(self.function_id, block_id, value, no_op);
    }

    /// Branches on a fresh condition to `then_block` or `else_block`.
    pub fn branch(&mut self, block_id: BlockID, then_block: BlockID, else_block: BlockID) {
        let condition = self.evaluate(block_id, IRExprKind::Error, self.bool());
        let conditional = Conditional::new(condition, then_block, else_block);
        self.terminate(block_id, Terminator::Conditional(conditional));
    }

    pub fn terminate(&mut self, block_id: BlockID, terminator: Terminator) {
        self.functions.set_terminator(self.function_id, block_id, terminator);
    }

    /// Runs the borrow checker's analyses on the function.
    pub async fn analyze(mut self) -> Analysis {
        let _ = Renumbering::renumber(&mut self.functions, &self.engine).await;
        let variances = LifetimeVariances::compute(&self.functions, &self.engine).await;
        let mut solver = Solver::without_givens(self.engine.clone()).await;

        let function = self.functions.get_function(self.function_id);
        let constraints = LocalizedConstraints::collect(function, None, &mut solver).await;
        let liveness = RegionLiveness::compute(&self.functions, self.function_id).await;
        let live_loans = LiveLoans::compute(function, &constraints, &liveness, &variances);

        Analysis {
            functions: self.functions,
            function_id: self.function_id,
            constraints,
            live_loans,
        }
    }
}

/// The borrow checker's analyses of a function built by a
/// [`FunctionBuilder`].
pub struct Analysis {
    functions: IRFunctionMap,
    function_id: FunctionID,
    constraints: LocalizedConstraints,
    live_loans: LiveLoans,
}

impl Analysis {
    /// Returns the loan issued by the borrow at `point`.
    pub fn loan_at(&self, point: Point) -> LoanID {
        self.constraints
            .loans()
            .find_map(|(loan_id, loan)| (loan.point() == point).then_some(loan_id))
            .expect("a borrow should issue a loan")
    }

    /// Returns whether `loan` is live at `point`.
    pub fn is_live(&self, loan: LoanID, point: Point) -> bool {
        self.live_loans.is_live(loan, point)
    }

    /// Computes the loans active at each point of the function.
    pub async fn loan_activity(&self) -> LoanActivity<'_> {
        let function = self.functions.get_function(self.function_id);
        LoanActivity::compute(function, &self.constraints, &self.live_loans).await
    }
}

fn test_span() -> RelativeSpan {
    RelativeSpan {
        start: RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ROOT_BRANCH_ID },
        end: RelativeLocation { offset: 1, mode: OffsetMode::End, relative_to: ROOT_BRANCH_ID },
        source_id: GlobalSourceID::default(),
    }
}

/// The points of the running example of "Polonius revisited, part 2"; see
/// [`running_example`].
pub struct RunningExample {
    /// `p = &x`, which issues `L0`.
    pub borrow_x: Point,
    /// `q = &y`, which issues `L1`.
    pub borrow_y: Point,
    /// `x = ..` in `bb2`, after `p = q`.
    pub x_after_reassign: Point,
    /// `y = ..` in `bb3`, where only `L0` flows on.
    pub y_on_else_path: Point,
    /// `y = ..` in `bb4`, before `use(p)`.
    pub y_before_use: Point,
}

/// Builds the running example:
///
/// ```text
/// bb1: p = &x; y = ..; q = &y; if c then bb2 else bb3
/// bb2: p = q; x = ..; jump bb4
/// bb3: y = ..; jump bb4
/// bb4: y = ..; use(p); return
/// ```
pub fn running_example(builder: &mut FunctionBuilder) -> RunningExample {
    let x = builder.variable(builder.int32());
    let y = builder.variable(builder.int32());
    let p = builder.variable(builder.reference());
    let q = builder.variable(builder.reference());
    let bb1 = builder.entry();
    let bb2 = builder.block();
    let bb3 = builder.block();
    let bb4 = builder.block();

    let borrow_x = builder.borrow_into(bb1, p, builder.address(x));
    builder.write(bb1, builder.address(y), builder.int32());
    let borrow_y = builder.borrow_into(bb1, q, builder.address(y));
    builder.branch(bb1, bb2, bb3);

    builder.copy(bb2, p, q);
    let x_after_reassign = builder.write(bb2, builder.address(x), builder.int32());
    builder.terminate(bb2, Terminator::Jump(bb4));

    let y_on_else_path = builder.write(bb3, builder.address(y), builder.int32());
    builder.terminate(bb3, Terminator::Jump(bb4));

    let y_before_use = builder.write(bb4, builder.address(y), builder.int32());
    builder.read(bb4, p);
    builder.terminate(bb4, Terminator::Return(None));

    RunningExample { borrow_x, borrow_y, x_after_reassign, y_on_else_path, y_before_use }
}

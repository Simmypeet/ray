use qbice::storage::intern::Interned;
use rayc_arena::ID;
use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
use rayc_qbice::{TrackedEngine, create_minimal_engine};
use rayc_source_file::GlobalSourceID;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::{Integer, Primitive, Ty};

use super::MaybeBorrowedLocals;
use crate::{
    address::{Address, Local},
    cfg::{BlockID, Conditional, Point, Terminator},
    ir_expr::{IRExpr, ref_of::RefOf},
    ir_function::{FunctionID, IRFunctionMap},
    scope::ScopeID,
};

/// Builds the root function of an [`IRFunctionMap`] one instruction at a
/// time, recording the point of each instruction it appends.
struct FunctionBuilder {
    engine: TrackedEngine,
    functions: IRFunctionMap,
    function_id: FunctionID,
    ty: Interned<Ty>,
}

impl FunctionBuilder {
    async fn new() -> Self {
        let engine = create_minimal_engine().await;
        let ty = Ty::new_primitive(Primitive::Integer(Integer::Int32), &engine);
        let functions = IRFunctionMap::new(GlobalSymbolID::default());
        let function_id = functions.root_id();
        Self { engine, functions, function_id, ty }
    }

    /// Creates a scope nested in the root scope.
    fn scope(&mut self) -> ScopeID {
        let root = self.functions.root_scope_id(self.function_id);
        self.functions.insert_scope(self.function_id, root)
    }

    fn variable(&mut self, scope_id: ScopeID) -> Local {
        Local::Variable(self.functions.create_variable_in_scope(
            self.function_id,
            scope_id,
            self.ty.clone(),
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

    fn address(&self, local: Local) -> Address { local.to_address(&self.engine) }

    fn borrow(&mut self, block_id: BlockID, address: Address) -> Point {
        let point = self.next_point(block_id);
        let expression = self.functions.insert_expression(
            self.function_id,
            IRExpr::new(RefOf::new(address), test_span(), self.ty.clone()),
        );
        self.functions.push_expression(self.function_id, block_id, expression);
        point
    }

    fn push_scope(&mut self, block_id: BlockID, scope_id: ScopeID) {
        self.functions.push_scope_push_instruction(self.function_id, block_id, scope_id);
    }

    fn pop_scope(&mut self, block_id: BlockID, scope_id: ScopeID) -> Point {
        let point = self.next_point(block_id);
        self.functions.push_scope_pop_instruction(self.function_id, block_id, scope_id);
        point
    }

    fn terminate(&mut self, block_id: BlockID, terminator: Terminator) {
        self.functions.set_terminator(self.function_id, block_id, terminator);
    }

    async fn borrowed_before(&self, point: Point) -> Vec<Local> {
        let function = self.functions.get_function(self.function_id);
        MaybeBorrowedLocals::compute(function)
            .await
            .borrowed_before(function, point)
            .unwrap()
            .iter()
            .collect()
    }
}

fn test_span() -> RelativeSpan {
    RelativeSpan {
        start: RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ROOT_BRANCH_ID },
        end: RelativeLocation { offset: 1, mode: OffsetMode::End, relative_to: ROOT_BRANCH_ID },
        source_id: GlobalSourceID::default(),
    }
}

// input: borrowed locals before the borrow, before the end of the scope and
//        after it
// premise: push(s); x.&; pop(s); return, where `s` declares `x`
// output: `x` is borrowed from the borrow until its scope ends
#[tokio::test]
async fn borrow_lasts_until_its_scope_ends() {
    let mut builder = FunctionBuilder::new().await;
    let scope = builder.scope();
    let x = builder.variable(scope);
    let entry = builder.entry();
    builder.push_scope(entry, scope);
    let address = builder.address(x);
    let borrow = builder.borrow(entry, address);
    let pop = builder.pop_scope(entry, scope);
    let end = builder.next_point(entry);
    builder.terminate(entry, Terminator::Return(None));

    assert_eq!(builder.borrowed_before(borrow).await, vec![]);
    assert_eq!(builder.borrowed_before(pop).await, vec![x]);
    assert_eq!(builder.borrowed_before(end).await, vec![]);
}

// input: borrowed locals after `p.*.&`
// premise: p.*.&; return
// output: nothing, since the borrow is of the memory `p` points to
#[tokio::test]
async fn borrow_behind_dereference_does_not_borrow_the_pointer() {
    let mut builder = FunctionBuilder::new().await;
    let scope = builder.scope();
    let p = builder.variable(scope);
    let entry = builder.entry();
    let mut address = builder.address(p);
    address.add_deref(&builder.engine);
    builder.borrow(entry, address);
    let end = builder.next_point(entry);
    builder.terminate(entry, Terminator::Return(None));

    assert_eq!(builder.borrowed_before(end).await, vec![]);
}

// input: borrowed locals at the head of a loop
// premise: entry: push(s); jump head; head: if c then body else exit;
//          body: x.&; jump head; exit: pop(s); return,
//          where `s` declares `x`
// output: `x` is borrowed at the head, through the back edge
#[tokio::test]
async fn borrow_flows_around_a_loop() {
    let mut builder = FunctionBuilder::new().await;
    let scope = builder.scope();
    let x = builder.variable(scope);
    let entry = builder.entry();
    let head = builder.block();
    let body = builder.block();
    let exit = builder.block();

    builder.push_scope(entry, scope);
    builder.terminate(entry, Terminator::Jump(head));

    let condition = builder
        .functions
        .insert_expression(builder.function_id, IRExpr::new_error(test_span(), builder.ty.clone()));
    builder.functions.push_expression(builder.function_id, head, condition);
    let branch = builder.next_point(head);
    builder.terminate(head, Terminator::Conditional(Conditional::new(condition, body, exit)));

    let address = builder.address(x);
    builder.borrow(body, address);
    builder.terminate(body, Terminator::Jump(head));

    builder.pop_scope(exit, scope);
    builder.terminate(exit, Terminator::Return(None));

    assert_eq!(builder.borrowed_before(branch).await, vec![x]);
}

// input: borrowed locals after the scope of the borrow ends
// premise: push(s); p.&; pop(s); return, where `p` is a parameter
// output: `p` stays borrowed, since its storage lasts for the whole function
#[tokio::test]
async fn parameter_stays_borrowed() {
    let mut builder = FunctionBuilder::new().await;
    let scope = builder.scope();
    let p = Local::Parameter(ID::new(0));
    let entry = builder.entry();
    builder.push_scope(entry, scope);
    let address = builder.address(p);
    builder.borrow(entry, address);
    builder.pop_scope(entry, scope);
    let end = builder.next_point(entry);
    builder.terminate(entry, Terminator::Return(None));

    assert_eq!(builder.borrowed_before(end).await, vec![p]);
}

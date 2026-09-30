use qbice::storage::intern::Interned;
use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
use rayc_qbice::{TrackedEngine, create_minimal_engine};
use rayc_source_file::GlobalSourceID;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::{Primitive, Ty};

use super::{LiveLocals, LocalLiveness};
use crate::{
    address::{Address, Local},
    cfg::{
        AddressDrop, BlockID, Conditional, Instruction, InstructionInsertion, Point, Terminator,
    },
    ir_expr::{IRExpr, load::Load},
    ir_function::{FunctionID, IRFunctionMap},
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
        let ty = Ty::new_primitive(Primitive::Int32, &engine);
        let functions = IRFunctionMap::new(GlobalSymbolID::default());
        let function_id = functions.root_id();
        Self { engine, functions, function_id, ty }
    }

    fn variable(&mut self) -> Local {
        let scope_id = self.functions.root_scope_id(self.function_id);
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

    fn read(&mut self, block_id: BlockID, local: Local) -> Point {
        let point = self.next_point(block_id);
        let address = self.address(local);
        let expression = self.functions.insert_expression(
            self.function_id,
            IRExpr::new(Load::new(address), test_span(), self.ty.clone()),
        );
        self.functions.push_expression(self.function_id, block_id, expression);
        point
    }

    fn drop(&mut self, block_id: BlockID, local: Local, drop_instance: Interned<Ty>) -> Point {
        let point = self.next_point(block_id);
        let address = self.address(local);
        let mut insertion = InstructionInsertion::new();
        insertion.insert_before(point, [Instruction::AddressDrop(AddressDrop::new(
            address,
            drop_instance,
            test_span(),
        ))]);
        self.functions.insert_instructions(self.function_id, insertion);
        point
    }

    fn store(&mut self, block_id: BlockID, address: Address) -> Point {
        let point = self.next_point(block_id);
        let value = self
            .functions
            .insert_expression(self.function_id, IRExpr::new_error(test_span(), self.ty.clone()));
        self.functions.push_expression(self.function_id, block_id, value);
        self.functions.push_store(self.function_id, block_id, address, value, test_span());
        point
    }

    fn terminate(&mut self, block_id: BlockID, terminator: Terminator) {
        self.functions.set_terminator(self.function_id, block_id, terminator);
    }

    async fn live_before(&self, point: Point) -> LiveLocals {
        let function = self.functions.get_function(self.function_id);
        LocalLiveness::compute(function).await.live_before(function, point).unwrap()
    }
}

fn test_span() -> RelativeSpan {
    RelativeSpan {
        start: RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ROOT_BRANCH_ID },
        end: RelativeLocation { offset: 1, mode: OffsetMode::End, relative_to: ROOT_BRANCH_ID },
        source_id: GlobalSourceID::default(),
    }
}

// input: liveness before and after the last read of `x`
// premise: read(x); read(x); return
// output: `x` is use-live before the last read and dead after it
#[tokio::test]
async fn local_is_use_live_until_its_last_read() {
    let mut builder = FunctionBuilder::new().await;
    let x = builder.variable();
    let entry = builder.entry();
    builder.read(entry, x);
    let last_read = builder.read(entry, x);
    let end = builder.next_point(entry);
    builder.terminate(entry, Terminator::Return(None));

    assert_eq!(builder.live_before(last_read).await.use_live().collect::<Vec<_>>(), vec![x]);
    assert_eq!(builder.live_before(end).await, LiveLocals::default());
}

// input: liveness before `x = v`
// premise: x = v; read(x); return
// output: `x` is dead, since the store overwrites its previous value
#[tokio::test]
async fn whole_store_ends_the_previous_value() {
    let mut builder = FunctionBuilder::new().await;
    let x = builder.variable();
    let entry = builder.entry();
    let address = builder.address(x);
    let store = builder.store(entry, address);
    builder.read(entry, x);
    builder.terminate(entry, Terminator::Return(None));

    assert!(!builder.live_before(store).await.is_use_live(x));
}

// input: liveness before `x.0 = v`
// premise: x.0 = v; read(x); return
// output: `x` is use-live, since the rest of `x` keeps its value
#[tokio::test]
async fn partial_store_keeps_the_local_live() {
    let mut builder = FunctionBuilder::new().await;
    let x = builder.variable();
    let entry = builder.entry();
    let address =
        builder.address(x).projected(crate::address::Projection::Tuple(0), &builder.engine);
    let store = builder.store(entry, address);
    builder.read(entry, x);
    builder.terminate(entry, Terminator::Return(None));

    assert!(builder.live_before(store).await.is_use_live(x));
}

// input: liveness before `p.* = v`
// premise: p.* = v; return
// output: `p` is use-live, since the store reads the pointer
#[tokio::test]
async fn store_through_dereference_reads_the_pointer() {
    let mut builder = FunctionBuilder::new().await;
    let p = builder.variable();
    let entry = builder.entry();
    let mut address = builder.address(p);
    address.add_raw_deref(&builder.engine);
    let store = builder.store(entry, address);
    builder.terminate(entry, Terminator::Return(None));

    assert!(builder.live_before(store).await.is_use_live(p));
}

// input: liveness before the read and between the read and the drop of `x`
// premise: read(x); drop(x); return
// output: `x` is use-live before the read, then only drop-live
#[tokio::test]
async fn local_is_drop_live_after_its_last_use() {
    let mut builder = FunctionBuilder::new().await;
    let x = builder.variable();
    let entry = builder.entry();
    let read = builder.read(entry, x);
    let drop = builder.drop(entry, x, builder.ty.clone());
    builder.terminate(entry, Terminator::Return(None));

    let before_read = builder.live_before(read).await;
    assert!(before_read.is_use_live(x));
    assert!(!before_read.is_drop_live(x));

    let before_drop = builder.live_before(drop).await;
    assert!(!before_drop.is_use_live(x));
    assert!(before_drop.is_drop_live(x));
}

// input: liveness before the read and no-op drop of `x`
// premise: entry: read(x); jump exit; exit: no_op_drop(x); return
// output: `x` is use-live before its read and dead on entry to `exit`
#[tokio::test]
async fn no_op_drop_does_not_keep_the_local_live() {
    let mut builder = FunctionBuilder::new().await;
    let x = builder.variable();
    let entry = builder.entry();
    let exit = builder.block();
    let read = builder.read(entry, x);
    builder.terminate(entry, Terminator::Jump(exit));
    let no_op = Ty::new_no_op_drop_instance(builder.ty.clone(), &builder.engine);
    let drop = builder.drop(exit, x, no_op);
    builder.terminate(exit, Terminator::Return(None));

    let function = builder.functions.get_function(builder.function_id);
    let liveness = LocalLiveness::compute(function).await;
    assert!(liveness.live_before(function, read).unwrap().is_use_live(x));
    assert_eq!(liveness.block_entry(exit), Some(&LiveLocals::default()));
    assert_eq!(liveness.live_before(function, drop), Some(LiveLocals::default()));
}

// input: liveness at a branch on `c`
// premise: if c then read(x) else drop(x); return
// output: `x` is use-live and not drop-live, since the use subsumes the drop
#[tokio::test]
async fn use_on_one_branch_subsumes_drop_on_another() {
    let mut builder = FunctionBuilder::new().await;
    let x = builder.variable();
    let entry = builder.entry();
    let then_block = builder.block();
    let else_block = builder.block();
    let merge = builder.block();

    let condition = builder
        .functions
        .insert_expression(builder.function_id, IRExpr::new_error(test_span(), builder.ty.clone()));
    let branch = builder.next_point(entry);
    builder.terminate(
        entry,
        Terminator::Conditional(Conditional::new(condition, then_block, else_block)),
    );
    builder.read(then_block, x);
    builder.terminate(then_block, Terminator::Jump(merge));
    builder.drop(else_block, x, builder.ty.clone());
    builder.terminate(else_block, Terminator::Jump(merge));
    builder.terminate(merge, Terminator::Return(None));

    let live = builder.live_before(branch).await;
    assert!(live.is_use_live(x));
    assert!(!live.is_drop_live(x));
}

// input: liveness at the jump into a loop with no exit
// premise: entry: jump loop; loop: read(x); jump loop
// output: `x` is use-live, although no path reaches a return
#[tokio::test]
async fn local_read_in_a_loop_without_exit_is_live() {
    let mut builder = FunctionBuilder::new().await;
    let x = builder.variable();
    let entry = builder.entry();
    let loop_block = builder.block();

    let jump = builder.next_point(entry);
    builder.terminate(entry, Terminator::Jump(loop_block));
    builder.read(loop_block, x);
    builder.terminate(loop_block, Terminator::Jump(loop_block));

    assert!(builder.live_before(jump).await.is_use_live(x));
}

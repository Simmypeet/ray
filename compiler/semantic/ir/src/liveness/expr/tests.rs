use qbice::storage::intern::Interned;
use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
use rayc_qbice::create_minimal_engine;
use rayc_source_file::GlobalSourceID;
use rayc_type::ty::{Primitive, Ty};

use super::{ExprLiveness, LiveExprs};
use crate::{
    cfg::{BlockID, Conditional, Point, Terminator},
    ir_expr::{IRExpr, IRExprID, IRExprKind, phi::Phi, tuple::Tuple},
    ir_function::{FunctionID, IRFunctionMap},
};

/// Builds the root function of an [`IRFunctionMap`] one instruction at a
/// time, recording the point of each instruction it appends.
struct FunctionBuilder {
    functions: IRFunctionMap,
    function_id: FunctionID,
    ty: Interned<Ty>,
}

impl FunctionBuilder {
    async fn new() -> Self {
        let engine = create_minimal_engine().await;
        let ty = Ty::new_primitive(Primitive::Int32, &engine);
        let functions = IRFunctionMap::new(ty.clone());
        let function_id = functions.root_id();
        Self { functions, function_id, ty }
    }

    fn entry(&self) -> BlockID { self.functions.entry_block(self.function_id) }

    fn block(&mut self) -> BlockID { self.functions.create_block(self.function_id) }

    fn next_point(&self, block_id: BlockID) -> Point {
        let instruction_idx =
            self.functions.get_function(self.function_id).block_instructions(block_id).len();
        Point::builder().block_id(block_id).instruction_idx(instruction_idx).build()
    }

    /// Defines an expression of kind `kind` at the end of `block_id`.
    fn define(&mut self, block_id: BlockID, kind: impl Into<IRExprKind>) -> (IRExprID, Point) {
        let point = self.next_point(block_id);
        let expression = self
            .functions
            .insert_expression(self.function_id, IRExpr::new(kind, test_span(), self.ty.clone()));
        self.functions.push_expression(self.function_id, block_id, expression);
        (expression, point)
    }

    /// Defines a value with no operands at the end of `block_id`.
    fn value(&mut self, block_id: BlockID) -> IRExprID {
        self.define(block_id, IRExprKind::Error).0
    }

    fn discard(&mut self, block_id: BlockID, expression: IRExprID) -> Point {
        let point = self.next_point(block_id);
        self.functions.push_expr_discard(self.function_id, block_id, expression, self.ty.clone());
        point
    }

    fn terminate(&mut self, block_id: BlockID, terminator: Terminator) {
        self.functions.set_terminator(self.function_id, block_id, terminator);
    }

    async fn liveness(&self) -> ExprLiveness {
        ExprLiveness::compute(self.functions.get_function(self.function_id)).await
    }

    async fn live_before(&self, point: Point) -> LiveExprs {
        let function = self.functions.get_function(self.function_id);
        self.liveness().await.live_before(function, point).unwrap()
    }
}

fn test_span() -> RelativeSpan {
    RelativeSpan {
        start: RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ROOT_BRANCH_ID },
        end: RelativeLocation { offset: 1, mode: OffsetMode::End, relative_to: ROOT_BRANCH_ID },
        source_id: GlobalSourceID::default(),
    }
}

// input: liveness before `v`, before `t`, and at the return
// premise: v = ..; t = (v,); return t
// output: `v` is use-live only between its definition and `t`, and `t` is
//         use-live at the return, which consumes it
#[tokio::test]
async fn expression_is_use_live_from_its_definition_to_its_use() {
    let mut builder = FunctionBuilder::new().await;
    let entry = builder.entry();
    let v_point = builder.next_point(entry);
    let v = builder.value(entry);
    let (t, t_point) = builder.define(entry, Tuple::new(vec![v]));
    let end = builder.next_point(entry);
    builder.terminate(entry, Terminator::Return(Some(t)));

    assert_eq!(builder.live_before(v_point).await, LiveExprs::default());
    assert_eq!(builder.live_before(t_point).await.use_live().collect::<Vec<_>>(), vec![v]);
    assert_eq!(builder.live_before(end).await.use_live().collect::<Vec<_>>(), vec![t]);
}

// input: liveness before the discard of `v`
// premise: v = ..; discard v; return
// output: `v` is drop-live and not use-live
#[tokio::test]
async fn discarded_expression_is_only_drop_live() {
    let mut builder = FunctionBuilder::new().await;
    let entry = builder.entry();
    let v = builder.value(entry);
    let discard = builder.discard(entry, v);
    builder.terminate(entry, Terminator::Return(None));

    let live = builder.live_before(discard).await;
    assert!(!live.is_use_live(v));
    assert!(live.is_drop_live(v));
}

// input: liveness at the jumps into `merge` and on entry to `merge`
// premise: entry: c = ..; if c then then_block else else_block
//          then_block: x = ..; jump merge
//          else_block: y = ..; jump merge
//          merge: p = phi(then_block: x, else_block: y); return p
// output: `x` is live only at the jump from `then_block`, `y` only at the
//         jump from `else_block`, and neither is live on entry to `merge`
#[tokio::test]
async fn phi_operand_is_live_only_on_its_incoming_edge() {
    let mut builder = FunctionBuilder::new().await;
    let entry = builder.entry();
    let then_block = builder.block();
    let else_block = builder.block();
    let merge = builder.block();

    let condition = builder.value(entry);
    builder.terminate(
        entry,
        Terminator::Conditional(Conditional::new(condition, then_block, else_block)),
    );
    let then_value = builder.value(then_block);
    let then_jump = builder.next_point(then_block);
    builder.terminate(then_block, Terminator::Jump(merge));
    let else_value = builder.value(else_block);
    let else_jump = builder.next_point(else_block);
    builder.terminate(else_block, Terminator::Jump(merge));
    let incoming = [(then_block, then_value), (else_block, else_value)].into_iter().collect();
    let (merged, _) = builder.define(merge, Phi::new(incoming));
    builder.terminate(merge, Terminator::Return(Some(merged)));

    assert_eq!(builder.live_before(then_jump).await.use_live().collect::<Vec<_>>(), vec![
        then_value
    ]);
    assert_eq!(builder.live_before(else_jump).await.use_live().collect::<Vec<_>>(), vec![
        else_value
    ]);
    assert_eq!(builder.liveness().await.block_entry(merge), Some(&LiveExprs::default()));
}

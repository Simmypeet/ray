use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
use rayc_qbice::create_minimal_engine;
use rayc_source_file::GlobalSourceID;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::{Integer, Primitive, Ty};

use super::{FunctionID, IRFunctionMap};
use crate::{
    cfg::{BlockID, Conditional, ControlFlowEdgeKind, Terminator},
    ir_expr::{IRExpr, IRExprID, phi::Phi},
};

/// Builds the root function of an [`IRFunctionMap`].
struct FunctionBuilder {
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
        Self { functions, function_id, ty }
    }

    fn entry(&self) -> BlockID { self.functions.entry_block(self.function_id) }

    fn block(&mut self) -> BlockID { self.functions.create_block(self.function_id) }

    /// Defines `expression` at the end of `block_id`.
    fn define(&mut self, block_id: BlockID, expression: IRExpr) -> IRExprID {
        let expression = self.functions.insert_expression(self.function_id, expression);
        self.functions.push_expression(self.function_id, block_id, expression);
        expression
    }

    /// Defines a value with no operands at the end of `block_id`.
    fn value(&mut self, block_id: BlockID) -> IRExprID {
        self.define(block_id, IRExpr::new_error(test_span(), self.ty.clone()))
    }

    /// Defines a phi at the end of `block_id`, which then returns it.
    fn phi(&mut self, block_id: BlockID, incoming: &[(BlockID, IRExprID)]) -> IRExprID {
        let phi = Phi::new(incoming.iter().copied().collect());
        let phi = self.define(block_id, IRExpr::new(phi, test_span(), self.ty.clone()));
        self.terminate(block_id, Terminator::Return(Some(phi)));
        phi
    }

    fn terminate(&mut self, block_id: BlockID, terminator: Terminator) {
        self.functions.set_terminator(self.function_id, block_id, terminator);
    }

    /// Splits the edge of kind `kind` leaving `source`.
    fn split(&mut self, source: BlockID, kind: ControlFlowEdgeKind) -> BlockID {
        let edge = self
            .functions
            .get_function(self.function_id)
            .cfg
            .outgoing_edges(source)
            .unwrap()
            .find(|edge| edge.kind() == kind)
            .unwrap();
        self.functions.split_edge(self.function_id, edge)
    }

    /// Returns the incoming values of `phi`, keyed by predecessor block.
    fn incoming(&self, phi: IRExprID) -> FxHashMap<BlockID, IRExprID> {
        let function = self.functions.get_function(self.function_id);
        function.get_expression(phi).kind().as_phi().unwrap().incoming().collect()
    }
}

fn test_span() -> RelativeSpan {
    RelativeSpan {
        start: RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ROOT_BRANCH_ID },
        end: RelativeLocation { offset: 1, mode: OffsetMode::End, relative_to: ROOT_BRANCH_ID },
        source_id: GlobalSourceID::default(),
    }
}

// input: split the edge entry -> merge
// premise: entry: x = ..; c = ..; if c then merge else other
//          other: y = ..; jump merge
//          merge: p = phi(entry: x, other: y)
// output: p = phi(split: x, other: y)
#[tokio::test]
async fn split_edge_moves_the_phi_value_to_the_new_block() {
    let mut builder = FunctionBuilder::new().await;
    let entry = builder.entry();
    let other = builder.block();
    let merge = builder.block();

    let x = builder.value(entry);
    let condition = builder.value(entry);
    builder.terminate(entry, Terminator::Conditional(Conditional::new(condition, merge, other)));
    let y = builder.value(other);
    builder.terminate(other, Terminator::Jump(merge));
    let phi = builder.phi(merge, &[(entry, x), (other, y)]);

    let split = builder.split(entry, ControlFlowEdgeKind::ConditionalTrue);

    assert_eq!(builder.incoming(phi), FxHashMap::from_iter([(split, x), (other, y)]));
}

// input: split the true edge entry -> merge
// premise: entry: x = ..; c = ..; if c then merge else merge
//          merge: p = phi(entry: x)
// output: p = phi(entry: x, split: x), since the false edge still comes
//         from entry
#[tokio::test]
async fn split_edge_keeps_the_phi_value_on_a_parallel_edge() {
    let mut builder = FunctionBuilder::new().await;
    let entry = builder.entry();
    let merge = builder.block();

    let x = builder.value(entry);
    let condition = builder.value(entry);
    builder.terminate(entry, Terminator::Conditional(Conditional::new(condition, merge, merge)));
    let phi = builder.phi(merge, &[(entry, x)]);

    let split = builder.split(entry, ControlFlowEdgeKind::ConditionalTrue);

    assert_eq!(builder.incoming(phi), FxHashMap::from_iter([(entry, x), (split, x)]));
}

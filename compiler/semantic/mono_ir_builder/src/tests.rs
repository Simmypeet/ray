use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{
    cfg::{Conditional, Terminator as IRTerminator},
    ir_expr::{IRExpr, IRExprKind, literal::Literal, phi::Phi},
    ir_function::IRFunctionMap,
};
use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
use rayc_mono_ir::{
    MonoDefInstance, MonoIR,
    cfg::{BlockID, Terminator},
    function::{LocalID, MonoFunction},
    instruction::Instruction,
    operand::{Constant, Operand},
    rvalue::Rvalue,
    ty::{MonoType, ReturnType},
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::parameter::ParameterMap;
use rayc_source_file::{GlobalSourceID, LocalSourceID};
use rayc_symbol::{GlobalSymbolID, SymbolID};
use rayc_target::TargetID;
use rayc_type::{
    poly_var::{GlobalPolyVarID, PolyVarID},
    subst::Subst,
    ty::{Primitive, Ty},
};

use crate::builder::Builder;

fn test_def(id: u128) -> GlobalSymbolID { TargetID::TEST.make_global(SymbolID::from_u128(id)) }

fn test_span(offset: usize) -> RelativeSpan {
    let source_id: GlobalSourceID = TargetID::TEST.make_global(LocalSourceID::new(0, 0));
    RelativeSpan {
        start: RelativeLocation { offset, mode: OffsetMode::Start, relative_to: ROOT_BRANCH_ID },
        end: RelativeLocation {
            offset: offset + 1,
            mode: OffsetMode::End,
            relative_to: ROOT_BRANCH_ID,
        },
        source_id,
    }
}

async fn lower_test_ir(
    engine: &TrackedEngine,
    def_id: GlobalSymbolID,
    source: IRFunctionMap,
    return_type: Interned<Ty>,
    substitution: Subst,
) -> MonoIR {
    Builder::new(
        engine.clone(),
        MonoDefInstance::new(def_id, substitution),
        engine.intern(source),
        engine.intern(ParameterMap::new()),
        return_type,
    )
    .lower()
    .await
}

// Input: a generic IR expression and return type instantiated with Int32.
// Premise: MonoIR is keyed by the definition plus its concrete substitution.
// Output: the function signature, local, and literal are all concretely Int32.
#[tokio::test]
async fn substitution_monomorphizes_signature_and_expression_values() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let def_id = test_def(1);
    let poly_var = GlobalPolyVarID::new(def_id, PolyVarID::new(0));
    let generic_type = Ty::new_poly_var(poly_var, &engine);
    let int_type = Ty::new_primitive(Primitive::Int32, &engine);
    let effect = Ty::new_effect_row([], None, &engine);
    let mut source = IRFunctionMap::new(effect);
    let root = source.root_id();
    let entry = source.entry_block(root);
    let literal = source.insert_expression(
        root,
        IRExpr::new(IRExprKind::Literal(Literal::Numeric(7)), test_span(0), generic_type.clone()),
    );
    source.push_expression(root, entry, literal);
    source.set_terminator(root, entry, IRTerminator::Return(Some(literal)));

    let mono = lower_test_ir(
        &engine,
        def_id,
        source,
        generic_type,
        Subst::new_singleton(poly_var, int_type),
    )
    .await;
    let function = mono.root();
    let ReturnType::Value(return_types) = function.signature().return_type() else {
        panic!("a Ray value-returning function should not lower to C void");
    };
    assert_eq!(return_types.len(), 1);
    assert!(matches!(&*return_types[0], MonoType::Int32));

    let locals = function.locals().collect::<Vec<_>>();
    assert_eq!(locals.len(), 1);
    assert!(matches!(&**locals[0].1.ty(), MonoType::Int32));
    let instructions = function.get_block(function.entry_block()).instructions();
    assert_eq!(instructions.len(), 1);
    let Instruction::Assign(assign) = &instructions[0] else {
        panic!("a literal should lower to an assignment");
    };
    assert!(matches!(assign.value(), Rvalue::Use(Operand::Constant(Constant::Int32(7)))));
}

// Input: a diamond CFG whose merge block begins with one phi expression.
// Premise: MonoIR has mutable locals and deliberately has no phi instruction.
// Output: each predecessor is split by an edge block containing parallel-copy
// assignments.
#[tokio::test]
async fn phi_values_lower_to_explicit_copies_on_incoming_edges() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let def_id = test_def(2);
    let int_type = Ty::new_primitive(Primitive::Int32, &engine);
    let bool_type = Ty::new_primitive(Primitive::Bool, &engine);
    let effect = Ty::new_effect_row([], None, &engine);
    let mut source = IRFunctionMap::new(effect);
    let root = source.root_id();
    let entry = source.entry_block(root);
    let then_block = source.create_block(root);
    let else_block = source.create_block(root);
    let merge_block = source.create_block(root);

    let condition = source.insert_expression(
        root,
        IRExpr::new(IRExprKind::Literal(Literal::Bool(true)), test_span(0), bool_type),
    );
    source.push_expression(root, entry, condition);
    source.set_terminator(
        root,
        entry,
        IRTerminator::Conditional(Conditional::new(condition, then_block, else_block)),
    );

    let then_value = source.insert_expression(
        root,
        IRExpr::new(IRExprKind::Literal(Literal::Numeric(10)), test_span(1), int_type.clone()),
    );
    source.push_expression(root, then_block, then_value);
    source.set_terminator(root, then_block, IRTerminator::Jump(merge_block));
    let else_value = source.insert_expression(
        root,
        IRExpr::new(IRExprKind::Literal(Literal::Numeric(20)), test_span(2), int_type.clone()),
    );
    source.push_expression(root, else_block, else_value);
    source.set_terminator(root, else_block, IRTerminator::Jump(merge_block));

    let incoming = FxHashMap::from_iter([(then_block, then_value), (else_block, else_value)]);
    let phi = source.insert_expression(
        root,
        IRExpr::new(IRExprKind::Phi(Phi::new(incoming)), test_span(3), int_type.clone()),
    );
    source.push_expression(root, merge_block, phi);
    source.set_terminator(root, merge_block, IRTerminator::Return(Some(phi)));

    let mono = lower_test_ir(&engine, def_id, source, int_type, Subst::new_empty()).await;
    let function = mono.root();
    assert_eq!(function.blocks().len(), 6);
    let Terminator::Branch(branch) = function
        .get_block(function.entry_block())
        .terminator()
        .expect("entry block should be terminated")
    else {
        panic!("the source conditional should remain a branch");
    };
    let (then_merge, then_phi) = phi_edge_contract(function, branch.then_block());
    let (else_merge, else_phi) = phi_edge_contract(function, branch.else_block());
    assert_eq!(then_merge, else_merge);
    assert_eq!(then_phi, else_phi);
    let Terminator::Return(Some(Operand::Copy(returned))) =
        function.get_block(then_merge).terminator().expect("merge block should be terminated")
    else {
        panic!("merge block should return its phi local");
    };
    assert_eq!(returned.local(), then_phi);
}

fn phi_edge_contract(function: &MonoFunction, predecessor: BlockID) -> (BlockID, LocalID) {
    let Terminator::Goto(edge) =
        function.get_block(predecessor).terminator().expect("phi predecessor should be terminated")
    else {
        panic!("phi predecessor should jump through a split edge");
    };
    let edge_block = function.get_block(*edge);
    assert_eq!(edge_block.instructions().len(), 2);
    let [Instruction::Assign(read), Instruction::Assign(write)] = edge_block.instructions() else {
        panic!("phi edge should copy through a temporary local");
    };
    let Rvalue::Use(Operand::Copy(temporary)) = write.value() else {
        panic!("phi destination should receive the edge temporary");
    };
    assert_eq!(read.destination().local(), temporary.local());
    let Terminator::Goto(merge) = edge_block.terminator().expect("phi edge should be terminated")
    else {
        panic!("phi edge should jump to the merge block");
    };
    (*merge, write.destination().local())
}

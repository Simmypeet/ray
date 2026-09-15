use std::{
    io::Write,
    process::{Command, Stdio},
};

use rayc_mono_ir::{
    MonoDefInstance, MonoIR,
    cfg::Terminator,
    instance::FunctionReference,
    instruction::{Call, Instruction},
    operand::{Constant, FunctionOperand, Operand},
    ty::{AggregateType, FunctionSignature, MonoType, ReturnType, Tuple},
};
use rayc_qbice::create_minimal_engine;
use rayc_symbol::SymbolID;
use rayc_target::TargetID;
use rayc_type::subst::Subst;

use crate::{
    generator::Generator,
    name::{aggregate_name, definition_name},
    write_c_translation_unit_from_mono_ir,
};

// Input: a MonoIR function whose parameter is an outer tuple containing an
// inner tuple by value, and whose result is unit.
// Premise: aggregate definitions must precede functions and nested by-value
// layouts must be complete before their containing layouts.
// Output: the four C sections are ordered and the inner struct precedes the
// outer struct while the function uses its stable definition name.
#[tokio::test]
async fn writes_ordered_translation_unit_from_mono_ir() {
    let engine = create_minimal_engine().await;
    let int32 = engine.intern(MonoType::Int32);
    let inner_tuple = Tuple::new(engine.intern_unsized([int32]));
    let inner_aggregate = AggregateType::Tuple(inner_tuple.clone());
    let inner_type = engine.intern(MonoType::Aggregate(inner_aggregate.clone()));
    let outer_tuple = Tuple::new(engine.intern_unsized([inner_type]));
    let outer_aggregate = AggregateType::Tuple(outer_tuple.clone());
    let outer_type = engine.intern(MonoType::Aggregate(outer_aggregate.clone()));
    let unit_tuple = Tuple::new(engine.intern_unsized(Vec::new()));
    let unit_aggregate = AggregateType::Tuple(unit_tuple.clone());
    let unit_type = engine.intern(MonoType::Aggregate(unit_aggregate));

    let signature =
        FunctionSignature::new(engine.intern_unsized([outer_type]), ReturnType::Value(unit_type));
    let instance = MonoDefInstance::new(
        TargetID::TEST.make_global(SymbolID::from_u128(7)),
        Subst::new_empty(),
        &engine,
    )
    .await;
    let mut ir = MonoIR::new(instance.clone(), signature);
    ir.set_terminator(
        ir.root_id(),
        ir.entry_block(ir.root_id()),
        Terminator::Return(Some(Operand::Constant(Constant::Unit))),
    );

    let mut output = Vec::new();
    write_c_translation_unit_from_mono_ir(&engine, [ir], &mut output).await.unwrap();
    let output = String::from_utf8(output).unwrap();

    let type_forwards = output.find("/* Aggregate type forward declarations */").unwrap();
    let type_definitions = output.find("/* Aggregate type definitions */").unwrap();
    let function_forwards = output.find("/* Function forward declarations */").unwrap();
    let function_definitions = output.find("/* Function definitions */").unwrap();
    assert!(type_forwards < type_definitions);
    assert!(type_definitions < function_forwards);
    assert!(function_forwards < function_definitions);

    let inner_definition =
        output.find(&format!("struct {} {{", aggregate_name(&inner_aggregate))).unwrap();
    let outer_definition =
        output.find(&format!("struct {} {{", aggregate_name(&outer_aggregate))).unwrap();
    assert!(inner_definition < outer_definition);

    let definition_name = definition_name(&instance);
    assert!(output.contains(&format!("{definition_name}(")));

    assert_compiles(&output);
}

// Input: one preloaded definition that directly calls a second preloaded
// definition which is not placed in the initial definition queue.
// Premise: global MonoIR calls extend `def_worklist` and definition identities
// can be named without inspecting the callee body.
// Output: both stable function definitions are present in compilable C.
#[tokio::test]
async fn discovers_called_definition_through_worklist() {
    let engine = create_minimal_engine().await;
    let unit_tuple = Tuple::new(engine.intern_unsized(Vec::new()));
    let unit_type = engine.intern(MonoType::Aggregate(AggregateType::Tuple(unit_tuple)));
    let signature =
        FunctionSignature::new(engine.intern_unsized(Vec::new()), ReturnType::Value(unit_type));
    let root_instance = MonoDefInstance::new(
        TargetID::TEST.make_global(SymbolID::from_u128(11)),
        Subst::new_empty(),
        &engine,
    )
    .await;
    let dependency_instance = MonoDefInstance::new(
        TargetID::TEST.make_global(SymbolID::from_u128(12)),
        Subst::new_empty(),
        &engine,
    )
    .await;

    let mut root = MonoIR::new(root_instance.clone(), signature.clone());
    root.push_instruction(
        root.root_id(),
        root.entry_block(root.root_id()),
        Instruction::Call(Call::new(
            None,
            Operand::Function(FunctionOperand::new(
                FunctionReference::Global(dependency_instance.clone()),
                signature.clone(),
            )),
            Vec::new(),
        )),
    );
    root.set_terminator(
        root.root_id(),
        root.entry_block(root.root_id()),
        Terminator::Return(Some(Operand::Constant(Constant::Unit))),
    );

    let mut dependency = MonoIR::new(dependency_instance.clone(), signature);
    dependency.set_terminator(
        dependency.root_id(),
        dependency.entry_block(dependency.root_id()),
        Terminator::Return(Some(Operand::Constant(Constant::Unit))),
    );

    let output =
        Generator::new(&engine, [root_instance.clone()], [root, dependency], None).generate().await;
    assert!(output.contains(&format!("{}(void) {{", definition_name(&root_instance))));
    assert!(output.contains(&format!("{}(void) {{", definition_name(&dependency_instance))));

    assert_compiles(&output);
}

fn assert_compiles(output: &str) {
    let mut compiler = Command::new("cc")
        .args(["-std=c11", "-x", "c", "-fsyntax-only", "-"])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    compiler.stdin.take().unwrap().write_all(output.as_bytes()).unwrap();
    let compilation = compiler.wait_with_output().unwrap();
    assert!(
        compilation.status.success(),
        "generated C did not compile:\n{}",
        String::from_utf8_lossy(&compilation.stderr)
    );
}

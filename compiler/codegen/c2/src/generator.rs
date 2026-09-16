use std::{collections::VecDeque, fmt::Write};

use qbice::storage::intern::Interned;
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_mono_ir::{
    MonoClosureInstance, MonoDefInstance, MonoIR,
    cfg::Terminator,
    instance::FunctionReference,
    instruction::Instruction,
    operand::{Constant, Operand},
    rvalue::{AggregateValue, Rvalue},
    ty::{AggregateType, FunctionSignature, HandlerLayout, MonoType, build_handler_layout},
};
use rayc_mono_ir_builder::lower_ir;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{name::get_name, symbol_kind::get_symbol_kind};

use crate::{
    c_type::signature_declaration,
    name::{
        AggregateID, aggregate_name, aggregate_typedef_name, closure_name, definition_name,
        ir_function_name,
    },
};

#[derive(Debug, Default)]
struct Buffers {
    type_forward_declarations: String,
    aggregate_definitions: String,
    function_forward_declarations: String,
    function_definitions: String,
}

#[derive(Debug)]
pub(super) struct Generator<'engine> {
    engine: &'engine TrackedEngine,
    def_worklist: VecDeque<MonoDefInstance>,
    aggregate_worklist: VecDeque<AggregateType>,
    seen_definitions: FxHashSet<MonoDefInstance>,
    internal_definitions: FxHashSet<MonoDefInstance>,
    seen_aggregates: FxHashSet<AggregateType>,
    preloaded_definitions: FxHashMap<MonoDefInstance, MonoIR>,
    global_signatures: FxHashMap<MonoDefInstance, FunctionSignature>,
    closure_signatures: FxHashMap<MonoClosureInstance, FunctionSignature>,
    exported_closures: FxHashSet<MonoClosureInstance>,
    aggregate_layouts: FxHashMap<AggregateType, Option<Interned<HandlerLayout>>>,
    function_declarations: Vec<String>,
    entry_point: Option<MonoDefInstance>,
    buffers: Buffers,
}

impl<'engine> Generator<'engine> {
    pub(super) fn new(
        engine: &'engine TrackedEngine,
        initial_definitions: impl IntoIterator<Item = MonoDefInstance>,
        preloaded_definitions: impl IntoIterator<Item = MonoIR>,
        entry_point: Option<MonoDefInstance>,
    ) -> Self {
        let mut generator = Self {
            engine,
            def_worklist: VecDeque::new(),
            aggregate_worklist: VecDeque::new(),
            seen_definitions: FxHashSet::default(),
            internal_definitions: FxHashSet::default(),
            seen_aggregates: FxHashSet::default(),
            preloaded_definitions: FxHashMap::default(),
            global_signatures: FxHashMap::default(),
            closure_signatures: FxHashMap::default(),
            exported_closures: FxHashSet::default(),
            aggregate_layouts: FxHashMap::default(),
            function_declarations: Vec::new(),
            entry_point,
            buffers: Buffers::default(),
        };

        for definition in preloaded_definitions {
            generator.internal_definitions.insert(definition.instance().clone());
            assert!(
                generator
                    .preloaded_definitions
                    .insert(definition.instance().clone(), definition)
                    .is_none(),
                "a MonoIR definition instance should only be preloaded once"
            );
        }

        let mut initial_definitions = initial_definitions.into_iter().collect::<Vec<_>>();
        initial_definitions.sort();
        initial_definitions.dedup();
        for definition in initial_definitions {
            generator.enqueue_definition(definition);
        }

        generator
    }

    pub(super) async fn generate(mut self) -> String {
        while !self.def_worklist.is_empty() || !self.aggregate_worklist.is_empty() {
            if let Some(definition) = self.def_worklist.pop_front() {
                self.process_definition(definition).await;
                continue;
            }
            if let Some(aggregate) = self.aggregate_worklist.pop_front() {
                self.process_aggregate(aggregate).await;
            }
        }

        // These buffers are populated only after their corresponding definition
        // buffers have discovered the complete set of required items.
        for closure in self.closure_signatures.keys() {
            assert!(
                self.exported_closures.contains(closure),
                "referenced nominal closure has no exported body"
            );
        }
        self.populate_aggregate_buffers();
        self.populate_function_forward_declarations();
        self.populate_entry_point();
        self.finish()
    }

    fn enqueue_definition(&mut self, definition: MonoDefInstance) {
        if self.seen_definitions.insert(definition.clone()) {
            self.def_worklist.push_back(definition);
        }
    }

    fn enqueue_aggregate(&mut self, aggregate: AggregateType) {
        if self.seen_aggregates.insert(aggregate.clone()) {
            self.aggregate_worklist.push_back(aggregate);
        }
    }

    async fn process_definition(&mut self, definition: MonoDefInstance) {
        if let Some(ir) = self.preloaded_definitions.remove(&definition) {
            self.process_ir(ir).await;
            return;
        }

        match self.engine.get_symbol_kind(definition.def_id()).await {
            rayc_symbol::symbol_kind::SymbolKind::Def
            | rayc_symbol::symbol_kind::SymbolKind::InstanceDef => {
                self.internal_definitions.insert(definition.clone());
                let ir =
                    lower_ir(self.engine, definition.def_id(), definition.substitution().clone())
                        .await;
                self.process_ir(ir).await;
            }
            rayc_symbol::symbol_kind::SymbolKind::ExternDef => {
                let signature = self
                    .global_signatures
                    .get(&definition)
                    .unwrap_or_else(|| {
                        panic!("an extern definition requires a signature from a call site")
                    })
                    .clone();
                self.collect_signature(&signature);
                let name = self.engine.get_name(definition.def_id()).await;
                self.function_declarations
                    .push(format!("extern {}", signature_declaration(&signature, &name, None)));
            }
            rayc_symbol::symbol_kind::SymbolKind::Effect
            | rayc_symbol::symbol_kind::SymbolKind::EffectOperation
            | rayc_symbol::symbol_kind::SymbolKind::Instance
            | rayc_symbol::symbol_kind::SymbolKind::Marker
            | rayc_symbol::symbol_kind::SymbolKind::MarkerImplementation
            | rayc_symbol::symbol_kind::SymbolKind::Module
            | rayc_symbol::symbol_kind::SymbolKind::Strut
            | rayc_symbol::symbol_kind::SymbolKind::Trait
            | rayc_symbol::symbol_kind::SymbolKind::TraitType
            | rayc_symbol::symbol_kind::SymbolKind::InstanceType
            | rayc_symbol::symbol_kind::SymbolKind::TraitDef => {
                panic!("non-definition symbol reached the C definition worklist")
            }
        }
    }

    async fn process_ir(&mut self, ir: MonoIR) {
        let mut function_ids =
            ir.functions().map(|(function_id, _)| function_id).collect::<Vec<_>>();
        function_ids.sort_unstable();
        for function_id in function_ids.iter().copied() {
            let function = ir.get_function(function_id);
            if let Some(closure) = ir.closure_instance(function_id) {
                assert!(self.exported_closures.insert(closure.clone()));
                self.record_closure_signature(closure, function.signature());
            }
            self.collect_signature(function.signature());
            for (_, local) in function.locals() {
                self.collect_type(local.ty());
            }
            let mut blocks = function.blocks().collect::<Vec<_>>();
            blocks.sort_unstable_by_key(|(block_id, _)| *block_id);
            for (_, block) in blocks {
                for instruction in block.instructions() {
                    self.collect_instruction(instruction, &ir);
                }
                self.collect_terminator(
                    block.terminator().expect("reachable MonoIR block should have a terminator"),
                    &ir,
                );
            }
        }

        for function_id in function_ids {
            let function = ir.get_function(function_id);
            let name = ir_function_name(&ir, function_id);
            let parameter_names = function
                .parameters()
                .map(|local| crate::name::local_name(local.index()))
                .collect::<Vec<_>>();
            self.function_declarations.push(signature_declaration(
                function.signature(),
                &name,
                Some(&parameter_names),
            ));

            let definition = self.emit_function_definition(&ir, function_id).await;
            self.buffers.function_definitions.push_str(&definition);
            self.buffers.function_definitions.push_str("\n\n");
        }
    }

    async fn process_aggregate(&mut self, aggregate: AggregateType) {
        let handler_layout = match &aggregate {
            AggregateType::EffectHandler(handler) => {
                let layout =
                    self.engine.build_handler_layout(handler.mono_effect_instance().clone()).await;
                for operation in layout.operations() {
                    self.collect_signature(operation.signature());
                }
                Some(layout)
            }
            AggregateType::Tuple(tuple) => {
                for field in tuple.fields() {
                    self.collect_type(field);
                }
                None
            }
            AggregateType::Environment(environment) => {
                for capture in environment.captures() {
                    self.collect_type(capture);
                }
                None
            }

            AggregateType::Struct(st) => {
                for field in st.fields().values() {
                    self.collect_type(field);
                }
                None
            }
        };
        assert!(self.aggregate_layouts.insert(aggregate, handler_layout).is_none());
    }

    fn collect_signature(&mut self, signature: &FunctionSignature) {
        for parameter in signature.parameter_types() {
            self.collect_type(parameter);
        }
        match signature.return_type() {
            rayc_mono_ir::ty::ReturnType::Void => {}
            rayc_mono_ir::ty::ReturnType::Value(ty) => {
                self.collect_type(ty);
            }
        }
    }

    fn collect_type(&mut self, ty: &MonoType) {
        match ty {
            MonoType::Bool
            | MonoType::Int32
            | MonoType::Float32
            | MonoType::CInt
            | MonoType::CStr
            | MonoType::OpaquePointer(_) => {}
            MonoType::Pointer(pointer) => self.collect_type(pointer.pointee()),
            MonoType::Aggregate(aggregate) => self.enqueue_aggregate(aggregate.clone()),
            MonoType::FunctionPointer(signature) => self.collect_signature(signature),
        }
    }

    fn collect_instruction(&mut self, instruction: &Instruction, ir: &MonoIR) {
        match instruction {
            Instruction::Assign(assign) => self.collect_rvalue(assign.value(), ir),
            Instruction::Call(call) => {
                self.collect_operand(call.callee(), ir);
                for argument in call.arguments() {
                    self.collect_operand(argument, ir);
                }
            }
        }
    }

    fn collect_terminator(&mut self, terminator: &Terminator, ir: &MonoIR) {
        match terminator {
            Terminator::Goto(_) | Terminator::Unreachable => {}
            Terminator::Branch(branch) => self.collect_operand(branch.condition(), ir),
            Terminator::Return(value) => {
                if let Some(value) = value {
                    self.collect_operand(value, ir);
                }
            }
        }
    }

    fn collect_rvalue(&mut self, value: &Rvalue, ir: &MonoIR) {
        match value {
            Rvalue::Use(operand) => self.collect_operand(operand, ir),
            Rvalue::AddressOf(_) => {}
            Rvalue::Unary(unary) => self.collect_operand(unary.operand(), ir),
            Rvalue::Binary(binary) => {
                self.collect_operand(binary.left(), ir);
                self.collect_operand(binary.right(), ir);
            }
            Rvalue::Cast(cast) => {
                self.collect_operand(cast.operand(), ir);
                self.collect_type(cast.target());
            }
            Rvalue::Aggregate(aggregate) => match aggregate {
                AggregateValue::Tuple(tuple) => {
                    self.enqueue_aggregate(AggregateType::Tuple(tuple.ty().clone()));
                    for field in tuple.fields() {
                        self.collect_operand(field, ir);
                    }
                }
                AggregateValue::Environment(environment) => {
                    self.enqueue_aggregate(AggregateType::Environment(environment.ty().clone()));
                    for field in environment.fields() {
                        self.collect_operand(field, ir);
                    }
                }

                AggregateValue::EffectHandler(handler) => {
                    self.enqueue_aggregate(AggregateType::EffectHandler(
                        rayc_mono_ir::ty::EffectHandler::new(handler.effect().clone()),
                    ));
                    for slot in handler.slots().values() {
                        self.collect_operand(slot.environment(), ir);
                        self.collect_operand(slot.function(), ir);
                    }
                }

                AggregateValue::Struct(st) => {
                    self.enqueue_aggregate(AggregateType::Struct(st.ty().clone()));
                    for field in st.fields().values() {
                        self.collect_operand(field, ir);
                    }
                }
            },
        }
    }

    fn collect_operand(&mut self, operand: &Operand, ir: &MonoIR) {
        match operand {
            Operand::Copy(_) => {}
            Operand::Constant(constant) => match constant {
                Constant::NullPointer(ty) => self.collect_type(ty),
                Constant::Unit
                | Constant::Bool(_)
                | Constant::Int32(_)
                | Constant::Float32(_)
                | Constant::CInt(_)
                | Constant::CStr(_) => {}
            },
            Operand::Function(function) => {
                self.collect_signature(function.signature());
                match function.function() {
                    FunctionReference::Local(id) => {
                        assert_eq!(function.signature(), ir.get_function(*id).signature());
                        if let Some(closure) = ir.closure_instance(*id) {
                            self.record_closure_signature(closure, function.signature());
                        }
                    }
                    FunctionReference::Closure(closure) => {
                        self.enqueue_definition(closure.owner().clone());
                        self.record_closure_signature(closure.clone(), function.signature());
                    }
                    FunctionReference::Global(definition) => {
                        if let Some(previous) = self
                            .global_signatures
                            .insert(definition.clone(), function.signature().clone())
                        {
                            assert_eq!(
                                previous,
                                *function.signature(),
                                "a global function instance should have one concrete signature"
                            );
                        }
                        self.enqueue_definition(definition.clone());
                    }
                }
            }
        }
    }

    fn record_closure_signature(
        &mut self,
        closure: MonoClosureInstance,
        signature: &FunctionSignature,
    ) {
        if let Some(previous) = self.closure_signatures.insert(closure, signature.clone()) {
            assert_eq!(&previous, signature, "nominal closure reference and body ABI must agree");
        }
    }

    fn populate_aggregate_buffers(&mut self) {
        let aggregates = self.ordered_aggregates();
        for aggregate in &aggregates {
            let handler_layout = self
                .aggregate_layouts
                .get(aggregate)
                .expect("aggregate worklist item should have a resolved layout")
                .as_deref();
            let definition = Self::emit_aggregate_definition(aggregate, handler_layout);
            self.buffers.aggregate_definitions.push_str(&definition);
            self.buffers.aggregate_definitions.push_str("\n\n");
        }

        for aggregate in aggregates {
            let name = aggregate_name(&aggregate);
            let typedef = aggregate_typedef_name(&aggregate);
            writeln!(self.buffers.type_forward_declarations, "typedef struct {name} {typedef};")
                .unwrap();
        }
    }

    fn populate_function_forward_declarations(&mut self) {
        self.function_declarations.sort();
        self.function_declarations.dedup();
        for declaration in &self.function_declarations {
            writeln!(self.buffers.function_forward_declarations, "{declaration};").unwrap();
        }
    }

    fn populate_entry_point(&mut self) {
        let Some(entry_point) = &self.entry_point else {
            return;
        };
        assert!(
            self.seen_definitions.contains(entry_point),
            "the executable entry point should be present in the definition worklist"
        );
        writeln!(
            self.buffers.function_definitions,
            "int main(void) {{ return {}(); }}",
            definition_name(entry_point)
        )
        .unwrap();
    }

    fn ordered_aggregates(&self) -> Vec<AggregateType> {
        let mut roots = self.aggregate_layouts.keys().cloned().collect::<Vec<_>>();
        roots.sort_unstable_by_key(AggregateID::for_type);
        let mut visiting = FxHashSet::default();
        let mut visited = FxHashSet::default();
        let mut ordered = Vec::with_capacity(roots.len());
        for aggregate in roots {
            self.visit_aggregate(&aggregate, &mut visiting, &mut visited, &mut ordered);
        }
        ordered
    }

    fn visit_aggregate(
        &self,
        aggregate: &AggregateType,
        visiting: &mut FxHashSet<AggregateType>,
        visited: &mut FxHashSet<AggregateType>,
        ordered: &mut Vec<AggregateType>,
    ) {
        if visited.contains(aggregate) {
            return;
        }
        assert!(
            visiting.insert(aggregate.clone()),
            "aggregate types cannot contain a cycle by value"
        );

        let mut dependencies = by_value_dependencies(aggregate);
        dependencies.sort_unstable_by_key(AggregateID::for_type);
        for dependency in dependencies {
            assert!(
                self.aggregate_layouts.contains_key(&dependency),
                "aggregate dependency should have passed through the aggregate worklist"
            );
            self.visit_aggregate(&dependency, visiting, visited, ordered);
        }

        visiting.remove(aggregate);
        visited.insert(aggregate.clone());
        ordered.push(aggregate.clone());
    }

    pub(super) async fn callable_name(&self, reference: &FunctionReference, ir: &MonoIR) -> String {
        match reference {
            FunctionReference::Local(function_id) => ir_function_name(ir, *function_id),
            FunctionReference::Closure(closure) => closure_name(closure),
            FunctionReference::Global(definition) => {
                if self.internal_definitions.contains(definition) {
                    return definition_name(definition);
                }
                match self.engine.get_symbol_kind(definition.def_id()).await {
                    rayc_symbol::symbol_kind::SymbolKind::Def
                    | rayc_symbol::symbol_kind::SymbolKind::InstanceDef => {
                        definition_name(definition)
                    }
                    rayc_symbol::symbol_kind::SymbolKind::ExternDef => {
                        self.engine.get_name(definition.def_id()).await.to_string()
                    }
                    rayc_symbol::symbol_kind::SymbolKind::Effect
                    | rayc_symbol::symbol_kind::SymbolKind::EffectOperation
                    | rayc_symbol::symbol_kind::SymbolKind::Instance
                    | rayc_symbol::symbol_kind::SymbolKind::Marker
                    | rayc_symbol::symbol_kind::SymbolKind::MarkerImplementation
                    | rayc_symbol::symbol_kind::SymbolKind::Module
                    | rayc_symbol::symbol_kind::SymbolKind::Strut
                    | rayc_symbol::symbol_kind::SymbolKind::Trait
                    | rayc_symbol::symbol_kind::SymbolKind::TraitType
                    | rayc_symbol::symbol_kind::SymbolKind::InstanceType
                    | rayc_symbol::symbol_kind::SymbolKind::TraitDef => {
                        panic!("non-callable symbol reached a MonoIR function operand")
                    }
                }
            }
        }
    }

    pub(super) const fn engine(&self) -> &TrackedEngine { self.engine }

    fn finish(self) -> String {
        let mut output = String::from(
            "/* This file is auto generated by the Ray compiler. */\n#include \
             <stdbool.h>\n#include <stdint.h>\n#include <math.h>\n\n",
        );
        append_section(
            &mut output,
            "Aggregate type forward declarations",
            &self.buffers.type_forward_declarations,
        );
        append_section(
            &mut output,
            "Aggregate type definitions",
            &self.buffers.aggregate_definitions,
        );
        append_section(
            &mut output,
            "Function forward declarations",
            &self.buffers.function_forward_declarations,
        );
        append_section(&mut output, "Function definitions", &self.buffers.function_definitions);
        output
    }
}

fn by_value_dependencies(aggregate: &AggregateType) -> Vec<AggregateType> {
    fn collect_operand<'a>(
        iter: impl IntoIterator<Item = &'a Interned<MonoType>>,
    ) -> Vec<AggregateType> {
        iter.into_iter()
            .filter_map(|ty| match &**ty {
                MonoType::Aggregate(aggregate) => Some(aggregate.clone()),
                MonoType::Bool
                | MonoType::Int32
                | MonoType::Float32
                | MonoType::CInt
                | MonoType::CStr
                | MonoType::OpaquePointer(_)
                | MonoType::Pointer(_)
                | MonoType::FunctionPointer(_) => None,
            })
            .collect()
    }

    match aggregate {
        AggregateType::EffectHandler(_) => Vec::new(),
        AggregateType::Tuple(tuple) => collect_operand(tuple.fields()),
        AggregateType::Environment(environment) => collect_operand(environment.captures()),
        AggregateType::Struct(st) => collect_operand(st.fields().values()),
    }
}

fn append_section(output: &mut String, heading: &str, contents: &str) {
    writeln!(output, "/* {heading} */").unwrap();
    output.push_str(contents);
    if !contents.ends_with('\n') {
        output.push('\n');
    }
    output.push('\n');
}

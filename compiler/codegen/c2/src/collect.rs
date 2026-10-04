//! Discovery of everything a `MonoIR` fragment depends on.

use rayc_mono_ir::{
    MonoIR,
    cfg::Terminator,
    function::MonoFunctionID,
    instance::FunctionReference,
    instruction::Instruction,
    operand::{Constant, FunctionOperand, Operand},
    rvalue::{AggregateValue, Rvalue},
    ty::{AggregateType, EffectHandler},
};

use crate::{
    aggregates::AggregateRegistry, functions::FunctionRegistry, worklist::FragmentWorklist,
};

/// Walks a fragment's functions, scheduling the fragments they call and
/// registering the aggregates and cross-fragment interfaces they use.
///
/// Discovery is synchronous; facts that need engine queries (handler layouts,
/// global linkage) are resolved afterwards by the registries.
#[derive(Debug)]
pub(crate) struct DependencyCollector<'a> {
    ir: &'a MonoIR,
    fragments: &'a mut FragmentWorklist,
    functions: &'a mut FunctionRegistry,
    aggregates: &'a mut AggregateRegistry,
}

impl<'a> DependencyCollector<'a> {
    pub(crate) const fn new(
        ir: &'a MonoIR,
        fragments: &'a mut FragmentWorklist,
        functions: &'a mut FunctionRegistry,
        aggregates: &'a mut AggregateRegistry,
    ) -> Self {
        Self { ir, fragments, functions, aggregates }
    }

    /// Collects the dependencies of one function of the fragment.
    pub(crate) fn collect_function(&mut self, function_id: MonoFunctionID) {
        let function = self.ir.get_function(function_id);

        // The function's own interface.
        if let Some(closure) = self.ir.closure_instance(function_id) {
            self.functions.record_closure_body(closure, function.signature());
        }
        self.aggregates.visit_signature(function.signature());
        for (_, local) in function.locals() {
            self.aggregates.visit_type(local.ty());
        }

        // The body, in block order so that newly discovered fragments are
        // scheduled deterministically.
        let mut blocks = function.blocks().collect::<Vec<_>>();
        blocks.sort_unstable_by_key(|(block_id, _)| *block_id);
        for (_, block) in blocks {
            for instruction in block.instructions() {
                self.collect_instruction(instruction);
            }
            let terminator =
                block.terminator().expect("reachable MonoIR block should have a terminator");
            self.collect_terminator(terminator);
        }
    }

    fn collect_instruction(&mut self, instruction: &Instruction) {
        match instruction {
            Instruction::Assign(assign) => self.collect_rvalue(assign.value()),
            Instruction::Call(call) => {
                self.collect_operand(call.callee());
                call.arguments().iter().for_each(|argument| self.collect_operand(argument));
            }
        }
    }

    fn collect_terminator(&mut self, terminator: &Terminator) {
        match terminator {
            Terminator::Goto(_) | Terminator::Unreachable | Terminator::Return(None) => {}
            Terminator::Branch(branch) => self.collect_operand(branch.condition()),
            Terminator::Return(Some(value)) => self.collect_operand(value),
        }
    }

    fn collect_rvalue(&mut self, value: &Rvalue) {
        match value {
            Rvalue::Use(operand) => self.collect_operand(operand),
            Rvalue::AddressOf(_) => {}
            Rvalue::Unary(unary) => self.collect_operand(unary.operand()),
            Rvalue::Binary(binary) => {
                self.collect_operand(binary.left());
                self.collect_operand(binary.right());
            }
            Rvalue::Cast(cast) => {
                self.collect_operand(cast.operand());
                self.aggregates.visit_type(cast.target());
            }
            Rvalue::Aggregate(aggregate) => self.collect_aggregate_value(aggregate),
        }
    }

    fn collect_aggregate_value(&mut self, value: &AggregateValue) {
        match value {
            AggregateValue::Tuple(tuple) => {
                self.aggregates.insert(AggregateType::Tuple(tuple.ty().clone()));
                tuple.fields().iter().for_each(|field| self.collect_operand(field));
            }
            AggregateValue::Environment(environment) => {
                self.aggregates.insert(AggregateType::Environment(environment.ty().clone()));
                environment.fields().iter().for_each(|field| self.collect_operand(field));
            }
            AggregateValue::EffectHandler(handler) => {
                let ty = AggregateType::EffectHandler(EffectHandler::new(handler.effect().clone()));
                self.aggregates.insert(ty);
                for slot in handler.slots().values() {
                    self.collect_operand(slot.environment());
                    self.collect_operand(slot.function());
                }
            }
            AggregateValue::Struct(st) => {
                self.aggregates.insert(AggregateType::Struct(st.ty().clone()));
                st.fields().values().for_each(|field| self.collect_operand(field));
            }
        }
    }

    fn collect_operand(&mut self, operand: &Operand) {
        match operand {
            Operand::Constant(Constant::NullPointer(ty)) => self.aggregates.visit_type(ty),
            Operand::Copy(_)
            | Operand::Constant(
                Constant::Unit
                | Constant::Bool(_)
                | Constant::Int8(_)
                | Constant::Int16(_)
                | Constant::Int32(_)
                | Constant::Int64(_)
                | Constant::Isize(_)
                | Constant::Uint8(_)
                | Constant::Uint16(_)
                | Constant::Uint32(_)
                | Constant::Uint64(_)
                | Constant::Usize(_)
                | Constant::Float32(_)
                | Constant::CInt(_)
                | Constant::CStr(_),
            ) => {}
            Operand::Function(function) => self.collect_function_operand(function),
        }
    }

    fn collect_function_operand(&mut self, operand: &FunctionOperand) {
        let signature = operand.signature();
        self.aggregates.visit_signature(signature);

        match operand.function() {
            FunctionReference::Local(function_id) => {
                assert_eq!(signature, self.ir.get_function(*function_id).signature());
                if let Some(closure) = self.ir.closure_instance(*function_id) {
                    self.functions.record_closure_reference(&closure, signature);
                }
            }
            FunctionReference::Closure(closure) => {
                self.fragments.insert(closure.owner().clone());
                self.functions.record_closure_reference(closure, signature);
            }
            FunctionReference::NominalDrop(instance) => self.fragments.insert(instance.clone()),
            FunctionReference::Global(instance) => {
                self.functions.record_global(instance, signature);
                self.fragments.insert(instance.clone());
            }
        }
    }
}

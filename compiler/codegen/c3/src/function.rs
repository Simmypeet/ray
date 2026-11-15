//! Generating one function: the unit of work that the coordinator schedules.
//!
//! [`generate_function`] reads nothing but the engine and returns everything
//! it found as a [`FunctionOutput`]. It never touches state shared with other
//! functions, which is what later lets functions be generated in parallel.

use rayc_mono_ir::{
    MonoFunctionInstance,
    cfg::Terminator,
    function::{MonoFunction, get_mono_function},
    instruction::{Call, Instruction},
    operand::Operand,
    rvalue::Rvalue,
};
use rayc_qbice::TrackedEngine;
use rayc_symbol::name::get_name;

use crate::c::name::{FunctionName, LinkName};

/// What generating one function produced.
#[derive(Debug)]
pub(crate) struct FunctionOutput {
    instance: MonoFunctionInstance,
    /// The name the function is defined or declared under.
    name: LinkName,
    /// Every function the generated code refers to, in the order the
    /// references appear and possibly repeated.
    callees: Vec<MonoFunctionInstance>,
}

impl FunctionOutput {
    /// The function that was generated.
    pub(crate) const fn instance(&self) -> &MonoFunctionInstance { &self.instance }

    /// The name the function is defined or declared under.
    pub(crate) const fn name(&self) -> &LinkName { &self.name }

    /// The functions the generated code refers to, which must be generated
    /// too.
    pub(crate) fn callees(&self) -> impl Iterator<Item = &MonoFunctionInstance> {
        self.callees.iter()
    }
}

/// Generates the function `instance`.
pub(crate) async fn generate_function(
    engine: &TrackedEngine,
    instance: MonoFunctionInstance,
) -> FunctionOutput {
    match instance {
        // provided by the C environment: there is no body to lower, and so
        // nothing it refers to
        MonoFunctionInstance::ExternDef(def_id) => FunctionOutput {
            name: LinkName::Extern(engine.get_name(def_id).await),
            instance,
            callees: Vec::new(),
        },

        MonoFunctionInstance::Body(_)
        | MonoFunctionInstance::NominalDrop(_)
        | MonoFunctionInstance::CoroutineResume(_)
        | MonoFunctionInstance::CoroutineDrop(_) => {
            let function = engine.get_mono_function(instance).await;
            FunctionGenerator::new(&function).generate()
        }
    }
}

/// Walks the body of one lowered function.
///
/// For now it only records the functions the body refers to; it is the
/// skeleton that will print the body as C.
#[derive(Debug)]
struct FunctionGenerator<'f> {
    function: &'f MonoFunction,
    callees: Vec<MonoFunctionInstance>,
}

impl<'f> FunctionGenerator<'f> {
    const fn new(function: &'f MonoFunction) -> Self { Self { function, callees: Vec::new() } }

    fn generate(mut self) -> FunctionOutput {
        // every block, in arena order; which order does not matter, since only
        // the set of callees is observable
        for (_, block) in self.function.blocks() {
            for instruction in block.instructions() {
                self.visit_instruction(instruction);
            }
            let terminator =
                block.terminator().expect("reachable MonoFunction block should have a terminator");
            self.visit_terminator(terminator);
        }

        let instance = self.function.instance().clone();
        FunctionOutput {
            name: LinkName::Generated(FunctionName::of(&instance)),
            instance,
            callees: self.callees,
        }
    }

    fn visit_instruction(&mut self, instruction: &Instruction) {
        match instruction {
            Instruction::Assign(assign) => self.visit_rvalue(assign.value()),
            Instruction::Call(call) => self.visit_call(call),
            Instruction::HeapFree(pointer) => self.visit_operand(pointer),
        }
    }

    fn visit_call(&mut self, call: &Call) {
        self.visit_operand(call.callee());
        for argument in call.abi_arguments() {
            self.visit_operand(argument);
        }
    }

    fn visit_rvalue(&mut self, rvalue: &Rvalue) {
        match rvalue {
            Rvalue::Use(operand) => self.visit_operand(operand),
            Rvalue::Unary(unary) => self.visit_operand(unary.operand()),
            Rvalue::Binary(binary) => {
                self.visit_operand(binary.left());
                self.visit_operand(binary.right());
            }
            Rvalue::Cast(cast) => self.visit_operand(cast.operand()),
            Rvalue::Reinterpret(reinterpret) => self.visit_operand(reinterpret.operand()),

            // a place or a type, neither of which can name a function
            Rvalue::AddressOf(_) | Rvalue::HeapAllocate(_) => {}
        }
    }

    fn visit_terminator(&mut self, terminator: &Terminator) {
        match terminator {
            Terminator::Branch(branch) => self.visit_operand(branch.condition()),
            // the case values are constants, which cannot name a function
            Terminator::Switch(switch) => self.visit_operand(switch.discriminant()),
            Terminator::Return(Some(value)) => self.visit_operand(value),
            Terminator::Goto(_) | Terminator::Return(None) | Terminator::Unreachable => {}
        }
    }

    /// Records the function an operand names, the only way a body can refer
    /// to another function.
    fn visit_operand(&mut self, operand: &Operand) {
        match operand {
            Operand::Function(instance) => self.callees.push(instance.clone()),
            Operand::Copy(_) | Operand::Constant(_) => {}
        }
    }
}

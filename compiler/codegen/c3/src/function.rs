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
    signature::get_extern_def_signature,
    ty::AggregateType,
};
use rayc_qbice::TrackedEngine;
use rayc_symbol::{GlobalSymbolID, name::get_name};

use crate::{
    c::{
        name::{FunctionName, LinkName},
        ty::FunctionDeclaration,
    },
    context::FunctionContext,
};

/// What generating one function produced.
#[derive(Debug)]
pub(crate) struct FunctionOutput {
    instance: MonoFunctionInstance,
    /// The C declaration of the function, without the trailing `;`.
    declaration: String,
    /// Every function the generated code refers to, in the order the
    /// references appear and possibly repeated.
    callees: Vec<MonoFunctionInstance>,
    /// Every aggregate type the generated code mentions, closed under the
    /// types of their members.
    aggregates: Vec<AggregateType>,
}

impl FunctionOutput {
    pub(crate) const fn new(
        instance: MonoFunctionInstance,
        declaration: String,
        callees: Vec<MonoFunctionInstance>,
        aggregates: Vec<AggregateType>,
    ) -> Self {
        Self { instance, declaration, callees, aggregates }
    }

    /// The function that was generated.
    pub(crate) const fn instance(&self) -> &MonoFunctionInstance { &self.instance }

    /// The C declaration of the function, without the trailing `;`.
    pub(crate) fn declaration(&self) -> &str { &self.declaration }

    /// The functions the generated code refers to, which must be generated
    /// too.
    pub(crate) fn callees(&self) -> impl Iterator<Item = &MonoFunctionInstance> {
        self.callees.iter()
    }

    /// The aggregate types the generated code mentions, which must be
    /// declared, in no particular order.
    pub(crate) fn aggregates(&self) -> impl Iterator<Item = &AggregateType> {
        self.aggregates.iter()
    }
}

/// Generates the function `instance`.
pub(crate) async fn generate_function(
    engine: &TrackedEngine,
    instance: MonoFunctionInstance,
) -> FunctionOutput {
    match instance {
        MonoFunctionInstance::ExternDef(def_id) => declare_extern(engine, def_id).await,

        MonoFunctionInstance::Body(_)
        | MonoFunctionInstance::NominalDrop(_)
        | MonoFunctionInstance::CoroutineResume(_)
        | MonoFunctionInstance::CoroutineDrop(_) => {
            let function = engine.get_mono_function(instance).await;
            FunctionGenerator::new(engine, &function).generate().await
        }
    }
}

/// Declares an `extern def`, which the C environment provides: there is no
/// body to lower, so it refers to no function.
///
/// It is declared under its source name, through its declared signature, with
/// unnamed parameters, since it has no parameter locals.
async fn declare_extern(engine: &TrackedEngine, def_id: GlobalSymbolID) -> FunctionOutput {
    let mut context = FunctionContext::new(engine);
    let name = LinkName::Extern(engine.get_name(def_id).await);
    let signature = engine.get_extern_def_signature(def_id).await;

    context.use_signature(&signature).await;
    let declaration = FunctionDeclaration::new(&signature, &name, context.aggregate_registry());
    let declaration = format!("extern {declaration}");

    context.finish(MonoFunctionInstance::ExternDef(def_id), declaration)
}

/// Generates one lowered function.
///
/// For now it writes the declaration and walks the body only for the
/// functions it refers to; it is the skeleton that will print the body as C.
#[derive(Debug)]
struct FunctionGenerator<'f, 'engine> {
    context: FunctionContext<'engine>,
    function: &'f MonoFunction,
}

impl<'f, 'engine> FunctionGenerator<'f, 'engine> {
    fn new(engine: &'engine TrackedEngine, function: &'f MonoFunction) -> Self {
        Self { context: FunctionContext::new(engine), function }
    }

    async fn generate(mut self) -> FunctionOutput {
        let function = self.function;

        // the declaration: its types are registered before they are spelled
        self.context.use_signature(function.signature()).await;
        let name = LinkName::Generated(FunctionName::of(function.instance()));
        let declaration = FunctionDeclaration::new(
            function.signature(),
            &name,
            self.context.aggregate_registry(),
        )
        .with_parameters_of(function)
        .to_string();

        // the body, in arena order: which order does not matter, since only
        // the set of callees is observable so far
        for (_, block) in function.blocks() {
            for instruction in block.instructions() {
                self.visit_instruction(instruction);
            }
            let terminator =
                block.terminator().expect("reachable MonoFunction block should have a terminator");
            self.visit_terminator(terminator);
        }

        self.context.finish(function.instance().clone(), declaration)
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
            Operand::Function(instance) => self.context.use_function(instance),
            Operand::Copy(_) | Operand::Constant(_) => {}
        }
    }
}

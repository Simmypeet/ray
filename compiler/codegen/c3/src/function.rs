//! Generating one function: the unit of work that the coordinator schedules.
//!
//! [`generate_function`] reads nothing but the engine and returns everything
//! it found as a [`FunctionOutput`]. It never touches state shared with other
//! functions, which is what later lets functions be generated in parallel.

use rayc_mono_ir::{
    MonoFunctionInstance,
    function::{MonoFunction, get_mono_function},
    signature::get_extern_def_signature,
    ty::AggregateType,
};
use rayc_qbice::TrackedEngine;
use rayc_symbol::{GlobalSymbolID, name::get_name};

use crate::{
    c::{
        name::{FunctionName, LinkName},
        ty::FunctionDeclaration,
        writer::CWriter,
    },
    describe::describe_function,
    function_print::FunctionPrinter,
};

/// What generating one function produced.
#[derive(Debug)]
pub(crate) struct FunctionOutput {
    instance: MonoFunctionInstance,
    /// What the function is in Ray, such as `main.foo[int32]`.
    description: String,
    /// The C declaration of the function, without the trailing `;`.
    declaration: String,
    /// The C definition of the function, or `None` for an `extern def`,
    /// which the C environment defines.
    definition: Option<String>,
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
        description: String,
        declaration: String,
        definition: Option<String>,
        callees: Vec<MonoFunctionInstance>,
        aggregates: Vec<AggregateType>,
    ) -> Self {
        Self { instance, description, declaration, definition, callees, aggregates }
    }

    /// The function that was generated.
    pub(crate) const fn instance(&self) -> &MonoFunctionInstance { &self.instance }

    /// What the function is in Ray, such as `main.foo[int32]`.
    pub(crate) fn description(&self) -> &str { &self.description }

    /// The C declaration of the function, without the trailing `;`.
    pub(crate) fn declaration(&self) -> &str { &self.declaration }

    /// The C definition of the function, unless the C environment defines it.
    pub(crate) fn definition(&self) -> Option<&str> { self.definition.as_deref() }

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
///
/// The job owns everything it reads, so it can be spawned as a task of its
/// own; cloning the engine only clones a handle to it.
pub(crate) async fn generate_function(
    engine: TrackedEngine,
    instance: MonoFunctionInstance,
) -> FunctionOutput {
    let engine = &engine;
    let description = describe_function(engine, &instance).await;
    match instance {
        MonoFunctionInstance::ExternDef(def_id) => {
            declare_extern(engine, def_id, description).await
        }

        MonoFunctionInstance::Body(_)
        | MonoFunctionInstance::NominalDrop(_)
        | MonoFunctionInstance::CoroutineResume(_)
        | MonoFunctionInstance::CoroutineDrop(_) => {
            let function = engine.get_mono_function(instance).await;
            define(engine, &function, description).await
        }
    }
}

/// Declares an `extern def`, which the C environment provides: there is no
/// body to lower, so it refers to no function.
///
/// It is declared under its source name, through its declared signature, with
/// unnamed parameters, since it has no parameter locals.
async fn declare_extern(
    engine: &TrackedEngine,
    def_id: GlobalSymbolID,
    description: String,
) -> FunctionOutput {
    let name = LinkName::Extern(engine.get_name(def_id).await);
    let signature = engine.get_extern_def_signature(def_id).await;

    let mut writer = CWriter::new(engine);
    writer.push("extern ");
    writer.write(&FunctionDeclaration::new(&signature, &name)).await;
    let (declaration, aggregates) = writer.finish();

    let instance = MonoFunctionInstance::ExternDef(def_id);
    FunctionOutput::new(instance, description, declaration, None, Vec::new(), aggregates)
}

/// Declares and defines a lowered function.
async fn define(
    engine: &TrackedEngine,
    function: &MonoFunction,
    description: String,
) -> FunctionOutput {
    let mut writer = CWriter::new(engine);

    // the declaration, taken out of the writer, which keeps the aggregates it
    // spells
    let name = LinkName::Generated(FunctionName::of(function.instance()));
    let declaration = FunctionDeclaration::new(function.signature(), &name);
    writer.write(&declaration.with_parameters_of(function)).await;
    let declaration = writer.take_text();

    // the definition repeats the declaration as its head
    let mut printer = FunctionPrinter::new(writer, function);
    printer.print(&declaration).await;
    let (definition, callees, aggregates) = printer.finish();

    let instance = function.instance().clone();
    FunctionOutput::new(instance, description, declaration, Some(definition), callees, aggregates)
}

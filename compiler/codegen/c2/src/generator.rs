//! The worklist driver that turns `MonoFunction`s into a translation unit.

use rayc_mono_ir::{
    MonoBodyInstance, MonoFunctionInstance, function::MonoFunction,
    signature::get_extern_def_signature,
};
use rayc_symbol::{GlobalSymbolID, name::get_name};

use crate::{
    c::{
        name::{BodyName, FunctionName},
        ty::FunctionDeclaration,
    },
    print::FunctionPrinter,
    program::Program,
    unit::TranslationUnit,
    worklist::{FunctionWorklist, PendingFunction},
};

/// Generates C for every function reachable from a set of roots.
///
/// Functions are printed one at a time. Printing a function schedules the
/// functions it calls and records the aggregate types it uses; the aggregate
/// definitions are added last, once all of them are known.
#[derive(Debug)]
pub(crate) struct Generator<'engine> {
    program: Program<'engine>,
    unit: TranslationUnit,
    entry_point: Option<MonoBodyInstance>,
    /// A reusable buffer for the function being printed.
    definition: String,
}

impl<'engine> Generator<'engine> {
    /// Creates a generator starting from `roots`. Functions in `preloaded`
    /// are used as-is when reached instead of being lowered.
    pub(crate) fn new(
        engine: &'engine rayc_qbice::TrackedEngine,
        roots: impl IntoIterator<Item = impl Into<MonoFunctionInstance>>,
        preloaded: impl IntoIterator<Item = MonoFunction>,
        entry_point: Option<MonoBodyInstance>,
    ) -> Self {
        let mut functions = FunctionWorklist::with_preloaded(preloaded);

        // Roots are scheduled in a canonical order so the output does not
        // depend on the order the caller listed them in.
        let mut roots = roots.into_iter().map(Into::into).collect::<Vec<_>>();
        roots.sort_unstable();
        for root in roots {
            functions.insert(root);
        }

        Self {
            program: Program::new(engine, functions),
            unit: TranslationUnit::default(),
            entry_point,
            definition: String::new(),
        }
    }

    /// Processes functions until the worklist is exhausted.
    pub(crate) async fn generate(mut self) -> TranslationUnit {
        while let Some(function) = self.program.next_function() {
            self.process_function(function).await;
        }
        self.program.define_aggregates(&mut self.unit);

        if let Some(entry_point) = &self.entry_point {
            assert!(
                self.program.has_scheduled(&MonoFunctionInstance::Body(entry_point.clone())),
                "the executable entry point should be present in the function worklist"
            );
            self.unit.define_entry_point(BodyName::of(entry_point));
        }
        self.unit
    }

    async fn process_function(&mut self, function: PendingFunction) {
        let function = match function {
            PendingFunction::Lowered(function) => *function,
            PendingFunction::Unlowered(MonoFunctionInstance::ExternDef(def_id)) => {
                self.declare_extern(def_id).await;
                return;
            }
            PendingFunction::Unlowered(instance) => self.program.lower(instance).await,
        };
        self.print_function(&function).await;
    }

    /// Declares an `extern def` under its source name, through its declared
    /// signature.
    async fn declare_extern(&mut self, def_id: GlobalSymbolID) {
        let engine = self.program.engine();
        let name = engine.get_name(def_id).await;
        let signature = engine.get_extern_def_signature(def_id).await;
        self.program.use_signature(&signature).await;
        let name: &str = &name;
        let declaration = FunctionDeclaration::new(&signature, &name);
        self.unit.declare_function(format_args!("extern {declaration}"));
    }

    async fn print_function(&mut self, function: &MonoFunction) {
        let name = FunctionName::of(function.instance());

        self.definition.clear();
        FunctionPrinter::new(&mut self.program, function)
            .print(&mut self.definition, name)
            .await
            .expect("writing to a String cannot fail");
        self.unit.define_function(&self.definition);

        let declaration = FunctionDeclaration::new(function.signature(), &name);
        self.unit.declare_function(declaration.with_parameters_of(function));
    }
}

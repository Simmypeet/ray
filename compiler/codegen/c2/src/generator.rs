//! The worklist driver that turns `MonoIR` fragments into a translation unit.

use rayc_mono_ir::{MonoDefInstance, MonoFragmentInstance, MonoIR, signature::get_def_signature};

use crate::{
    c::{name::DefinitionName, ty::FunctionDeclaration},
    print::{FragmentFunctions, FunctionPrinter},
    program::{Linkage, Program},
    unit::TranslationUnit,
    worklist::{FragmentWorklist, PendingFragment},
};

/// Generates C for every fragment reachable from a set of roots.
///
/// Fragments are printed one at a time. Printing a fragment schedules the
/// fragments it calls and records the aggregate types it uses; the aggregate
/// definitions are added last, once all of them are known.
#[derive(Debug)]
pub(crate) struct Generator<'engine> {
    program: Program<'engine>,
    unit: TranslationUnit,
    entry_point: Option<MonoDefInstance>,
    /// A reusable buffer for the function being printed.
    definition: String,
}

impl<'engine> Generator<'engine> {
    /// Creates a generator starting from `roots`. Fragments in `preloaded`
    /// are used as-is when reached instead of being lowered.
    pub(crate) fn new(
        engine: &'engine rayc_qbice::TrackedEngine,
        roots: impl IntoIterator<Item = impl Into<MonoFragmentInstance>>,
        preloaded: impl IntoIterator<Item = MonoIR>,
        entry_point: Option<MonoDefInstance>,
    ) -> Self {
        let mut fragments = FragmentWorklist::with_preloaded(preloaded);

        // Roots are scheduled in a canonical order so the output does not
        // depend on the order the caller listed them in.
        let mut roots = roots.into_iter().map(Into::into).collect::<Vec<_>>();
        roots.sort_unstable();
        for root in roots {
            fragments.insert(root);
        }

        Self {
            program: Program::new(engine, fragments),
            unit: TranslationUnit::default(),
            entry_point,
            definition: String::new(),
        }
    }

    /// Processes fragments until the worklist is exhausted.
    pub(crate) async fn generate(mut self) -> TranslationUnit {
        while let Some(fragment) = self.program.next_fragment() {
            self.process_fragment(fragment).await;
        }
        self.program.define_aggregates(&mut self.unit);

        if let Some(entry_point) = &self.entry_point {
            assert!(
                self.program.has_scheduled(&MonoFragmentInstance::Definition(entry_point.clone())),
                "the executable entry point should be present in the definition worklist"
            );
            self.unit.define_entry_point(DefinitionName::of(entry_point));
        }
        self.unit
    }

    async fn process_fragment(&mut self, fragment: PendingFragment) {
        let ir = match fragment {
            PendingFragment::Lowered(ir) => ir,
            PendingFragment::Unlowered(MonoFragmentInstance::Definition(instance)) => {
                match self.program.linkage(&instance).await {
                    Linkage::Internal => {
                        self.program.lower(MonoFragmentInstance::Definition(instance)).await
                    }
                    Linkage::Extern(name) => {
                        self.declare_extern(instance, &name).await;
                        return;
                    }
                }
            }
            PendingFragment::Unlowered(fragment) => self.program.lower(fragment).await,
        };
        self.print_ir(&ir).await;
    }

    /// Declares an extern definition through its declared signature.
    async fn declare_extern(&mut self, instance: MonoDefInstance, name: &str) {
        let signature = self.program.engine().get_def_signature(instance).await;
        let signature = signature.signature();
        self.program.use_signature(signature).await;
        let declaration = FunctionDeclaration::new(signature, &name);
        self.unit.declare_function(format_args!("extern {declaration}"));
    }

    async fn print_ir(&mut self, ir: &MonoIR) {
        let fragment = FragmentFunctions::of(ir);
        for (function_id, name) in fragment.iter() {
            let function = ir.get_function(function_id);

            self.definition.clear();
            FunctionPrinter::new(&mut self.program, &fragment, function)
                .print(&mut self.definition, name)
                .await
                .expect("writing to a String cannot fail");
            self.unit.define_function(&self.definition);

            let declaration = FunctionDeclaration::new(function.signature(), &name);
            self.unit.declare_function(declaration.with_parameters_of(function));
        }
    }
}

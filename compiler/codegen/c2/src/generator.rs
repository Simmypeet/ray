//! The worklist driver that turns `MonoIR` fragments into a translation unit.

use rayc_mono_ir::{MonoDefInstance, MonoFragmentInstance, MonoIR};
use rayc_mono_ir_builder::{lower_ir, lower_nominal_drop};
use rayc_qbice::TrackedEngine;

use crate::{
    aggregates::AggregateRegistry,
    c::{name::DefinitionName, ty::SignatureDeclaration},
    collect::DependencyCollector,
    functions::{FunctionRegistry, Linkage},
    print::{FragmentFunctions, FunctionPrinter},
    unit::TranslationUnit,
    worklist::{FragmentWorklist, PendingFragment},
};

/// Generates C for every fragment reachable from a set of roots.
///
/// Each fragment goes through three phases:
///
/// 1. **collect** — walk its functions, scheduling called fragments and
///    discovering used aggregate types and cross-fragment interfaces;
/// 2. **resolve** — run the engine queries those discoveries need (handler
///    layouts, global linkage);
/// 3. **print** — write its declarations and definitions synchronously.
#[derive(Debug)]
pub(crate) struct Generator<'engine> {
    engine: &'engine TrackedEngine,
    fragments: FragmentWorklist,
    functions: FunctionRegistry,
    aggregates: AggregateRegistry,
    unit: TranslationUnit,
    entry_point: Option<MonoDefInstance>,
}

impl<'engine> Generator<'engine> {
    /// Creates a generator starting from `roots`. Fragments in `preloaded`
    /// are used as-is when reached instead of being lowered.
    pub(crate) fn new(
        engine: &'engine TrackedEngine,
        roots: impl IntoIterator<Item = impl Into<MonoFragmentInstance>>,
        preloaded: impl IntoIterator<Item = MonoIR>,
        entry_point: Option<MonoDefInstance>,
    ) -> Self {
        let mut fragments = FragmentWorklist::with_preloaded(preloaded);
        let functions = FunctionRegistry::with_internal(fragments.preloaded_definitions());

        // Roots are scheduled in a canonical order so the output does not
        // depend on the order the caller listed them in.
        let mut roots = roots.into_iter().map(Into::into).collect::<Vec<_>>();
        roots.sort_unstable();
        for root in roots {
            fragments.insert(root);
        }

        Self {
            engine,
            fragments,
            functions,
            aggregates: AggregateRegistry::default(),
            unit: TranslationUnit::default(),
            entry_point,
        }
    }

    /// Processes fragments until the worklist is exhausted.
    pub(crate) async fn generate(mut self) -> TranslationUnit {
        while let Some(fragment) = self.fragments.pop() {
            self.process_fragment(fragment).await;
        }

        // An extern declaration processed last may still have discovered
        // aggregates through its signature.
        self.aggregates.resolve_pending(self.engine).await;
        self.functions.assert_closures_have_bodies();
        self.unit.define_aggregates(&self.aggregates);

        if let Some(entry_point) = &self.entry_point {
            assert!(
                self.fragments.contains(&MonoFragmentInstance::Definition(entry_point.clone())),
                "the executable entry point should be present in the definition worklist"
            );
            self.unit.define_entry_point(DefinitionName::of(entry_point));
        }
        self.unit
    }

    async fn process_fragment(&mut self, fragment: PendingFragment) {
        let engine = self.engine;
        let ir = match fragment {
            PendingFragment::Lowered(ir) => ir,
            PendingFragment::Unlowered(MonoFragmentInstance::NominalDrop(instance)) => {
                lower_nominal_drop(engine, instance).await
            }
            PendingFragment::Unlowered(MonoFragmentInstance::Definition(instance)) => {
                match self.functions.linkage(engine, &instance).await {
                    Linkage::Internal => {
                        lower_ir(engine, instance.def_id(), instance.substitution().clone()).await
                    }
                    Linkage::Extern(name) => {
                        self.declare_extern(&instance, &name);
                        return;
                    }
                }
            }
        };
        self.process_ir(&ir).await;
    }

    /// Declares an extern definition using the signature its call sites
    /// agreed on.
    fn declare_extern(&mut self, instance: &MonoDefInstance, name: &str) {
        let signature = self.functions.extern_signature(instance);
        self.aggregates.visit_signature(signature);
        let declaration = SignatureDeclaration::new(signature, &name);
        self.unit.declare_function(format_args!("extern {declaration}"));
    }

    async fn process_ir(&mut self, ir: &MonoIR) {
        let fragment = FragmentFunctions::of(ir);

        // Collect.
        let mut collector = DependencyCollector::new(
            ir,
            &mut self.fragments,
            &mut self.functions,
            &mut self.aggregates,
        );
        for (function_id, _) in fragment.iter() {
            collector.collect_function(function_id);
        }

        // Resolve.
        self.aggregates.resolve_pending(self.engine).await;
        self.functions.resolve_linkages(self.engine).await;

        // Print.
        for (function_id, name) in fragment.iter() {
            let function = ir.get_function(function_id);
            let signature = SignatureDeclaration::new(function.signature(), &name);
            self.unit.declare_function(signature.with_parameters_of(function));

            let printer =
                FunctionPrinter::new(function, &fragment, &self.functions, &self.aggregates);
            self.unit.define_function(|out| printer.print(out, name));
        }
    }
}

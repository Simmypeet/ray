//! What generated code may refer to, discovered while it is printed.

use qbice::storage::intern::Interned;
use rayc_mono_ir::{
    MonoDefInstance, MonoFragmentInstance, MonoIR,
    instance::FunctionReference,
    ty::{AggregateType, FunctionSignature, MonoType, Tuple},
};
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    name::get_name,
    symbol_kind::{SymbolKind, get_symbol_kind},
};

use crate::{
    aggregates::AggregateRegistry,
    unit::TranslationUnit,
    worklist::{FragmentWorklist, PendingFragment},
};

/// How a global definition instance is provided to the translation unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Linkage {
    /// Generated in this translation unit from its `MonoIR` body.
    Internal,
    /// Provided by the C environment under its unmangled source name.
    Extern(Interned<str>),
}

impl Linkage {
    async fn of_symbol(engine: &TrackedEngine, instance: &MonoDefInstance) -> Self {
        let def_id = instance.def_id();
        match engine.get_symbol_kind(def_id).await {
            SymbolKind::Def | SymbolKind::InstanceDef => Self::Internal,
            SymbolKind::ExternDef => Self::Extern(engine.get_name(def_id).await),

            SymbolKind::Effect
            | SymbolKind::EffectOperation
            | SymbolKind::Instance
            | SymbolKind::Marker
            | SymbolKind::MarkerImplementation
            | SymbolKind::Module
            | SymbolKind::Strut
            | SymbolKind::Trait
            | SymbolKind::TraitType
            | SymbolKind::InstanceType
            | SymbolKind::TraitDef => {
                panic!("non-callable symbol reached C code generation")
            }
        }
    }
}

/// The program-wide state shared by every fragment: the engine, the
/// fragments still to generate, and the aggregate types used so far.
///
/// Everything else (signatures, linkage, handler layouts) is derived from
/// engine queries on demand, which the engine caches.
#[derive(Debug)]
pub(crate) struct Program<'engine> {
    engine: &'engine TrackedEngine,
    fragments: FragmentWorklist,
    aggregates: AggregateRegistry,
    /// The empty tuple, which is the type of every unit constant.
    unit: AggregateType,
}

impl<'engine> Program<'engine> {
    pub(crate) fn new(engine: &'engine TrackedEngine, fragments: FragmentWorklist) -> Self {
        let unit = AggregateType::Tuple(Tuple::new(engine.intern_unsized(Vec::new())));
        Self { engine, fragments, aggregates: AggregateRegistry::default(), unit }
    }

    pub(crate) const fn engine(&self) -> &'engine TrackedEngine { self.engine }

    /// The type of every unit constant.
    pub(crate) const fn unit_type(&self) -> &AggregateType { &self.unit }

    /// Takes the next fragment to generate.
    pub(crate) fn next_fragment(&mut self) -> Option<PendingFragment> { self.fragments.pop() }

    /// Whether `fragment` has ever been scheduled.
    pub(crate) fn has_scheduled(&self, fragment: &MonoFragmentInstance) -> bool {
        self.fragments.contains(fragment)
    }

    /// Records that generated code refers to `ty`.
    pub(crate) async fn use_type(&mut self, ty: &MonoType) {
        self.aggregates.insert_type(self.engine, ty).await;
    }

    /// Records that generated code refers to every type in `signature`.
    pub(crate) async fn use_signature(&mut self, signature: &FunctionSignature) {
        self.aggregates.insert_signature(self.engine, signature).await;
    }

    /// Records that generated code constructs `aggregate`.
    pub(crate) async fn use_aggregate(&mut self, aggregate: AggregateType) {
        self.aggregates.insert(self.engine, aggregate).await;
    }

    /// Records that generated code calls or takes the address of the
    /// function, scheduling the fragment that defines it.
    pub(crate) fn use_function(&mut self, reference: &FunctionReference) {
        match reference {
            // Defined by the fragment being printed.
            FunctionReference::Local(_) => {}
            FunctionReference::Global(instance) => self.fragments.insert(instance.clone()),
            FunctionReference::Closure(closure) => self.fragments.insert(closure.owner().clone()),
            FunctionReference::NominalDrop(instance) => self.fragments.insert(instance.clone()),
        }
    }

    /// How a global definition is provided.
    pub(crate) async fn linkage(&self, instance: &MonoDefInstance) -> Linkage {
        // Supplied bodies are generated regardless of their symbol, which
        // lets tests provide fragments for symbols the engine does not know.
        if self.fragments.is_preloaded_definition(instance) {
            return Linkage::Internal;
        }
        Linkage::of_symbol(self.engine, instance).await
    }

    /// Adds every used aggregate to `unit`.
    pub(crate) fn define_aggregates(&self, unit: &mut TranslationUnit) {
        unit.define_aggregates(&self.aggregates);
    }

    /// Lowers the body of an internal definition fragment.
    pub(crate) async fn lower(&self, fragment: MonoFragmentInstance) -> MonoIR {
        match fragment {
            MonoFragmentInstance::Definition(instance) => {
                rayc_mono_ir_builder::lower_ir(
                    self.engine,
                    instance.def_id(),
                    instance.substitution().clone(),
                )
                .await
            }
            MonoFragmentInstance::NominalDrop(instance) => {
                rayc_mono_ir_builder::lower_nominal_drop(self.engine, instance).await
            }
        }
    }
}

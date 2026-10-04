//! Interfaces of functions referenced across fragment boundaries.

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_mono_ir::{MonoClosureInstance, MonoDefInstance, ty::FunctionSignature};
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    name::get_name,
    symbol_kind::{SymbolKind, get_symbol_kind},
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
    /// Determines the linkage of `instance` from its symbol kind.
    async fn query(engine: &TrackedEngine, instance: &MonoDefInstance) -> Self {
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

#[derive(Debug)]
struct GlobalFunction {
    /// The concrete signature, known once some call site references it.
    signature: Option<FunctionSignature>,
    /// Resolved by [`FunctionRegistry::resolve_linkages`].
    linkage: Option<Linkage>,
}

#[derive(Debug)]
struct ClosureFunction {
    signature: FunctionSignature,
    /// Whether the owning fragment has emitted the closure's body.
    has_body: bool,
}

/// Tracks global definitions and source closures referenced by generated
/// code, checking that every reference agrees on one concrete signature.
#[derive(Debug, Default)]
pub(crate) struct FunctionRegistry {
    globals: FxHashMap<MonoDefInstance, GlobalFunction>,
    closures: FxHashMap<MonoClosureInstance, ClosureFunction>,
    /// Globals first referenced since the last linkage resolution.
    unresolved: Vec<MonoDefInstance>,
}

impl FunctionRegistry {
    /// Creates a registry where `internal` definitions are known to be
    /// generated without consulting their symbol kind.
    pub(crate) fn with_internal<'a>(
        internal: impl IntoIterator<Item = &'a MonoDefInstance>,
    ) -> Self {
        let globals = internal
            .into_iter()
            .map(|instance| {
                (instance.clone(), GlobalFunction {
                    signature: None,
                    linkage: Some(Linkage::Internal),
                })
            })
            .collect();
        Self { globals, ..Self::default() }
    }

    /// Records a reference to `instance` through `signature`.
    pub(crate) fn record_global(
        &mut self,
        instance: &MonoDefInstance,
        signature: &FunctionSignature,
    ) {
        let Some(global) = self.globals.get_mut(instance) else {
            let global = GlobalFunction { signature: Some(signature.clone()), linkage: None };
            self.globals.insert(instance.clone(), global);
            self.unresolved.push(instance.clone());
            return;
        };

        match &global.signature {
            Some(previous) => assert_eq!(
                previous, signature,
                "a global function instance should have one concrete signature"
            ),
            None => global.signature = Some(signature.clone()),
        }
    }

    /// Resolves the linkage of every global referenced since the last call,
    /// so that call sites can be named synchronously.
    pub(crate) async fn resolve_linkages(&mut self, engine: &TrackedEngine) {
        for instance in std::mem::take(&mut self.unresolved) {
            let linkage = Linkage::query(engine, &instance).await;
            let global = self.globals.get_mut(&instance).expect("unresolved global was recorded");
            global.linkage.get_or_insert(linkage);
        }
    }

    /// The linkage of `instance`, resolving it if no call site has done so.
    pub(crate) async fn linkage(
        &mut self,
        engine: &TrackedEngine,
        instance: &MonoDefInstance,
    ) -> Linkage {
        if let Some(linkage) = self.globals.get(instance).and_then(|global| global.linkage.as_ref())
        {
            return linkage.clone();
        }

        let linkage = Linkage::query(engine, instance).await;
        self.globals
            .entry(instance.clone())
            .or_insert(GlobalFunction { signature: None, linkage: None })
            .linkage = Some(linkage.clone());
        linkage
    }

    /// The resolved linkage of a referenced global.
    pub(crate) fn resolved_linkage(&self, instance: &MonoDefInstance) -> &Linkage {
        self.globals
            .get(instance)
            .and_then(|global| global.linkage.as_ref())
            .expect("referenced global function should have a resolved linkage")
    }

    /// The signature that call sites use for an extern definition, which has
    /// no body to take it from.
    pub(crate) fn extern_signature(&self, instance: &MonoDefInstance) -> &FunctionSignature {
        self.globals
            .get(instance)
            .and_then(|global| global.signature.as_ref())
            .expect("an extern definition requires a signature from a call site")
    }

    /// Records a reference to a closure body through `signature`.
    pub(crate) fn record_closure_reference(
        &mut self,
        closure: &MonoClosureInstance,
        signature: &FunctionSignature,
    ) {
        if let Some(existing) = self.closures.get(closure) {
            existing.assert_signature(signature);
        } else {
            self.closures.insert(closure.clone(), ClosureFunction::new(signature));
        }
    }

    /// Records that the owning fragment emitted `closure`'s body.
    pub(crate) fn record_closure_body(
        &mut self,
        closure: MonoClosureInstance,
        signature: &FunctionSignature,
    ) {
        let function =
            self.closures.entry(closure).or_insert_with(|| ClosureFunction::new(signature));
        function.assert_signature(signature);
        assert!(!function.has_body, "a closure body should be emitted once");
        function.has_body = true;
    }

    /// Checks that every referenced closure had its body emitted.
    pub(crate) fn assert_closures_have_bodies(&self) {
        assert!(
            self.closures.values().all(|closure| closure.has_body),
            "referenced nominal closure has no exported body"
        );
    }
}

impl ClosureFunction {
    fn new(signature: &FunctionSignature) -> Self {
        Self { signature: signature.clone(), has_body: false }
    }

    fn assert_signature(&self, signature: &FunctionSignature) {
        assert_eq!(&self.signature, signature, "nominal closure reference and body ABI must agree");
    }
}

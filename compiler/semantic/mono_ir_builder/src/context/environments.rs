//! The capture environments of a fragment's functions.

use rayc_arena::{Arena, ID};
use rayc_hash::FxHashMap;
use rayc_ir::{
    ir_closure::{CaptureID, CaptureMapID},
    ir_function::{IRContext, IRFunctionID, IRFunctionMap},
};
use rayc_mono_ir::{
    MonoEffectInstance,
    ty::{Environment as EnvironmentTy, MonoType},
};

use crate::resolver::Resolver;

/// How the hidden capture parameter is passed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum EnvironmentPassing {
    ErasedPointer(EnvironmentTy),
    ByValue(EnvironmentTy),
}

/// The concrete layout and passing convention of one capture environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnvironmentAbi {
    passing: EnvironmentPassing,
    capture_ids: Vec<CaptureID>,
    captured_effects: Vec<MonoEffectInstance>,
}

impl EnvironmentAbi {
    const fn new(
        passing: EnvironmentPassing,
        capture_ids: Vec<CaptureID>,
        captured_effects: Vec<MonoEffectInstance>,
    ) -> Self {
        Self { passing, capture_ids, captured_effects }
    }

    pub(crate) const fn by_value(&self) -> bool {
        matches!(self.passing, EnvironmentPassing::ByValue(_))
    }

    pub(crate) const fn environment_type(&self) -> &EnvironmentTy {
        match &self.passing {
            EnvironmentPassing::ErasedPointer(environment)
            | EnvironmentPassing::ByValue(environment) => environment,
        }
    }

    pub(crate) fn capture_ids(&self) -> impl ExactSizeIterator<Item = CaptureID> + '_ {
        self.capture_ids.iter().copied()
    }

    pub(crate) const fn capture_count(&self) -> usize { self.capture_ids.len() }

    pub(crate) fn captured_effects(&self) -> impl ExactSizeIterator<Item = &MonoEffectInstance> {
        self.captured_effects.iter()
    }
}

type EnvironmentAbiID = ID<EnvironmentAbi>;

/// Every environment a fragment reads, planned when it is created.
///
/// Environments are planned apart from function signatures: what creates a
/// closure only needs the closure's environment, and functions sharing a
/// capture map share one environment ABI.
pub(crate) struct Environments {
    abis: Arena<EnvironmentAbi>,
    environment_abi_ids_by_capture_map_id: FxHashMap<CaptureMapID, EnvironmentAbiID>,
    capture_map_ids_by_ir_function_id: FxHashMap<IRFunctionID, CaptureMapID>,
}

impl Environments {
    /// Plans the environment of each of `functions` that has one. A definition
    /// has none.
    pub(super) async fn new(
        resolver: &Resolver,
        source: &IRFunctionMap,
        functions: impl IntoIterator<Item = IRFunctionID>,
    ) -> Self {
        // Collect the capture maps first, each with one function that uses it,
        // so that functions sharing a capture map plan its environment once.
        let mut capture_map_ids_by_ir_function_id = FxHashMap::default();
        let mut planned_functions = FxHashMap::default();
        for function_id in functions {
            if let Some(capture_map_id) =
                source.get_function(function_id).context().capture_map_id()
            {
                capture_map_ids_by_ir_function_id.insert(function_id, capture_map_id);
                planned_functions.entry(capture_map_id).or_insert(function_id);
            }
        }

        // Plan the environment of every capture map.
        let mut abis = Arena::new();
        let mut environment_abi_ids_by_capture_map_id = FxHashMap::default();
        for (capture_map_id, function_id) in planned_functions {
            let environment = plan_environment(resolver, source, function_id).await;
            environment_abi_ids_by_capture_map_id.insert(capture_map_id, abis.insert(environment));
        }

        Self { abis, environment_abi_ids_by_capture_map_id, capture_map_ids_by_ir_function_id }
    }

    /// The environment of the function `function_id`.
    pub(crate) fn of_function(&self, function_id: IRFunctionID) -> &EnvironmentAbi {
        let capture_map_id = self
            .capture_map_ids_by_ir_function_id
            .get(&function_id)
            .expect("function should have a capture environment ABI");
        self.of_capture_map(*capture_map_id)
    }

    /// The environment that the functions of `capture_map_id` share.
    pub(crate) fn of_capture_map(&self, capture_map_id: CaptureMapID) -> &EnvironmentAbi {
        let environment_id = self
            .environment_abi_ids_by_capture_map_id
            .get(&capture_map_id)
            .expect("capture environment ABI should be planned");
        self.abis.get(*environment_id).expect("capture environment ABI should exist")
    }

    /// Whether the function `function_id` uses the environment of
    /// `capture_map_id`.
    pub(crate) fn function_uses_capture_map(
        &self,
        function_id: IRFunctionID,
        capture_map_id: CaptureMapID,
    ) -> bool {
        self.capture_map_ids_by_ir_function_id.get(&function_id) == Some(&capture_map_id)
    }
}

/// The environment of a function that has one. An operation handler's also
/// stores the handlers of its effects.
async fn plan_environment(
    resolver: &Resolver,
    source: &IRFunctionMap,
    function_id: IRFunctionID,
) -> EnvironmentAbi {
    let engine = resolver.engine();
    let captured_effects = match source.get_function(function_id).context() {
        IRContext::Def => panic!("a definition has no environment"),
        IRContext::Closure(_) | IRContext::Thunk(_) => Vec::new(),
        IRContext::OperationHandler(_) => {
            let effect = source.effect_of(function_id, engine).await;
            resolver.lower_effects(&effect).await
        }
    };

    let mut capture_ids = Vec::new();
    let mut fields = Vec::new();
    for (capture_id, capture) in source.captures(function_id) {
        capture_ids.push(capture_id);
        fields.push(resolver.lower_type(&capture.storage_ty(engine)).await);
    }
    fields.extend(
        captured_effects.iter().map(|effect| MonoType::new_handler_pointer(effect.clone(), engine)),
    );

    let environment = EnvironmentTy::new(engine.intern_unsized(fields));
    let passing = if source.is_closure_function(function_id) {
        EnvironmentPassing::ByValue(environment)
    } else {
        EnvironmentPassing::ErasedPointer(environment)
    };

    EnvironmentAbi::new(passing, capture_ids, captured_effects)
}

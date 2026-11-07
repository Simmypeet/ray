//! The capture environments of a fragment's functions.

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{
    ir_closure::{CaptureID, CaptureMapID},
    ir_function::{IRFunctionID, IRFunctionMap},
};
use rayc_mono_ir::{
    MonoDefInstance, MonoEnvironmentInstance,
    layout::{EnvironmentLayout, build_environment_layout},
    ty::{AggregateType, MonoType},
};

use crate::resolver::Resolver;

/// One environment that a fragment reads, with its layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnvironmentAbi {
    instance: MonoEnvironmentInstance,
    /// Whether the environment is a closure's, which is its value and is passed
    /// by value.
    by_value: bool,
    layout: Interned<EnvironmentLayout>,
    /// The captures in the order of the IR capture map, which is the order of
    /// the capture expressions that build the environment. The layout itself
    /// keeps no order.
    capture_ids: Vec<CaptureID>,
}

impl EnvironmentAbi {
    /// Whether the environment is passed by value, which only a closure's is.
    /// Any other is passed behind an opaque pointer.
    pub(crate) const fn by_value(&self) -> bool { self.by_value }

    pub(crate) const fn layout(&self) -> &Interned<EnvironmentLayout> { &self.layout }

    /// The type of the environment itself.
    pub(crate) fn environment_type(&self) -> MonoType {
        MonoType::Aggregate(AggregateType::Environment(self.instance.clone()))
    }

    /// The captures in the order of the capture expressions that build the
    /// environment.
    pub(crate) fn capture_ids(
        &self,
    ) -> impl ExactSizeIterator<Item = CaptureID> + DoubleEndedIterator + '_ {
        self.capture_ids.iter().copied()
    }
}

/// Every environment a fragment reads, planned when it is created.
///
/// Environments are planned apart from function signatures: what creates a
/// closure only needs the closure's environment, and functions sharing a
/// capture map share one environment.
pub(crate) struct Environments {
    abis_by_capture_map_id: FxHashMap<CaptureMapID, EnvironmentAbi>,
    capture_map_ids_by_ir_function_id: FxHashMap<IRFunctionID, CaptureMapID>,
}

impl Environments {
    /// Plans the environment of each of `functions` that has one. A definition
    /// has none.
    pub(super) async fn new(
        resolver: &Resolver,
        source: &IRFunctionMap,
        owner: &MonoDefInstance,
        functions: impl IntoIterator<Item = IRFunctionID>,
    ) -> Self {
        // Collect the capture maps first, each with its environment, so that
        // functions sharing a capture map plan its environment once.
        let mut capture_map_ids_by_ir_function_id = FxHashMap::default();
        let mut environment_instances_by_capture_map_id = FxHashMap::default();
        for function_id in functions {
            let Some(capture_map) = source.try_get_capture_map_id(function_id) else {
                continue;
            };

            capture_map_ids_by_ir_function_id.insert(function_id, capture_map);
            environment_instances_by_capture_map_id.entry(capture_map).or_insert_with(|| {
                let instance = MonoEnvironmentInstance::new(owner.clone(), capture_map);
                (instance, source.is_closure_function(function_id))
            });
        }

        // Read the layout of every environment.
        let mut abis_by_capture_map_id = FxHashMap::default();
        for (capture_map, (instance, by_value)) in environment_instances_by_capture_map_id {
            let layout = resolver.engine().build_environment_layout(instance.clone()).await;
            let capture_ids = source
                .get_capture_map(capture_map)
                .ordered_captures()
                .map(|(capture, _)| capture)
                .collect();
            abis_by_capture_map_id.insert(capture_map, EnvironmentAbi {
                instance,
                by_value,
                layout,
                capture_ids,
            });
        }

        Self { abis_by_capture_map_id, capture_map_ids_by_ir_function_id }
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
        self.abis_by_capture_map_id
            .get(&capture_map_id)
            .expect("capture environment ABI should be planned")
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

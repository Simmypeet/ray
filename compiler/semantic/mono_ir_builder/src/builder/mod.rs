use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{
    ir_function::{FunctionID as IRFunctionID, IRFunctionMap},
    ir_lambda::Capture,
};
use rayc_mono_ir::{
    MonoDefInstance, MonoEffectInstance, MonoIR,
    ty::{AggregateType, Environment, FunctionSignature, MonoType, lower_effects, lower_type},
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{
    parameter::{ParameterMap, get_parameter_map},
    return_type::get_return_type,
};
use rayc_type::ty::Ty;

use crate::function_abi::FunctionABI;

mod call;
mod cfg;
mod closure;
mod effect;
mod expression;
mod function;
mod state;

use state::FunctionState;

/// Coordinates lowering for one independently cacheable definition instance.
pub(crate) struct Builder {
    engine: TrackedEngine,
    instance: MonoDefInstance,
    source: Interned<IRFunctionMap>,
    function_abis: FxHashMap<IRFunctionID, FunctionABI>,
}

impl Builder {
    pub(crate) fn new(
        engine: TrackedEngine,
        instance: MonoDefInstance,
        source: Interned<IRFunctionMap>,
    ) -> Self {
        Self { engine, instance, source, function_abis: FxHashMap::default() }
    }

    pub(crate) async fn get_root_parameter_map(&self) -> Interned<ParameterMap> {
        self.engine.get_parameter_map(self.instance.def_id()).await
    }

    pub(crate) async fn get_root_return_type(&self) -> Interned<Ty> {
        self.engine.get_return_type(self.instance.def_id()).await
    }

    pub(crate) async fn lower_effects(&self, eff: &Interned<Ty>) -> Vec<MonoEffectInstance> {
        self.engine.lower_effects(eff, self.instance.substitution()).await
    }

    pub(crate) async fn lower_type(&self, ty: &Interned<Ty>) -> Interned<MonoType> {
        self.engine.lower_type(ty, self.instance.substitution()).await
    }

    pub(crate) fn create_function_signature(
        &self,
        parameter_types: impl IntoIterator<Item = Interned<MonoType>>,
        return_type: Interned<MonoType>,
    ) -> FunctionSignature {
        MonoType::new_function_signature(parameter_types, return_type, &self.engine)
    }

    pub(crate) fn create_aggregate_type_for_capture_environment(
        &self,
        fields: Vec<Interned<MonoType>>,
    ) -> Interned<MonoType> {
        let captures = self.engine.intern_unsized(fields);
        self.engine
            .intern(MonoType::Aggregate(AggregateType::Environment(Environment::new(captures))))
    }

    pub(crate) fn create_handler_pointer(&self, effect: MonoEffectInstance) -> Interned<MonoType> {
        MonoType::new_handler_pointer(effect, &self.engine)
    }

    pub(crate) fn create_opaque_pointer(&self) -> Interned<MonoType> {
        MonoType::new_opaque_pointer(&self.engine)
    }

    pub(crate) async fn lower_pointer_type_for_capture(&self, ty: &Capture) -> Interned<MonoType> {
        self.engine.lower_type(&ty.pointer_ty(&self.engine), self.instance.substitution()).await
    }

    pub(crate) async fn lower(mut self) -> MonoIR {
        let root_source_id = self.source.root_id();
        let root_source = self.source.root().clone();
        let root_abi = self.plan_function(&root_source).await;
        let mut output = MonoIR::new(self.instance.clone(), root_abi.signature().clone());
        self.function_abis.insert(root_source_id, root_abi);

        let mut source_functions = self.source.functions().map(|(id, _)| id).collect::<Vec<_>>();
        source_functions.sort_unstable();

        let mut source_to_target = FxHashMap::default();
        source_to_target.insert(root_source_id, output.root_id());

        for source_id in source_functions.iter().copied() {
            if source_id == root_source_id {
                continue;
            }
            let source = self.source.get_function(source_id).clone();
            let abi = self.plan_function(&source).await;
            let target = output.insert_function(abi.kind(), abi.signature().clone());
            self.function_abis.insert(source_id, abi);

            source_to_target.insert(source_id, target);
        }

        for source_id in source_functions {
            self.lower_function(source_id, source_to_target[&source_id], &mut output).await;
        }

        output
    }

    fn function_abi(&self, source_id: IRFunctionID) -> &FunctionABI {
        self.function_abis.get(&source_id).expect("nested MonoIR function ABI should be planned")
    }
}

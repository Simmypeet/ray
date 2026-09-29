use qbice::storage::intern::Interned;
use rayc_arena::Arena;
use rayc_hash::FxHashMap;
use rayc_ir::{
    ir_function::{FunctionID as IRFunctionID, IRFunction, IRFunctionMap},
    ir_lambda::{Capture, CaptureID, CaptureMapID},
};
use rayc_mono_ir::{
    MonoDefInstance, MonoEffectInstance, MonoIR,
    function::MonoFunctionID,
    ty::{
        AggregateType, EffectHandler, Environment, FunctionSignature, MonoType, PointerMutability,
        instantiate_effect,
    },
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{
    parameter::{ParameterMap, get_parameter_map},
    return_type::get_return_type,
};
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{subst::Subst, ty::Ty};

use crate::{
    builder::Builder,
    function_abi::{EnvironmentABI, EnvironmentABIID, FunctionABI},
    resolver::Resolver,
};

/// Long-lived context shared while lowering one definition instance.
pub(crate) struct Context {
    engine: TrackedEngine,
    instance: MonoDefInstance,
    resolver: Resolver,
    source: Interned<IRFunctionMap>,
    environment_abis: Arena<EnvironmentABI>,
    capture_environments: FxHashMap<CaptureMapID, EnvironmentABIID>,
    function_abis: FxHashMap<IRFunctionID, FunctionABI>,
    function_ids: FxHashMap<IRFunctionID, MonoFunctionID>,
}

impl Context {
    /// `solver` becomes the fragment's only solver, shared by every function
    /// lowered from `source`.
    pub(crate) fn new(
        solver: Solver,
        instance: MonoDefInstance,
        source: Interned<IRFunctionMap>,
    ) -> Self {
        let engine = solver.engine().clone();
        let resolver = Resolver::new(solver, instance.substitution().clone());
        Self {
            engine,
            instance,
            resolver,
            source,
            environment_abis: Arena::new(),
            capture_environments: FxHashMap::default(),
            function_abis: FxHashMap::default(),
            function_ids: FxHashMap::default(),
        }
    }

    /// Dictionary and callee resolution in this definition's substitution.
    pub(crate) const fn resolver(&self) -> &Resolver { &self.resolver }

    pub(crate) async fn get_root_parameter_map(&self) -> Interned<ParameterMap> {
        self.engine.get_parameter_map(self.instance.def_id()).await
    }

    pub(crate) async fn get_root_return_type(&self) -> Interned<Ty> {
        self.engine.get_return_type(self.instance.def_id()).await
    }

    pub(crate) async fn lower_effects(&self, effect: &Interned<Ty>) -> Vec<MonoEffectInstance> {
        self.resolver.lower_effects(effect).await
    }

    pub(crate) async fn lower_type(&self, ty: &Interned<Ty>) -> Interned<MonoType> {
        self.resolver.lower_type(ty).await
    }

    pub(crate) fn create_function_signature(
        &self,
        parameter_types: impl IntoIterator<Item = Interned<MonoType>>,
        return_type: Interned<MonoType>,
    ) -> FunctionSignature {
        MonoType::new_function_signature(parameter_types, return_type, &self.engine)
    }

    pub(crate) fn create_nominal_signature(
        &self,
        environment: Environment,
        parameters: Vec<Interned<MonoType>>,
        result: Interned<MonoType>,
        effects: &[MonoEffectInstance],
    ) -> FunctionSignature {
        rayc_mono_ir::ty::nominal_body_signature(
            &self.engine,
            environment,
            parameters,
            result,
            effects,
        )
    }

    pub(crate) fn create_aggregate_type_for_capture_environment(
        &self,
        fields: Vec<Interned<MonoType>>,
    ) -> Environment {
        let captures = self.engine.intern_unsized(fields);
        Environment::new(captures)
    }

    pub(crate) fn create_handler_pointer(&self, effect: MonoEffectInstance) -> Interned<MonoType> {
        MonoType::new_handler_pointer(effect, &self.engine)
    }

    pub(crate) fn create_opaque_pointer(&self) -> Interned<MonoType> {
        MonoType::new_opaque_pointer(&self.engine)
    }

    pub(crate) fn intern_environment_type(&self, environment: Environment) -> Interned<MonoType> {
        self.engine.intern(MonoType::Aggregate(AggregateType::Environment(environment)))
    }

    pub(crate) fn create_pointer(
        &self,
        pointee: Interned<MonoType>,
        mutability: PointerMutability,
    ) -> Interned<MonoType> {
        MonoType::new_pointer(pointee, mutability, &self.engine)
    }

    pub(crate) fn intern_effect_instance(
        &self,
        instance: MonoEffectInstance,
    ) -> Interned<MonoType> {
        self.engine
            .intern(MonoType::Aggregate(AggregateType::EffectHandler(EffectHandler::new(instance))))
    }

    pub(crate) async fn lower_capture_storage_type(&self, ty: &Capture) -> Interned<MonoType> {
        self.resolver.lower_type(&ty.storage_ty(&self.engine)).await
    }

    pub(crate) fn source_function(&self, source_id: IRFunctionID) -> IRFunction {
        self.source.get_function(source_id).clone()
    }

    /// Lowers the effect row of a source function.
    pub(crate) async fn lower_source_effects(
        &self,
        source_id: IRFunctionID,
    ) -> Vec<MonoEffectInstance> {
        let effect = self.source.effect_of(source_id, &self.engine).await;
        self.lower_effects(&effect).await
    }

    pub(crate) fn source_capture_count(&self, source_id: IRFunctionID) -> usize {
        self.source.captures(source_id).len()
    }

    pub(crate) fn source_capture_map_id(&self, source_id: IRFunctionID) -> CaptureMapID {
        self.source.capture_map_id(source_id)
    }

    pub(crate) fn source_captures(
        &self,
        source_id: IRFunctionID,
    ) -> impl ExactSizeIterator<Item = (CaptureID, &Capture)> {
        self.source.captures(source_id)
    }

    pub(crate) fn function_abi(&self, source_id: IRFunctionID) -> &FunctionABI {
        self.function_abis.get(&source_id).expect("nested MonoIR function ABI should be planned")
    }

    pub(crate) fn function_environment_abi(&self, source_id: IRFunctionID) -> &EnvironmentABI {
        let environment_id = self.function_abi(source_id).environment_id();
        self.environment_abis
            .get(environment_id)
            .expect("function environment ABI should be planned")
    }

    pub(crate) fn capture_environment_abi(&self, capture_map_id: CaptureMapID) -> &EnvironmentABI {
        let environment_id = self
            .capture_environments
            .get(&capture_map_id)
            .expect("capture environment ABI should be planned");
        self.environment_abis.get(*environment_id).expect("capture environment ABI should exist")
    }

    pub(crate) fn assert_function_uses_capture_environment(
        &self,
        source_id: IRFunctionID,
        capture_map_id: CaptureMapID,
    ) {
        let expected = self
            .capture_environments
            .get(&capture_map_id)
            .expect("capture environment ABI should be planned");
        assert_eq!(
            self.function_abi(source_id).environment_id(),
            *expected,
            "operation handlers in one handler record should share an environment ABI"
        );
    }

    pub(super) fn plan_capture_environment(
        &mut self,
        capture_map_id: CaptureMapID,
        environment: EnvironmentABI,
    ) -> EnvironmentABIID {
        if let Some(environment_id) = self.capture_environments.get(&capture_map_id).copied() {
            let planned = self
                .environment_abis
                .get(environment_id)
                .expect("capture environment ABI should exist");
            assert_eq!(
                planned, &environment,
                "functions sharing a capture map should share an environment ABI"
            );
            environment_id
        } else {
            let environment_id = self.environment_abis.insert(environment);
            assert!(self.capture_environments.insert(capture_map_id, environment_id).is_none());
            environment_id
        }
    }

    pub(crate) fn target_function_id(&self, source_id: IRFunctionID) -> MonoFunctionID {
        *self
            .function_ids
            .get(&source_id)
            .expect("semantic IR function should be mapped to a MonoIR function")
    }

    pub(crate) fn apply_owner_substitution(&self, substitution: &mut Subst) {
        substitution.compose(self.instance.substitution(), &self.engine);
    }

    pub(crate) async fn instantiate_effect(
        &self,
        effect_id: GlobalSymbolID,
        substitution: &Subst,
    ) -> MonoEffectInstance {
        self.engine.instantiate_effect(effect_id, substitution, self.instance.substitution()).await
    }

    pub(crate) async fn definition_instance(
        &self,
        def_id: GlobalSymbolID,
        substitution: Subst,
    ) -> MonoDefInstance {
        self.resolver.definition_instance(def_id, substitution).await
    }

    pub(crate) fn is_nominal(&self, source_id: IRFunctionID) -> bool {
        self.source.closures().any(|(_, function)| function == source_id)
    }

    pub(crate) async fn lower(mut self) -> MonoIR {
        let root_source_id = self.source.root_id();
        let root_source = self.source.root().clone();
        let root_abi = self.plan_function(root_source_id, &root_source).await;
        let source_ids = self.source.functions().map(|(id, _)| id).collect::<Vec<_>>();

        let mut output = MonoIR::new(self.instance.clone(), root_abi.signature().clone());

        self.function_abis.insert(root_source_id, root_abi);
        self.function_ids.insert(root_source_id, output.root_id());

        for source_id in source_ids.iter().copied() {
            if source_id == root_source_id {
                continue;
            }

            let source = self.source.get_function(source_id).clone();
            let abi = self.plan_function(source_id, &source).await;
            let target = output.insert_function(abi.kind(), abi.signature().clone());
            self.function_abis.insert(source_id, abi);
            self.function_ids.insert(source_id, target);
        }

        for (closure, source) in self.source.closures() {
            output.register_closure(closure, self.target_function_id(source));
        }

        for source_id in source_ids {
            let target_id = self.target_function_id(source_id);
            let mut builder = Builder::new(&mut output, target_id);
            builder.lower_function(&self, source_id).await;
        }

        output
    }
}

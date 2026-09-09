use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{
    ir_function::{FunctionID as IRFunctionID, IRFunction, IRFunctionMap},
    ir_lambda::Capture,
};
use rayc_mono_ir::{
    MonoDefInstance, MonoEffectInstance, MonoIR,
    function::MonoFunctionID,
    ty::{
        AggregateType, EffectHandler, Environment, FunctionSignature, MonoType, PointerMutability,
        ReturnType, instantiate_effect, lower_effects, lower_type,
    },
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{
    effect_row::get_effect_row,
    parameter::{ParameterMap, get_parameter_map},
    return_type::get_return_type,
};
use rayc_symbol::{
    GlobalSymbolID,
    member::get_member_by_name,
    name::get_name,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::is_variadic_def,
};
use rayc_type::{
    instance_member::get_instance_member,
    poly_var::{GlobalPolyVarID, build_subst_from_args, get_poly_var_map},
    subst::{Subst, Substitutable},
    ty::{Ty, application::View as ApplicationView, args::Args},
};

use crate::{builder::Builder, function_abi::FunctionABI};

/// Long-lived context shared while lowering one definition instance.
pub(crate) struct Context {
    engine: TrackedEngine,
    instance: MonoDefInstance,
    source: Interned<IRFunctionMap>,
    function_abis: FxHashMap<IRFunctionID, FunctionABI>,
    function_ids: FxHashMap<IRFunctionID, MonoFunctionID>,
}

impl Context {
    pub(crate) fn new(
        engine: TrackedEngine,
        instance: MonoDefInstance,
        source: Interned<IRFunctionMap>,
    ) -> Self {
        Self {
            engine,
            instance,
            source,
            function_abis: FxHashMap::default(),
            function_ids: FxHashMap::default(),
        }
    }

    pub(crate) async fn get_root_parameter_map(&self) -> Interned<ParameterMap> {
        self.engine.get_parameter_map(self.instance.def_id()).await
    }

    pub(crate) async fn get_root_return_type(&self) -> Interned<Ty> {
        self.engine.get_return_type(self.instance.def_id()).await
    }

    pub(crate) async fn lower_effects(&self, effect: &Interned<Ty>) -> Vec<MonoEffectInstance> {
        self.engine.lower_effects(effect, self.instance.substitution()).await
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

    pub(crate) async fn lower_pointer_type_for_capture(&self, ty: &Capture) -> Interned<MonoType> {
        self.engine.lower_type(&ty.pointer_ty(&self.engine), self.instance.substitution()).await
    }

    pub(crate) fn source_function(&self, source_id: IRFunctionID) -> IRFunction {
        self.source.get_function(source_id).clone()
    }

    pub(crate) fn function_abi(&self, source_id: IRFunctionID) -> &FunctionABI {
        self.function_abis.get(&source_id).expect("nested MonoIR function ABI should be planned")
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

    /// Resolves an abstract trait-method call to one concrete instance method.
    ///
    /// The semantic IR identifies the dictionary, the trait method, and the
    /// trait method's call-site substitution. `MonoIR` instead needs an
    /// ordinary global definition reference whose substitution is expressed
    /// entirely in the concrete instance and instance-method namespaces.
    pub(crate) async fn resolve_instance_call(
        &self,
        dictionary_ty: &Interned<Ty>,
        trait_def_id: GlobalSymbolID,
        trait_call_substitution: &Subst,
    ) -> MonoDefInstance {
        // TODO: this is a bit dense, there could be better ways to do this!

        // The dictionary may still be a polymorphic variable owned by the
        // function being monomorphized. Applying the owner's substitution turns
        // it into the concrete instance chosen at the caller, such as `EqInt`.
        let dictionary_ty =
            dictionary_ty.apply_subst_or_clone(self.instance.substitution(), &self.engine);
        let Ty::Application(application) = &*dictionary_ty else {
            panic!("an instance call should resolve to a concrete instance application")
        };
        let ApplicationView::Instance(instance) = application.view() else {
            panic!("an instance call should resolve to a type of instance kind")
        };

        // Trait and instance methods correspond by name. The InstanceMember query
        // verifies that correspondence and provides the precomputed mapping
        // from trait-owned polymorphic variables to instance-owned variables.
        let trait_def_name = self.engine.get_name(trait_def_id).await;
        let instance_def_id = self
            .engine
            .get_member_by_name(instance.symbol_id(), &trait_def_name)
            .await
            .expect("a concrete instance should implement the selected trait definition");
        let instance_def = self
            .engine
            .get_instance_member(instance_def_id)
            .await
            .expect("a checked method has a correspondence");
        assert_eq!(
            instance_def.trait_member_id(),
            trait_def_id,
            "the selected instance definition should implement the called trait definition"
        );

        // Seed the callee substitution with arguments belonging to the instance
        // declaration itself. For `SomeInstance[int32]`, this maps the instance's
        // type variables to `int32`.
        let instance_args = Args::new(instance.args().iter().cloned(), &self.engine);
        let mut substitution =
            self.engine.build_subst_from_args(instance.symbol_id(), &instance_args).await;

        // Method-local variables have different IDs in the trait declaration and
        // its instance implementation. First make the trait call substitution
        // concrete, then use InstanceMember's mapping to move each value
        // into the corresponding instance-method variable.
        let mut trait_call_substitution = trait_call_substitution.clone();
        self.apply_owner_substitution(&mut trait_call_substitution);
        let trait_def_poly_vars = self.engine.get_poly_var_map(trait_def_id).await;
        let local_substitution = trait_def_poly_vars
            .iter()
            .filter_map(|(trait_poly_var_id, _)| {
                let trait_poly_var_id = GlobalPolyVarID::new(trait_def_id, trait_poly_var_id);
                let concrete_ty = trait_call_substitution.get(&trait_poly_var_id)?;
                let instance_poly_var =
                    instance_def.poly_var_substitution().get(&trait_poly_var_id)?;
                let Ty::PolyVar(instance_poly_var_id) = &**instance_poly_var else {
                    panic!(
                        "a trait definition variable should map to an instance definition variable"
                    )
                };
                Some((*instance_poly_var_id, concrete_ty.clone()))
            })
            .collect();
        substitution.compose(&local_substitution, &self.engine);

        // From this point on, the call is indistinguishable from any other
        // global MonoIR call: a concrete definition ID plus its substitution.
        MonoDefInstance::new(instance_def_id, substitution)
    }

    pub(crate) fn instantiate_effect(
        &self,
        effect_id: GlobalSymbolID,
        substitution: &Subst,
    ) -> MonoEffectInstance {
        self.engine.instantiate_effect(effect_id, substitution, self.instance.substitution())
    }

    pub(crate) async fn global_signature(
        &self,
        function_id: GlobalSymbolID,
        substitution: &Subst,
    ) -> (FunctionSignature, Vec<MonoEffectInstance>, bool) {
        let parameters = self.engine.get_parameter_map(function_id).await;
        let mut parameter_types = Vec::new();
        for (_, parameter) in parameters.iter() {
            parameter_types.push(self.engine.lower_type(parameter.ty(), substitution).await);
        }
        let symbol_kind = self.engine.get_symbol_kind(function_id).await;
        let effects = if matches!(symbol_kind, SymbolKind::Def | SymbolKind::InstanceDef) {
            let effect = self.engine.get_effect_row(function_id).await;
            self.engine.lower_effects(&effect, substitution).await
        } else {
            Vec::new()
        };
        for effect in &effects {
            parameter_types.push(MonoType::new_handler_pointer(effect.clone(), &self.engine));
        }

        let return_type = self.engine.get_return_type(function_id).await;
        let return_type = self.engine.lower_type(&return_type, substitution).await;
        let is_void = symbol_kind == SymbolKind::ExternDef && return_type.is_unit();

        let return_type = if is_void {
            ReturnType::Void
        } else {
            ReturnType::Value(self.engine.intern_unsized([return_type]))
        };
        let is_variadic = if matches!(symbol_kind, SymbolKind::Def | SymbolKind::ExternDef) {
            self.engine.is_variadic_def(function_id).await
        } else {
            false
        };
        let parameter_types = self.engine.intern_unsized(parameter_types);
        let signature = if is_variadic {
            FunctionSignature::new_variadic(parameter_types, return_type)
        } else {
            FunctionSignature::new(parameter_types, return_type)
        };
        (signature, effects, is_void)
    }

    pub(crate) async fn lower(mut self) -> MonoIR {
        let root_source_id = self.source.root_id();
        let root_source = self.source.root().clone();
        let root_abi = self.plan_function(&root_source).await;
        let mut output = MonoIR::new(self.instance.clone(), root_abi.signature().clone());
        self.function_abis.insert(root_source_id, root_abi);
        self.function_ids.insert(root_source_id, output.root_id());

        let mut source_functions = self.source.functions().map(|(id, _)| id).collect::<Vec<_>>();
        source_functions.sort_unstable();

        for source_id in source_functions.iter().copied() {
            if source_id == root_source_id {
                continue;
            }
            let source = self.source.get_function(source_id).clone();
            let abi = self.plan_function(&source).await;
            let target = output.insert_function(abi.kind(), abi.signature().clone());
            self.function_abis.insert(source_id, abi);
            self.function_ids.insert(source_id, target);
        }

        for source_id in source_functions {
            let target_id = self.target_function_id(source_id);
            let mut builder = Builder::new(&mut output, source_id, target_id);
            builder.lower_function(&self, source_id).await;
        }

        output
    }
}

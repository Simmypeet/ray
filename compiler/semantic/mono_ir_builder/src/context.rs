use qbice::storage::intern::Interned;
use rayc_ir::{
    ir_closure::{Capture, CaptureID, CaptureMapID},
    ir_expr::handle::Handle,
    ir_function::{IRFunction, IRFunctionID, IRFunctionMap},
};
use rayc_mono_ir::{
    MonoClosureInstance, MonoCoroutineInstance, MonoDefInstance, MonoEffectInstance,
    MonoFragmentInstance, MonoIR, function::MonoFunctionID, signature::get_coroutine_signature,
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::parameter::{ParameterMap, get_parameter_map};
use rayc_solver::Solver;
use rayc_symbol::{GlobalSymbolID, parent::get_parent_global};
use rayc_type::subst::Subst;

use crate::{function_lowering::FunctionLowering, resolver::Resolver};

mod callback;
mod environments;

use callback::CallbackFragment;
pub(crate) use callback::CallbackStyleFunctionABI;
pub(crate) use environments::EnvironmentAbi;
use environments::Environments;

/// How the fragment being lowered performs its effects, and the state that
/// takes.
enum FragmentStyle {
    /// One function of the definition with the nested functions it creates,
    /// which perform their effects through handlers.
    Callback(CallbackFragment),

    /// One function of the definition, as the `coroutine_resume` of the
    /// fragment's instance.
    Coroutine { function_id: IRFunctionID },
}

/// The closures that the function `function_id` creates.
fn created_closures(
    source: &IRFunctionMap,
    function_id: IRFunctionID,
) -> impl Iterator<Item = IRFunctionID> + '_ {
    source
        .get_function(function_id)
        .created_functions()
        .filter(|created| source.is_closure_function(*created))
}

/// Long-lived context shared while lowering one fragment.
pub(crate) struct Context {
    engine: TrackedEngine,
    /// The fragment being lowered.
    instance: MonoFragmentInstance,
    /// The definition that owns the functions being lowered, whose substitution
    /// they are lowered in.
    owner: MonoDefInstance,
    resolver: Resolver,
    source: Interned<IRFunctionMap>,
    environments: Environments,
    style: FragmentStyle,
}

impl Context {
    /// A context for lowering the definition `instance` in callback style,
    /// with the fragment to lower it into.
    pub(crate) async fn new_definition(
        solver: Solver,
        instance: MonoDefInstance,
        source: Interned<IRFunctionMap>,
    ) -> (Self, MonoIR) {
        let root = source.root_id();
        Self::new_callback(solver, instance.clone(), source, instance.into(), root).await
    }

    /// A context for lowering the closure `instance` in callback style, with
    /// the fragment to lower it into.
    pub(crate) async fn new_closure(
        solver: Solver,
        instance: MonoClosureInstance,
        source: Interned<IRFunctionMap>,
    ) -> (Self, MonoIR) {
        let owner = instance.owner().clone();
        let root = source.get_function_id_of_closure(instance.closure_id());
        Self::new_callback(solver, owner, source, instance.into(), root).await
    }

    /// `solver` becomes the fragment's only solver, shared by every function
    /// lowered from `source`.
    async fn new_callback(
        solver: Solver,
        owner: MonoDefInstance,
        source: Interned<IRFunctionMap>,
        instance: MonoFragmentInstance,
        root: IRFunctionID,
    ) -> (Self, MonoIR) {
        let engine = solver.engine().clone();
        let resolver = Resolver::new(solver, owner.substitution().clone());

        let functions = callback::fragment_functions(&source, root);
        let environment_functions = callback::environment_functions(&source, &functions);
        let environments =
            Environments::new(&resolver, &source, environment_functions.iter().copied()).await;
        let (fragment, output) = CallbackFragment::new(
            &resolver,
            &source,
            &environments,
            owner.def_id(),
            instance.clone(),
            functions,
        )
        .await;

        let style = FragmentStyle::Callback(fragment);
        (Self { engine, instance, owner, resolver, source, environments, style }, output)
    }

    /// A context for lowering the function `function_id` of `owner` into the
    /// `coroutine_resume` of `instance`, with the fragment to lower it into.
    ///
    /// `solver` becomes the fragment's only solver.
    pub(crate) async fn new_coroutine(
        solver: Solver,
        owner: MonoDefInstance,
        source: Interned<IRFunctionMap>,
        instance: MonoCoroutineInstance,
        function_id: IRFunctionID,
    ) -> (Self, MonoIR) {
        let engine = solver.engine().clone();
        let resolver = Resolver::new(solver, owner.substitution().clone());

        let environment_functions = created_closures(&source, function_id).collect::<Vec<_>>();
        let environments =
            Environments::new(&resolver, &source, environment_functions.iter().copied()).await;

        let signature = engine.get_coroutine_signature(instance.clone()).await;
        let output = MonoIR::new(instance.clone(), signature.resume_signature().clone());

        let style = FragmentStyle::Coroutine { function_id };
        let instance = instance.into();
        (Self { engine, instance, owner, resolver, source, environments, style }, output)
    }

    fn callback(&self) -> &CallbackFragment {
        match &self.style {
            FragmentStyle::Callback(callback) => callback,
            FragmentStyle::Coroutine { .. } => {
                panic!("a coroutine fragment plans no function ABIs or target functions")
            }
        }
    }

    /// The definition that owns the functions being lowered.
    pub(crate) const fn def_id(&self) -> GlobalSymbolID { self.owner.def_id() }

    /// The effect of `operation_id`, one of the effects that `handle` handles,
    /// as an effect row of the owner would spell it.
    pub(crate) async fn lower_handled_effect(
        &self,
        handle: &Handle,
        operation_id: GlobalSymbolID,
    ) -> MonoEffectInstance {
        let effect_id = self
            .engine
            .get_parent_global(operation_id)
            .await
            .expect("an operation should belong to an effect");

        self.instantiate_effect(effect_id, handle.handled_effect(effect_id).substitution()).await
    }

    /// Dictionary and callee resolution in this definition's substitution.
    pub(crate) const fn resolver(&self) -> &Resolver { &self.resolver }

    pub(crate) async fn get_root_parameter_map(&self) -> Interned<ParameterMap> {
        self.engine.get_parameter_map(self.owner.def_id()).await
    }

    pub(crate) fn source_function(&self, source_id: IRFunctionID) -> &IRFunction {
        self.source.get_function(source_id)
    }

    pub(crate) fn source_capture_count(&self, source_id: IRFunctionID) -> usize {
        self.source.captures(source_id).len()
    }

    pub(crate) fn source_captures(
        &self,
        source_id: IRFunctionID,
    ) -> impl ExactSizeIterator<Item = (CaptureID, &Capture)> {
        self.source.captures(source_id)
    }

    pub(crate) fn function_abi(&self, source_id: IRFunctionID) -> &CallbackStyleFunctionABI {
        self.callback().function_abi(source_id)
    }

    pub(crate) fn function_environment_abi(&self, source_id: IRFunctionID) -> &EnvironmentAbi {
        self.environments.of_function(source_id)
    }

    pub(crate) fn capture_environment_abi(&self, capture_map_id: CaptureMapID) -> &EnvironmentAbi {
        self.environments.of_capture_map(capture_map_id)
    }

    pub(crate) fn assert_function_uses_capture_environment(
        &self,
        source_id: IRFunctionID,
        capture_map_id: CaptureMapID,
    ) {
        assert!(
            self.environments.function_uses_capture_map(source_id, capture_map_id),
            "operation handlers in one handler record should share an environment ABI"
        );
    }

    pub(crate) fn target_function_id(&self, source_id: IRFunctionID) -> MonoFunctionID {
        self.callback().target_function_id(source_id)
    }

    pub(crate) fn apply_owner_substitution(&self, substitution: &mut Subst) {
        substitution.compose(self.owner.substitution(), &self.engine);
    }

    pub(crate) async fn instantiate_effect(
        &self,
        effect_id: GlobalSymbolID,
        substitution: &Subst,
    ) -> MonoEffectInstance {
        self.resolver.instantiate_effect(effect_id, substitution, self.owner.substitution()).await
    }

    /// Lowers the fragment this context is for into `output`, which it was
    /// created with.
    pub(crate) async fn lower(self, output: MonoIR) -> MonoIR {
        match &self.style {
            FragmentStyle::Callback(_) => Box::pin(self.lower_callback(output)).await,
            FragmentStyle::Coroutine { function_id } => {
                let function_id = *function_id;
                Box::pin(self.lower_coroutine(output, function_id)).await
            }
        }
    }

    /// The coroutine of a fragment in coroutine style.
    fn coroutine_instance(&self) -> &MonoCoroutineInstance {
        match &self.instance {
            MonoFragmentInstance::CoroutineResume(instance) => instance,
            MonoFragmentInstance::Definition(_)
            | MonoFragmentInstance::Closure(_)
            | MonoFragmentInstance::NominalDrop(_) => {
                panic!("a fragment in coroutine style should be a `coroutine_resume`")
            }
        }
    }

    /// Lowers the function `source_id` into the `coroutine_resume` of the
    /// fragment's instance.
    async fn lower_coroutine(self, mut output: MonoIR, source_id: IRFunctionID) -> MonoIR {
        let target_id = output.root_id();
        let instance = self.coroutine_instance();
        FunctionLowering::new_coroutine(&self, &mut output, target_id, source_id, instance)
            .await
            .lower_coroutine()
            .await;

        output
    }

    /// Lowers one definition or closure into one fragment, with the nested
    /// functions it creates.
    async fn lower_callback(self, mut output: MonoIR) -> MonoIR {
        for source_id in self.callback().functions() {
            let target_id = self.target_function_id(source_id);
            FunctionLowering::new_callback(&self, &mut output, target_id, source_id)
                .await
                .lower_function()
                .await;
        }

        output
    }
}

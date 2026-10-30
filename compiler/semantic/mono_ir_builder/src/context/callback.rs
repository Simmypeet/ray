//! What only a fragment in callback style needs: the calling convention and
//! target of each of its functions.

use qbice::storage::intern::Interned;
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_ir::ir_function::{IRContext, IRFunction, IRFunctionID, IRFunctionMap};
use rayc_mono_ir::{
    MonoEffectInstance, MonoFragmentInstance, MonoIR,
    function::{MonoFunctionID, MonoFunctionKind},
    ty::{FunctionSignature, MonoType, closure_body_signature},
};
use rayc_semantic_element::parameter::get_parameter_map;
use rayc_symbol::GlobalSymbolID;
use rayc_type::signature::get_return_type;

use super::{created_closures, environments::Environments};
use crate::resolver::Resolver;

/// The concrete calling convention of one function in callback style.
#[derive(Debug, Clone)]
pub(crate) struct CallbackStyleFunctionABI {
    kind: MonoFunctionKind,
    signature: FunctionSignature,
    effects: Vec<MonoEffectInstance>,
}

impl CallbackStyleFunctionABI {
    pub(crate) fn effects(&self) -> impl ExactSizeIterator<Item = &MonoEffectInstance> {
        self.effects.iter()
    }

    pub(crate) fn captures_effect_handlers(&self) -> bool {
        self.kind == MonoFunctionKind::OperationHandler
    }
}

/// The functions of a callback-style fragment rooted at `root`, with the root
/// first.
pub(super) fn fragment_functions(source: &IRFunctionMap, root: IRFunctionID) -> Vec<IRFunctionID> {
    let mut functions = vec![root];
    functions.extend(nested_functions(source, root));
    functions
}

/// The functions whose environments lowering `functions` reads: their own, and
/// those of the closures they create, which are fragments of their own.
pub(super) fn environment_functions(
    source: &IRFunctionMap,
    functions: &[IRFunctionID],
) -> Vec<IRFunctionID> {
    let created = functions.iter().flat_map(|function| created_closures(source, *function));
    functions.iter().copied().chain(created).collect()
}

/// The functions nested in the fragment rooted at `root`: what it creates apart
/// from closures, and what those create in turn. They come in the order of the
/// source, so the output does not depend on the walk.
fn nested_functions(source: &IRFunctionMap, root: IRFunctionID) -> FxHashSet<IRFunctionID> {
    let mut nested = FxHashSet::default();
    let mut pending = vec![root];
    while let Some(function) = pending.pop() {
        for created in source.get_function(function).created_functions() {
            if !source.is_closure_function(created) && nested.insert(created) {
                pending.push(created);
            }
        }
    }

    nested
}

/// One definition or closure with the nested functions it creates, which refer
/// to each other, so all are planned before any is lowered.
pub(crate) struct CallbackFragment {
    functions: Vec<IRFunctionID>,
    function_abis: FxHashMap<IRFunctionID, CallbackStyleFunctionABI>,
    function_ids: FxHashMap<IRFunctionID, MonoFunctionID>,
}

impl CallbackFragment {
    /// Plans every one of `functions`, whose first is the root, and declares
    /// them in a new `MonoIR` for `instance`, which is returned with empty
    /// bodies.
    pub(super) async fn new(
        resolver: &Resolver,
        source: &IRFunctionMap,
        environments: &Environments,
        def_id: GlobalSymbolID,
        instance: MonoFragmentInstance,
        functions: Vec<IRFunctionID>,
    ) -> (Self, MonoIR) {
        let planner = FunctionPlanner { resolver, source, environments, def_id };
        let (root, nested) = functions.split_first().expect("a fragment should have a root");

        // Callers spell a definition through its declared signature, so the
        // body must implement exactly that interface.
        let root_abi = planner.plan_function(*root).await;
        if let MonoFragmentInstance::Definition(definition) = &instance {
            assert_eq!(
                &root_abi.signature,
                resolver.def_signature(definition).await.signature(),
                "a lowered definition should implement its declared signature"
            );
        }

        // Declare the root, then every nested function in order.
        let mut output = MonoIR::new(instance, root_abi.signature.clone());
        let mut function_abis = FxHashMap::default();
        let mut function_ids = FxHashMap::default();
        function_ids.insert(*root, output.root_id());
        function_abis.insert(*root, root_abi);
        for function in nested.iter().copied() {
            let abi = planner.plan_function(function).await;
            function_ids.insert(function, output.insert_function(abi.kind, abi.signature.clone()));
            function_abis.insert(function, abi);
        }

        (Self { functions, function_abis, function_ids }, output)
    }

    /// The functions of the fragment, with the root first.
    pub(crate) fn functions(&self) -> impl ExactSizeIterator<Item = IRFunctionID> + '_ {
        self.functions.iter().copied()
    }

    pub(crate) fn function_abi(&self, function_id: IRFunctionID) -> &CallbackStyleFunctionABI {
        self.function_abis.get(&function_id).expect("nested MonoIR function ABI should be planned")
    }

    pub(crate) fn target_function_id(&self, function_id: IRFunctionID) -> MonoFunctionID {
        *self
            .function_ids
            .get(&function_id)
            .expect("semantic IR function should be mapped to a MonoIR function")
    }
}

/// What the kind of a function decides about its calling convention.
struct FunctionShape {
    kind: MonoFunctionKind,
    parameter_types: Vec<Interned<MonoType>>,
    return_type: Interned<MonoType>,
}

/// What planning the calling convention of a function reads.
struct FunctionPlanner<'a> {
    resolver: &'a Resolver,
    source: &'a IRFunctionMap,
    environments: &'a Environments,
    def_id: GlobalSymbolID,
}

impl FunctionPlanner<'_> {
    /// Plans how a function is called.
    async fn plan_function(&self, function_id: IRFunctionID) -> CallbackStyleFunctionABI {
        let engine = self.resolver.engine();
        let effect = self.source.effect_of(function_id, engine).await;
        let effects = self.resolver.lower_effects(&effect).await;
        let FunctionShape { kind, mut parameter_types, return_type } =
            self.plan_shape(self.source.get_function(function_id)).await;

        // Nominal bodies and dictionary calls share the same
        // environment/handler ABI.
        let signature = if self.source.is_closure_function(function_id) {
            let environment = self.environments.of_function(function_id).environment_type().clone();
            closure_body_signature(engine, environment, parameter_types, return_type, &effects)
        } else {
            // Operation callbacks capture handlers instead of receiving them.
            if kind != MonoFunctionKind::OperationHandler {
                parameter_types.extend(
                    effects
                        .iter()
                        .map(|effect| MonoType::new_handler_pointer(effect.clone(), engine)),
                );
            }
            MonoType::new_function_signature(parameter_types, return_type, engine)
        };

        CallbackStyleFunctionABI { kind, signature, effects }
    }

    /// Reads the parameters and result of a function from what kind of function
    /// it is.
    async fn plan_shape(&self, source: &IRFunction) -> FunctionShape {
        let engine = self.resolver.engine();
        let resolver = self.resolver;

        match source.context() {
            IRContext::Def => {
                let parameters = engine.get_parameter_map(self.def_id).await;
                let mut parameter_types = Vec::new();
                for (_, parameter) in parameters.iter() {
                    parameter_types.push(resolver.lower_type(parameter.ty()).await);
                }
                let return_type = engine.get_return_type(self.def_id).await;

                FunctionShape {
                    kind: MonoFunctionKind::Def,
                    parameter_types,
                    return_type: resolver.lower_type(&return_type).await,
                }
            }

            // Every closure body uses the nominal environment ABI.
            IRContext::Closure(context) => {
                let mut parameter_types = Vec::new();
                for (_, parameter) in context.parameters() {
                    parameter_types.push(resolver.lower_type(parameter.ty()).await);
                }

                FunctionShape {
                    kind: MonoFunctionKind::Closure,
                    parameter_types,
                    return_type: resolver.lower_type(context.return_ty()).await,
                }
            }

            // The first parameter carries the erased capture environment.
            IRContext::Thunk(context) => FunctionShape {
                kind: MonoFunctionKind::Thunk,
                parameter_types: vec![MonoType::new_opaque_pointer(engine)],
                return_type: resolver.lower_type(context.return_ty()).await,
            },

            IRContext::OperationHandler(context) => {
                let mut parameter_types = vec![MonoType::new_opaque_pointer(engine)];
                for (_, parameter) in context.parameters() {
                    parameter_types.push(resolver.lower_type(parameter.ty()).await);
                }

                FunctionShape {
                    kind: MonoFunctionKind::OperationHandler,
                    parameter_types,
                    return_type: resolver.lower_type(context.return_ty()).await,
                }
            }
        }
    }
}

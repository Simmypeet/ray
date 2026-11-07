//! What only a fragment in callback style needs: the calling convention and
//! target of each of its functions.

use rayc_hash::{FxHashMap, FxHashSet};
use rayc_ir::ir_function::{IRFunctionID, IRFunctionMap};
use rayc_mono_ir::{
    MonoDefInstance, MonoEffectInstance, MonoFragmentInstance, MonoIR,
    function::{MonoFunctionID, MonoFunctionKind},
    signature::get_callback_function_signature,
    ty::FunctionSignature,
};

use super::created_closures;
use crate::resolver::Resolver;

/// The concrete calling convention of one function in callback style.
#[derive(Debug, Clone)]
pub(crate) struct CallbackStyleFunctionABI {
    kind: MonoFunctionKind,
    signature: FunctionSignature,
}

impl CallbackStyleFunctionABI {
    /// Reads how the function `function_id` of the IR of `owner` is called.
    async fn of(
        resolver: &Resolver,
        source: &IRFunctionMap,
        owner: &MonoDefInstance,
        function_id: IRFunctionID,
    ) -> Self {
        let kind =
            MonoFunctionKind::of_callback_function(source.get_function(function_id).context());
        let signature =
            resolver.engine().get_callback_function_signature(owner.clone(), function_id).await;

        Self { kind, signature }
    }

    /// The effects whose handler records a caller passes, in their order.
    pub(crate) fn handler_record_slots(&self) -> &[MonoEffectInstance] {
        self.signature.handler_record_slots()
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
        owner: &MonoDefInstance,
        instance: MonoFragmentInstance,
        functions: Vec<IRFunctionID>,
    ) -> (Self, MonoIR) {
        let (root, nested) = functions.split_first().expect("a fragment should have a root");
        let root_abi = CallbackStyleFunctionABI::of(resolver, source, owner, *root).await;

        // Declare the root, then every nested function in order.
        let engine = resolver.engine();
        let mut output = MonoIR::new(instance, root_abi.signature.clone(), engine);
        let mut function_abis = FxHashMap::default();
        let mut function_ids = FxHashMap::default();
        function_ids.insert(*root, output.root_id());
        function_abis.insert(*root, root_abi);
        for function in nested.iter().copied() {
            let abi = CallbackStyleFunctionABI::of(resolver, source, owner, function).await;
            let function_id = output.insert_function(abi.kind, abi.signature.clone(), engine);
            function_ids.insert(function, function_id);
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

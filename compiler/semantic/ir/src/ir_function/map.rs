use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::Arena;
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::deferred_marker::DeferredMarker;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    signature::get_effect_row,
    ty::{Ty, application::ClosureID},
};

use super::{IRContext, IRFunction, IRFunctionID};
use crate::{
    address::Address,
    cfg::{BlockID, ControlFlowEdge, InstructionInsertion, Terminator},
    ir_closure::{
        Capture, CaptureID, CaptureMap, CaptureMapID, ClosureParameter, ClosureParameterID,
    },
    ir_expr::{IRExpr, IRExprID},
    ir_operation_handler::{OperationHandlerParameter, OperationHandlerParameterID},
    ir_variable::IRVariableID,
    scope::{Scope, ScopeID},
    visit::{
        ExprVisitor, TypeSite, TypeVisitor, TypeVisitorMut, TypeVisitorMutAsync, VisitExpr,
        VisitType, VisitTypeMut, VisitTypeMutAsync,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct IRFunctionMap {
    /// The definition whose body this map lowers.
    def_id: GlobalSymbolID,
    functions: Arena<IRFunction>,
    /// Capture layouts referenced by nested-function contexts. Operation
    /// handlers belonging to one handler record share an entry.
    capture_maps: Arena<CaptureMap>,
    root: IRFunctionID,
    closures: FxHashMap<ClosureID, IRFunctionID>,
    /// Whether the definition reported no error up to and including the
    /// building of this IR.
    no_error: bool,
    deferred_markers: Vec<DeferredMarker>,
}

impl IRFunctionMap {
    #[must_use]
    pub fn new(def_id: GlobalSymbolID) -> Self {
        let mut functions = Arena::new();
        let root = functions.insert(IRFunction::new_def());
        Self {
            def_id,
            functions,
            capture_maps: Arena::new(),
            root,
            closures: FxHashMap::default(),
            no_error: false,
            deferred_markers: Vec::new(),
        }
    }

    pub fn extend_deferred_markers(&mut self, markers: impl IntoIterator<Item = DeferredMarker>) {
        self.deferred_markers.extend(markers);
    }

    /// Returns the marker predicates whose proof was deferred until the saved
    /// locals of the coroutines in this definition are known.
    #[must_use]
    pub fn deferred_markers(&self) -> &[DeferredMarker] { &self.deferred_markers }

    /// Returns whether the definition reported no error up to and including
    /// the building of this IR: none in its typed AST, in its lowering or in
    /// its memory analysis.
    ///
    /// A later phase reads this to skip the checks that are only meaningful
    /// on valid IR, without asking for the diagnostics.
    #[must_use]
    pub const fn no_error(&self) -> bool { self.no_error }

    /// Records whether the definition reported no error up to and including
    /// the building of this IR.
    pub const fn set_no_error(&mut self, no_error: bool) { self.no_error = no_error; }

    /// Returns the effect row of a function.
    ///
    /// The definition function takes the effect row declared in the
    /// definition's signature, while a nested function stores its own.
    pub async fn effect_of(
        &self,
        function_id: IRFunctionID,
        engine: &TrackedEngine,
    ) -> Interned<Ty> {
        match self.get_function(function_id).context().nested_effect() {
            Some(effect) => effect.clone(),
            None => engine.get_effect_row(self.def_id).await,
        }
    }

    #[must_use]
    pub fn root_scope_id(&self, function_id: IRFunctionID) -> ScopeID {
        self.get_function(function_id).root_scope_id()
    }

    #[must_use]
    pub fn get_scope(&self, function_id: IRFunctionID, scope_id: ScopeID) -> &Scope {
        self.get_function(function_id).get_scope(scope_id)
    }

    #[must_use]
    pub fn insert_scope(&mut self, function_id: IRFunctionID, parent: ScopeID) -> ScopeID {
        self.get_function_mut(function_id).insert_scope(parent)
    }

    #[must_use]
    pub fn insert_scope_branch(
        &mut self,
        function_id: IRFunctionID,
        parent: ScopeID,
        branch_count: usize,
    ) -> Vec<ScopeID> {
        self.get_function_mut(function_id).insert_scope_branch(parent, branch_count)
    }

    /// Associates a source closure identity with its lowered local function.
    pub fn register_closure(&mut self, closure_id: ClosureID, function_id: IRFunctionID) {
        let _ = self.get_function(function_id).context().assert_as_closure_context();
        assert!(self.closures.insert(closure_id, function_id).is_none());
    }

    #[must_use]
    pub fn get_function_id_of_closure(&self, closure_id: ClosureID) -> IRFunctionID {
        self.closures.get(&closure_id).copied().expect("should be a valid ID")
    }

    #[must_use]
    pub fn get_function_of_closure(&self, closure_id: ClosureID) -> &IRFunction {
        let function_id = self.get_function_id_of_closure(closure_id);
        self.get_function(function_id)
    }

    /// Returns whether `function_id` is the body of a source closure.
    #[must_use]
    pub fn is_closure_function(&self, function_id: IRFunctionID) -> bool {
        self.closures.values().any(|function| *function == function_id)
    }

    /// Iterates over nominal closure identities and their lowered functions.
    #[must_use]
    pub fn closures(&self) -> impl ExactSizeIterator<Item = (ClosureID, IRFunctionID)> {
        self.closures.iter().map(|(closure_id, function_id)| (*closure_id, *function_id))
    }

    pub fn fill_return_on_unterminated_blocks(&mut self, function_id: IRFunctionID) {
        self.get_function_mut(function_id).cfg.fill_return_on_unterminated_blocks();
    }

    /// Returns the definition whose body this map lowers.
    #[must_use]
    pub const fn def_id(&self) -> GlobalSymbolID { self.def_id }

    #[must_use]
    pub const fn root_id(&self) -> IRFunctionID { self.root }

    #[must_use]
    pub fn root(&self) -> &IRFunction {
        self.functions.get(self.root).expect("Root IR function should exist")
    }

    #[must_use]
    pub fn get_function(&self, id: IRFunctionID) -> &IRFunction {
        self.functions.get(id).expect("IR function should exist")
    }

    fn get_function_mut(&mut self, id: IRFunctionID) -> &mut IRFunction {
        self.functions.get_mut(id).expect("IR function should exist")
    }

    /// Iterates over all IR functions belonging to a source def and their IDs.
    ///
    /// The iteration order is not stable.
    #[must_use]
    pub fn functions(&self) -> impl ExactSizeIterator<Item = (IRFunctionID, &IRFunction)> {
        self.functions.iter()
    }

    /// Returns every IR function of the source def, each after the nested
    /// functions it creates: a closure body comes before the function whose
    /// closure expression creates it, and a handled body and its operation
    /// handlers come before the function of their `run ... with`.
    ///
    /// Otherwise, the order is not stable.
    #[must_use]
    pub fn functions_innermost_first(&self) -> Vec<IRFunctionID> {
        let mut order = Vec::with_capacity(self.functions.len());
        let mut visited = FxHashSet::default();

        // Walk the functions each function creates before the function
        // itself. Starting from every function covers the ones that no
        // expression creates, which is only the definition function in valid
        // IR.
        for (start, _) in self.functions() {
            let mut pending = vec![(start, false)];

            while let Some((function_id, is_expanded)) = pending.pop() {
                if is_expanded {
                    order.push(function_id);
                    continue;
                }
                if !visited.insert(function_id) {
                    continue;
                }

                pending.push((function_id, true));
                pending.extend(
                    self.get_function(function_id)
                        .created_functions()
                        .map(|created| (created, false)),
                );
            }
        }

        order
    }

    #[must_use]
    pub fn insert_closure(
        &mut self,
        return_ty: Interned<Ty>,
        effect: Interned<Ty>,
        capture_map: CaptureMapID,
    ) -> IRFunctionID {
        self.functions.insert(IRFunction::new_closure(return_ty, effect, capture_map))
    }

    #[must_use]
    pub fn insert_thunk(
        &mut self,
        return_ty: Interned<Ty>,
        effect: Interned<Ty>,
        capture_map: CaptureMapID,
    ) -> IRFunctionID {
        self.functions.insert(IRFunction::new_thunk(return_ty, effect, capture_map))
    }

    #[must_use]
    pub fn insert_operation_handler(
        &mut self,
        operation: rayc_symbol::GlobalSymbolID,
        return_ty: Interned<Ty>,
        effect: Interned<Ty>,
        capture_map: CaptureMapID,
        engine: &TrackedEngine,
    ) -> IRFunctionID {
        self.functions.insert(IRFunction::new_operation_handler(
            operation,
            return_ty,
            effect,
            capture_map,
            engine,
        ))
    }

    #[must_use]
    pub fn insert_closure_parameter(
        &mut self,
        function_id: IRFunctionID,
        parameter: ClosureParameter,
    ) -> ClosureParameterID {
        self.get_function_mut(function_id).insert_closure_parameter(parameter)
    }

    #[must_use]
    pub fn insert_operation_handler_parameter(
        &mut self,
        function_id: IRFunctionID,
        parameter: OperationHandlerParameter,
    ) -> OperationHandlerParameterID {
        self.get_function_mut(function_id).insert_operation_handler_parameter(parameter)
    }

    #[must_use]
    pub fn new_capture_map(&mut self) -> CaptureMapID {
        self.capture_maps.insert(CaptureMap::default())
    }

    #[must_use]
    pub fn insert_capture(&mut self, capture_map_id: CaptureMapID, capture: Capture) -> CaptureID {
        self.capture_maps
            .get_mut(capture_map_id)
            .expect("IR capture map should exist")
            .insert_capture(capture)
    }

    #[must_use]
    pub fn get_capture_map(&self, capture_map_id: CaptureMapID) -> &CaptureMap {
        self.capture_maps.get(capture_map_id).expect("IR capture map should exist")
    }

    #[must_use]
    pub fn get_capture(&self, function_id: IRFunctionID, capture_id: CaptureID) -> &Capture {
        self.capture_map(function_id).get_capture(capture_id)
    }

    #[must_use]
    pub fn captures(
        &self,
        function_id: IRFunctionID,
    ) -> impl ExactSizeIterator<Item = (CaptureID, &Capture)> {
        self.capture_map(function_id).iter()
    }

    /// Returns the capture layout owned by a nested function context.
    ///
    /// Definition functions do not capture values and therefore return `None`.
    #[must_use]
    pub fn captures_for_function(&self, function_id: IRFunctionID) -> Option<&CaptureMap> {
        let function = self.get_function(function_id);
        match function.context() {
            IRContext::Def => None,
            IRContext::Closure(_) | IRContext::Thunk(_) | IRContext::OperationHandler(_) => {
                Some(self.capture_map(function_id))
            }
        }
    }

    /// Returns the arena-owned capture layout referenced by a nested function.
    #[must_use]
    pub fn capture_map_id(&self, function_id: IRFunctionID) -> CaptureMapID {
        self.get_function(function_id).context().assert_capture_map_id()
    }

    /// Returns the capture layout of a nested function, or `None` for the
    /// definition function, which does not capture values.
    #[must_use]
    pub fn nested_capture_map_id(&self, function_id: IRFunctionID) -> Option<CaptureMapID> {
        self.get_function(function_id).context().capture_map_id()
    }

    /// Visits every type stored in this map, together with where it is
    /// stored.
    ///
    /// The order is the same as that of [`Self::visit_types_mut`].
    pub fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        // Capture layouts first; they may be shared by several functions.
        for (capture_map_id, capture_map) in self.capture_maps.iter() {
            capture_map.visit_types(TypeSite::Capture(capture_map_id), visitor);
        }

        // Then the signature and the body of every function.
        for (function_id, function) in self.functions() {
            function.visit_types(function_id, visitor);
        }
    }

    /// Visits every type stored in this map mutably, together with where it
    /// is stored.
    ///
    /// Every capture layout is visited before any function, so a visitor
    /// sees the captures of a nested function before its signature. The
    /// types of one function's signature are visited together, before the
    /// types of its body. Otherwise, the iteration order is not stable.
    pub fn visit_types_mut<V: TypeVisitorMut>(&mut self, visitor: &mut V) {
        // Capture layouts first; they may be shared by several functions.
        for (capture_map_id, capture_map) in self.capture_maps.iter_mut() {
            capture_map.visit_types_mut(TypeSite::Capture(capture_map_id), visitor);
        }

        // Then the signature and the body of every function.
        for (function_id, function) in self.functions.iter_mut() {
            function.visit_types_mut(function_id, visitor);
        }
    }

    /// Like [`Self::visit_types_mut`], but with a visitor that may await,
    /// for example to query the engine. The order is the same.
    pub async fn visit_types_mut_async<V: TypeVisitorMutAsync>(&mut self, visitor: &mut V) {
        // Capture layouts first; they may be shared by several functions.
        for (capture_map_id, capture_map) in self.capture_maps.iter_mut() {
            capture_map.visit_types_mut_async(TypeSite::Capture(capture_map_id), visitor).await;
        }

        // Then the signature and the body of every function.
        for (function_id, function) in self.functions.iter_mut() {
            function.visit_types_mut_async(function_id, visitor).await;
        }
    }

    fn capture_map(&self, function_id: IRFunctionID) -> &CaptureMap {
        let capture_map = self.capture_map_id(function_id);
        self.capture_maps.get(capture_map).expect("IR capture map should exist")
    }

    #[must_use]
    pub fn entry_block(&self, function_id: IRFunctionID) -> BlockID {
        self.get_function(function_id).entry_block()
    }

    #[must_use]
    pub fn create_block(&mut self, function_id: IRFunctionID) -> BlockID {
        self.get_function_mut(function_id).create_block()
    }

    #[must_use]
    pub fn insert_expression(&mut self, function_id: IRFunctionID, expression: IRExpr) -> IRExprID {
        self.get_function_mut(function_id).insert_expression(expression)
    }

    #[must_use]
    pub fn create_variable_in_scope(
        &mut self,
        function_id: IRFunctionID,
        scope_id: ScopeID,
        ty: Interned<Ty>,
        span: RelativeSpan,
    ) -> IRVariableID {
        self.get_function_mut(function_id).create_variable_in_scope(scope_id, ty, span, false)
    }

    /// Creates a temporary in `scope_id`: a variable which gives a place to a
    /// computed value, rather than a binding declared in the source.
    #[must_use]
    pub fn create_temporary_in_scope(
        &mut self,
        function_id: IRFunctionID,
        scope_id: ScopeID,
        ty: Interned<Ty>,
        span: RelativeSpan,
    ) -> IRVariableID {
        self.get_function_mut(function_id).create_variable_in_scope(scope_id, ty, span, true)
    }

    pub fn push_expression(
        &mut self,
        function_id: IRFunctionID,
        block_id: BlockID,
        expression: IRExprID,
    ) {
        self.get_function_mut(function_id).push_expression(block_id, expression);
    }

    pub fn push_expr_discard(
        &mut self,
        function_id: IRFunctionID,
        block_id: BlockID,
        expression: IRExprID,
        drop_instance: Interned<Ty>,
    ) {
        self.get_function_mut(function_id).push_expr_discard(block_id, expression, drop_instance);
    }

    pub fn push_scope_push_instruction(
        &mut self,
        function_id: IRFunctionID,
        block_id: BlockID,
        scope_id: ScopeID,
    ) {
        self.get_function_mut(function_id).push_scope_push_instruction(block_id, scope_id);
    }

    pub fn push_scope_pop_instruction(
        &mut self,
        function_id: IRFunctionID,
        block_id: BlockID,
        scope_id: ScopeID,
    ) {
        self.get_function_mut(function_id).push_scope_pop_instruction(block_id, scope_id);
    }

    pub fn push_store(
        &mut self,
        function_id: IRFunctionID,
        block_id: BlockID,
        address: Address,
        value: IRExprID,
        span: RelativeSpan,
    ) {
        self.get_function_mut(function_id).push_store(block_id, address, value, span);
    }

    /// Applies every instruction queued in `insertion` to `function_id`.
    pub fn insert_instructions(
        &mut self,
        function_id: IRFunctionID,
        insertion: InstructionInsertion,
    ) {
        self.get_function_mut(function_id).insert_instructions(insertion);
    }

    /// Redirects `edge` of `function_id` through a new empty block and
    /// returns that block, rewiring the phis of its target to take their
    /// value from the new block.
    pub fn split_edge(&mut self, function_id: IRFunctionID, edge: ControlFlowEdge) -> BlockID {
        self.get_function_mut(function_id).split_edge(edge)
    }

    pub fn set_terminator(
        &mut self,
        function_id: IRFunctionID,
        block_id: BlockID,
        terminator: Terminator,
    ) {
        self.get_function_mut(function_id).set_terminator(block_id, terminator);
    }

    #[must_use]
    pub fn block_terminator(
        &self,
        function_id: IRFunctionID,
        block_id: BlockID,
    ) -> Option<&Terminator> {
        self.get_function(function_id).block_terminator(block_id)
    }
}

impl VisitExpr for IRFunctionMap {
    fn visit_exprs<V: ExprVisitor>(&self, visitor: &mut V) {
        for (function_id, function) in self.functions() {
            for (expression_id, expression) in function.expression_map.expressions() {
                visitor.visit_expr(function_id, expression_id, expression);
            }
        }
    }
}

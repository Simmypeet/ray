use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::ID;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::Ty;

use crate::{
    address::{Address, Local},
    cfg::{
        BlockID, Cfg, ControlFlowEdge, Instruction, InstructionInsertion, Point, Reachables,
        Terminator, Traverser,
    },
    dataflow::{DataflowProblem, DataflowSolution, solve},
    ir_closure::{
        CaptureMapID, ClosureParameter, ClosureParameterID, IRClosureContext, IRThunkContext,
    },
    ir_expr::{IRExpr, IRExprID, IRExpressionMap},
    ir_operation_handler::{
        IROperationHandlerContext, OperationHandlerParameter, OperationHandlerParameterID,
    },
    ir_variable::{IRVariable, IRVariableID, IRVariableMap},
    scope::{Scope, ScopeID, ScopeMap},
    visit::{
        TypeSite, TypeVisitor, TypeVisitorMut, TypeVisitorMutAsync, VisitType, VisitTypeMut,
        VisitTypeMutAsync,
    },
};

mod context;
mod map;

pub use context::IRContext;
pub use map::IRFunctionMap;

pub type IRFunctionID = ID<IRFunction>;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct IRFunction {
    cfg: Cfg,
    scope_map: ScopeMap,
    variable_map: IRVariableMap,
    expression_map: IRExpressionMap,
    context: IRContext,
}

impl IRFunction {
    #[must_use]
    pub fn new_def() -> Self {
        Self {
            cfg: Cfg::default(),
            scope_map: ScopeMap::new(),
            variable_map: IRVariableMap::default(),
            expression_map: IRExpressionMap::default(),
            context: IRContext::Def,
        }
    }

    #[must_use]
    pub fn new_closure(
        return_ty: Interned<Ty>,
        effect: Interned<Ty>,
        capture_map: CaptureMapID,
    ) -> Self {
        Self {
            cfg: Cfg::default(),
            scope_map: ScopeMap::new(),
            variable_map: IRVariableMap::default(),
            expression_map: IRExpressionMap::default(),
            context: IRContext::Closure(IRClosureContext::new(return_ty, effect, capture_map)),
        }
    }

    #[must_use]
    pub fn new_thunk(
        return_ty: Interned<Ty>,
        effect: Interned<Ty>,
        capture_map: CaptureMapID,
    ) -> Self {
        Self {
            cfg: Cfg::default(),
            scope_map: ScopeMap::new(),
            variable_map: IRVariableMap::default(),
            expression_map: IRExpressionMap::default(),
            context: IRContext::Thunk(IRThunkContext::new(return_ty, effect, capture_map)),
        }
    }

    #[must_use]
    pub fn new_operation_handler(
        operation: rayc_symbol::GlobalSymbolID,
        return_ty: Interned<Ty>,
        effect: Interned<Ty>,
        capture_map: CaptureMapID,
        engine: &TrackedEngine,
    ) -> Self {
        Self {
            cfg: Cfg::default(),
            scope_map: ScopeMap::new(),
            variable_map: IRVariableMap::default(),
            expression_map: IRExpressionMap::default(),
            context: IRContext::OperationHandler(IROperationHandlerContext::new(
                operation,
                return_ty,
                effect,
                capture_map,
                engine,
            )),
        }
    }

    #[must_use]
    pub const fn context(&self) -> &IRContext { &self.context }

    /// Returns the lifetime that a run of this function borrows the
    /// environment holding its captures for, when it does not own them; see
    /// [`IRContext::environment_lifetime`].
    #[must_use]
    pub const fn environment_lifetime(&self) -> Option<&Interned<Ty>> {
        self.context.environment_lifetime()
    }

    /// Iterates over the nested functions the expressions of this function
    /// create, in unspecified order; see [`IRExprKind::created_functions`].
    ///
    /// [`IRExprKind::created_functions`]: crate::ir_expr::IRExprKind::created_functions
    pub fn created_functions(&self) -> impl Iterator<Item = IRFunctionID> + '_ {
        self.expression_map
            .expressions()
            .flat_map(|(_, expression)| expression.kind().created_functions())
    }

    /// Returns the parameters of this function, which belongs to the
    /// definition `def_id`, in declaration order; see
    /// [`IRContext::parameter_locals`].
    pub async fn parameter_locals(
        &self,
        def_id: GlobalSymbolID,
        engine: &TrackedEngine,
    ) -> Vec<Local> {
        self.context.parameter_locals(def_id, engine).await
    }

    /// Returns the return type of this function, which belongs to the
    /// definition `def_id`; see [`IRContext::return_ty`].
    pub async fn return_ty(&self, def_id: GlobalSymbolID, engine: &TrackedEngine) -> Interned<Ty> {
        self.context.return_ty(def_id, engine).await
    }

    #[must_use]
    pub fn insert_closure_parameter(&mut self, parameter: ClosureParameter) -> ClosureParameterID {
        self.context.assert_as_closure_context_mut().insert_parameter(parameter)
    }

    #[must_use]
    pub fn insert_operation_handler_parameter(
        &mut self,
        parameter: OperationHandlerParameter,
    ) -> OperationHandlerParameterID {
        match &mut self.context {
            IRContext::OperationHandler(context) => context.insert_parameter(parameter),
            IRContext::Def | IRContext::Closure(_) | IRContext::Thunk(_) => {
                panic!("operation handler parameters require an operation handler context")
            }
        }
    }

    #[must_use]
    pub fn get_expression(&self, id: IRExprID) -> &IRExpr { self.expression_map.get_expression(id) }

    #[must_use]
    pub fn get_variable(&self, id: IRVariableID) -> &IRVariable {
        self.variable_map.get_variable(id)
    }

    /// Iterates over function-local storage and its IDs.
    ///
    /// The iteration order is not stable.
    #[must_use]
    pub fn variables(&self) -> impl ExactSizeIterator<Item = (IRVariableID, &IRVariable)> {
        self.variable_map.variables()
    }

    #[must_use]
    pub const fn root_scope_id(&self) -> ScopeID { self.scope_map.root_id() }

    #[must_use]
    pub fn declared_variables(
        &self,
        scope_id: ScopeID,
    ) -> impl ExactSizeIterator<Item = IRVariableID> + '_ {
        self.get_scope(scope_id).declared_variables()
    }

    #[must_use]
    fn get_scope(&self, id: ScopeID) -> &Scope { self.scope_map.get_scope(id) }

    #[must_use]
    fn insert_scope(&mut self, parent: ScopeID) -> ScopeID { self.scope_map.insert_scope(parent) }

    #[must_use]
    fn insert_scope_branch(&mut self, parent: ScopeID, branch_count: usize) -> Vec<ScopeID> {
        self.scope_map.insert_branch(parent, branch_count)
    }

    #[must_use]
    pub const fn entry_block(&self) -> BlockID { self.cfg.entry_block() }

    #[must_use]
    pub fn create_block(&mut self) -> BlockID { self.cfg.create_block() }

    #[must_use]
    pub fn insert_expression(&mut self, expression: IRExpr) -> IRExprID {
        self.expression_map.insert_expression(expression)
    }

    #[must_use]
    fn create_variable_in_scope(
        &mut self,
        scope_id: ScopeID,
        ty: Interned<Ty>,
        span: RelativeSpan,
        is_temporary: bool,
    ) -> IRVariableID {
        let variable_id = self.variable_map.insert_variable(ty, span, scope_id, is_temporary);
        self.scope_map.register_variable(scope_id, variable_id);
        variable_id
    }

    pub fn push_expression(&mut self, block_id: BlockID, expression: IRExprID) {
        self.cfg.push_expression(block_id, expression);
    }

    pub fn push_expr_discard(
        &mut self,
        block_id: BlockID,
        expression: IRExprID,
        drop_instance: Interned<Ty>,
    ) {
        self.cfg.push_expr_discard(block_id, expression, drop_instance);
    }

    pub fn push_scope_push_instruction(&mut self, block_id: BlockID, scope_id: ScopeID) {
        self.cfg.push_scope_push_instruction(block_id, scope_id);
    }

    pub fn push_scope_pop_instruction(&mut self, block_id: BlockID, scope_id: ScopeID) {
        self.cfg.push_scope_pop_instruction(block_id, scope_id);
    }

    pub fn push_store(
        &mut self,
        block_id: BlockID,
        address: Address,
        value: IRExprID,
        span: RelativeSpan,
    ) {
        self.cfg.push_store(block_id, address, value, span);
    }

    /// Applies every instruction queued in `insertion`.
    pub fn insert_instructions(&mut self, insertion: InstructionInsertion) {
        self.cfg.insert_instructions(insertion);
    }

    /// Redirects `edge` through a new empty block and returns that block.
    ///
    /// Every phi in the edge's target then takes the value that flowed in
    /// along the edge from the new block.
    pub fn split_edge(&mut self, edge: ControlFlowEdge) -> BlockID {
        let split_block = self.cfg.split_edge(edge);

        // The source may still reach the target through another edge, as when
        // both arms of a conditional target it. The phis then keep the value
        // on that edge as well.
        let source_still_incoming = self
            .cfg
            .terminator(edge.source())
            .is_some_and(|terminator| terminator.jump_targets().any(|t| t == edge.target()));

        // Move the value the phis took from the source onto the new block.
        for instruction in self.cfg.instructions(edge.target()) {
            let Instruction::Expression(expression_id) = instruction else {
                continue;
            };
            if let Some(phi) = self.expression_map.phi_mut(*expression_id) {
                phi.split_incoming(edge.source(), split_block, source_still_incoming);
            }
        }

        split_block
    }

    pub fn set_terminator(&mut self, block_id: BlockID, terminator: Terminator) {
        self.cfg.set_terminator(block_id, terminator);
    }

    #[must_use]
    pub fn block_instructions(&self, block_id: BlockID) -> &[Instruction] {
        self.cfg.instructions(block_id)
    }

    /// Iterates over the instructions of a block, in order, each with the
    /// point it sits at.
    #[must_use]
    pub fn block_instructions_with_points(
        &self,
        block_id: BlockID,
    ) -> impl ExactSizeIterator<Item = (Point, &Instruction)> {
        self.cfg.instructions_with_points(block_id)
    }

    /// Iterates over the points control reaches right after `point`; see
    /// [`Cfg::successor_points`].
    pub fn successor_points(&self, point: Point) -> impl Iterator<Item = Point> + '_ {
        self.cfg.successor_points(point)
    }

    /// Iterates over the points control comes from right before `point`; see
    /// [`Cfg::predecessor_points`].
    pub fn predecessor_points(&self, point: Point) -> impl Iterator<Item = Point> + '_ {
        self.cfg.predecessor_points(point)
    }

    #[must_use]
    pub fn block_terminator(&self, block_id: BlockID) -> Option<&Terminator> {
        self.cfg.terminator(block_id)
    }

    /// Returns the point that stands for the terminator of `block_id`: one
    /// past its last instruction.
    #[must_use]
    pub fn terminator_point(&self, block_id: BlockID) -> Point {
        Point::builder()
            .block_id(block_id)
            .instruction_idx(self.cfg.instructions(block_id).len())
            .build()
    }

    /// Returns the values defined when control flows along `edge`: those the
    /// terminator of its source defines on that edge only, such as the value
    /// a suspension point resumes with.
    pub fn edge_definitions(&self, edge: ControlFlowEdge) -> impl Iterator<Item = IRExprID> + '_ {
        self.block_terminator(edge.source())
            .into_iter()
            .flat_map(move |terminator| terminator.expressions_defined_on(edge.kind()))
    }

    /// Returns the phi operands consumed when control flows along `edge`:
    /// the values the phis of its target take from its source.
    pub fn edge_phi_operands(&self, edge: ControlFlowEdge) -> impl Iterator<Item = IRExprID> + '_ {
        let source = edge.source();
        self.block_instructions(edge.target()).iter().filter_map(move |instruction| {
            let Instruction::Expression(expression_id) = instruction else {
                return None;
            };
            self.get_expression(*expression_id).kind().as_phi()?.value_from(source)
        })
    }

    /// Iterates over the expressions `block_id` evaluates, in order, each
    /// with the point it is evaluated at: its expression instructions, then
    /// the expression of a suspension point that ends it.
    pub fn block_evaluations(
        &self,
        block_id: BlockID,
    ) -> impl Iterator<Item = (Point, IRExprID)> + '_ {
        let instructions =
            self.block_instructions_with_points(block_id).filter_map(|(point, instruction)| {
                match instruction {
                    Instruction::Expression(expression_id) => Some((point, *expression_id)),
                    Instruction::ScopePush(_)
                    | Instruction::ScopePop(_)
                    | Instruction::ExprDiscard(_)
                    | Instruction::AddressDrop(_)
                    | Instruction::Store(_) => None,
                }
            });
        let point = self.terminator_point(block_id);
        let terminator = self
            .block_terminator(block_id)
            .into_iter()
            .flat_map(Terminator::evaluated_expressions)
            .map(move |expression_id| (point, expression_id));

        instructions.chain(terminator)
    }

    /// Returns the source of the instruction or terminator at `point`, or
    /// `None` for one that has no source of its own: a scope instruction, a
    /// jump, or a bare return.
    #[must_use]
    pub fn point_span(&self, point: Point) -> Option<RelativeSpan> {
        let Some(instruction) =
            self.block_instructions(point.block_id()).get(point.instruction_idx())
        else {
            // A terminator has the source of the value it consumes, or of
            // the expression it evaluates.
            let terminator = self.block_terminator(point.block_id())?;
            return terminator
                .used_value()
                .or_else(|| terminator.evaluated_expressions().next())
                .map(|value| self.get_expression(value).span());
        };

        match instruction {
            Instruction::Expression(expression_id) => {
                Some(self.get_expression(*expression_id).span())
            }
            Instruction::Store(store) => Some(store.span()),
            Instruction::ExprDiscard(discard) => {
                Some(self.get_expression(discard.expression()).span())
            }
            Instruction::AddressDrop(drop) => Some(drop.span()),
            Instruction::ScopePush(_) | Instruction::ScopePop(_) => None,
        }
    }

    /// Iterates over blocks that do not have a terminator.
    pub fn unterminated_blocks(&self) -> impl Iterator<Item = BlockID> + '_ {
        self.cfg.unterminated_blocks()
    }

    /// Iterates over the blocks reachable from the entry block; see
    /// [`Cfg::traverse`].
    #[must_use]
    pub fn traverse(&self) -> Traverser<'_> { self.cfg.traverse() }

    #[must_use]
    pub fn reachables(&self) -> Reachables { self.cfg.reachables() }

    /// Returns what is reachable in direct form, where no abandon edge is
    /// taken; see [`Cfg::direct_reachables`].
    #[must_use]
    pub fn direct_reachables(&self) -> Reachables { self.cfg.direct_reachables() }

    /// Solves a dataflow problem over this function's control-flow graph.
    pub async fn solve_dataflow<P: DataflowProblem>(
        &self,
        problem: &mut P,
    ) -> Result<DataflowSolution<P::JoinLattice>, P::Error> {
        solve(problem, &self.cfg).await
    }
}

impl IRFunction {
    /// Visits every type of the function `function_id`: its signature, then
    /// its body.
    ///
    /// The definition function stores no signature types.
    fn visit_types<V: TypeVisitor>(&self, function_id: IRFunctionID, visitor: &mut V) {
        self.context.visit_types(TypeSite::Signature(function_id), visitor);

        let body = TypeSite::Body(function_id);
        self.variable_map.visit_types(body, visitor);
        self.expression_map.visit_types(body, visitor);
        self.cfg.visit_types(body, visitor);
    }

    /// Visits every type of the function `function_id` mutably: its
    /// signature, then its body.
    ///
    /// The definition function stores no signature types.
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, function_id: IRFunctionID, visitor: &mut V) {
        self.context.visit_types_mut(TypeSite::Signature(function_id), visitor);

        let body = TypeSite::Body(function_id);
        self.variable_map.visit_types_mut(body, visitor);
        self.expression_map.visit_types_mut(body, visitor);
        self.cfg.visit_types_mut(body, visitor);
    }

    /// Like [`Self::visit_types_mut`], but with a visitor that may await.
    async fn visit_types_mut_async<V: TypeVisitorMutAsync>(
        &mut self,
        function_id: IRFunctionID,
        visitor: &mut V,
    ) {
        self.context.visit_types_mut_async(TypeSite::Signature(function_id), visitor).await;

        let body = TypeSite::Body(function_id);
        self.variable_map.visit_types_mut_async(body, visitor).await;
        self.expression_map.visit_types_mut_async(body, visitor).await;
        self.cfg.visit_types_mut_async(body, visitor).await;
    }
}

#[cfg(test)]
mod tests;

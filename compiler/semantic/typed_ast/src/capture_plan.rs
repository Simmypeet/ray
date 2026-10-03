use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    capture::{CaptureMode, LoadKind},
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::{Mutability, Ty, lifetime::Lifetime},
};

use crate::{
    name_binding::{NameBindingID, Source},
    statement::Statement,
    typed_expr::{
        TypedExprID, TypedExprKind,
        binary::{Binary, BinaryOp},
        call::{Call, CallTarget},
        if_else::IfElse,
        run_with::RunWith,
        struct_initialization::StructInitialization,
        while_loop::While,
    },
    typed_function::{TypedFunctionID, TypedFunctionMap},
};

/// Decides whether values of a type are `Copy`, which capture inference needs
/// to tell a read that borrows a binding from one that moves it.
pub trait CopyOracle {
    /// Returns whether a value of `ty` is `Copy`.
    fn is_copy(&mut self, ty: &Interned<Ty>) -> impl Future<Output = bool> + Send;
}

/// Stable capture layouts for every function in a typed AST.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct CapturePlan {
    plans: Arena<FunctionCapturePlan>,
    function_plans: FxHashMap<TypedFunctionID, FunctionCapturePlanID>,
}

impl CapturePlan {
    /// Computes capture layouts from a complete typed AST.
    ///
    /// `copy_oracle` decides which values are `Copy`. Reading such a value
    /// only needs a shared reference to the binding, while reading any other
    /// value moves the binding into the closure.
    pub async fn analyze(functions: &TypedFunctionMap, copy_oracle: &mut impl CopyOracle) -> Self {
        let mut analyzer = Analyzer {
            plans: Arena::default(),
            function_plans: FxHashMap::default(),
            parents: FxHashMap::default(),
            visiting: FxHashSet::default(),
            copy_oracle,
        };
        let root_id = functions.root_id();
        let mut root_plan = FunctionCapturePlan::new();
        analyzer.analyze_function(root_id, functions, &mut root_plan).await;
        analyzer.insert_plan(root_id, root_plan);
        Self { plans: analyzer.plans, function_plans: analyzer.function_plans }
    }

    #[must_use]
    pub fn plan(&self, function_id: TypedFunctionID) -> &FunctionCapturePlan {
        self.plans.get(self.plan_id(function_id)).expect("function capture plan should exist")
    }

    #[must_use]
    pub fn shares_plan(&self, first: TypedFunctionID, second: TypedFunctionID) -> bool {
        self.plan_id(first) == self.plan_id(second)
    }

    fn plan_id(&self, function_id: TypedFunctionID) -> FunctionCapturePlanID {
        *self
            .function_plans
            .get(&function_id)
            .expect("capture analysis should cover every reached function")
    }
}

impl MutSubstitutable for CapturePlan {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        for plan in self.plans.items_mut() {
            for requirement in &mut plan.captures {
                requirement.binding_ty.apply_in_place(subst, engine);
            }
        }
    }
}

/// Index into [`FunctionCapturePlan::captures`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, StableHash, Encode, Decode)]
pub struct CaptureSlot(usize);

/// The capture layout of one `TypedAST` function.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct FunctionCapturePlan {
    /// Stable closure-field order, determined by first encounter.
    captures: Vec<CaptureRequirement>,
    /// Deduplication and lookup only; never iterate this map to create a
    /// layout.
    capture_slots: FxHashMap<Source, CaptureSlot>,
}

/// Identifies one arena-owned function capture plan.
pub type FunctionCapturePlanID = ID<FunctionCapturePlan>;

/// One original binding that this function must receive.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct CaptureRequirement {
    source: Source,
    binding_ty: Interned<Ty>,
    mode: CaptureMode,
    span: RelativeSpan,
}

impl FunctionCapturePlan {
    #[must_use]
    pub fn new() -> Self { Self { captures: Vec::new(), capture_slots: FxHashMap::default() } }

    #[must_use]
    pub fn captures(&self) -> impl ExactSizeIterator<Item = (CaptureSlot, &CaptureRequirement)> {
        self.captures.iter().enumerate().map(|(index, capture)| (CaptureSlot(index), capture))
    }

    #[must_use]
    pub fn capture_slot(&self, source: Source) -> Option<CaptureSlot> {
        self.capture_slots.get(&source).copied()
    }

    pub fn require(&mut self, requirement: CaptureRequirement) -> CaptureSlot {
        if let Some(slot) = self.capture_slot(requirement.source) {
            let existing = &mut self.captures[slot.0];
            existing.mode = existing.mode.join(requirement.mode);
            return slot;
        }

        let source = requirement.source;
        let slot = CaptureSlot(self.captures.len());
        self.captures.push(requirement);
        self.capture_slots.insert(source, slot);
        slot
    }
}

impl CaptureRequirement {
    #[must_use]
    pub const fn source(&self) -> Source { self.source }

    #[must_use]
    pub const fn binding_ty(&self) -> &Interned<Ty> { &self.binding_ty }

    #[must_use]
    pub const fn mode(&self) -> CaptureMode { self.mode }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }

    /// The type of this capture's environment field: the binding itself for
    /// a value capture, or a reference to it for a reference capture.
    #[must_use]
    pub fn storage_ty(&self, engine: &TrackedEngine) -> Interned<Ty> {
        match self.mode {
            CaptureMode::Value(_) => self.binding_ty.clone(),
            CaptureMode::Reference(mutability) => Ty::new_reference(
                Ty::new_lifetime(Lifetime::Erased, engine),
                self.binding_ty.clone(),
                mutability,
                engine,
            ),
        }
    }
}

impl Default for FunctionCapturePlan {
    fn default() -> Self { Self::new() }
}

impl FunctionCapturePlan {
    /// The concrete environment layout, including references for borrowed
    /// captures.
    #[must_use]
    pub fn captured_tuple(&self, engine: &TrackedEngine) -> Interned<Ty> {
        Ty::new_tuple(
            engine.intern_unsized(
                self.captures.iter().map(|capture| capture.storage_ty(engine)).collect::<Vec<_>>(),
            ),
            engine,
        )
    }
}

impl CaptureRequirement {
    #[must_use]
    pub const fn new(
        source: Source,
        binding_ty: Interned<Ty>,
        mode: CaptureMode,
        span: RelativeSpan,
    ) -> Self {
        Self { source, binding_ty, mode, span }
    }
}

/// How the enclosing expression uses an lvalue-shaped child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UseMode {
    Value(LoadKind),
    Address(Mutability),
}

impl UseMode {
    /// Creates an ordinary value use, which copies a `Copy` value and moves
    /// any other value.
    const fn new_value_implicit() -> Self { Self::Value(LoadKind::Implicit) }
}

struct Analyzer<'a, O> {
    plans: Arena<FunctionCapturePlan>,
    function_plans: FxHashMap<TypedFunctionID, FunctionCapturePlanID>,
    parents: FxHashMap<TypedFunctionID, TypedFunctionID>,
    visiting: FxHashSet<TypedFunctionID>,
    copy_oracle: &'a mut O,
}

impl<O: CopyOracle> Analyzer<'_, O> {
    async fn analyze_function(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        plan: &mut FunctionCapturePlan,
    ) {
        assert!(
            self.visiting.insert(function_id),
            "TypedAST nested-function graph should not contain a cycle"
        );
        assert!(
            !self.function_plans.contains_key(&function_id),
            "each TypedAST function should be analyzed exactly once"
        );

        for statement in functions.statements(function_id) {
            self.visit_statement(function_id, functions, statement, plan).await;
        }

        assert!(self.visiting.remove(&function_id));
    }

    fn insert_plan(
        &mut self,
        function_id: TypedFunctionID,
        plan: FunctionCapturePlan,
    ) -> FunctionCapturePlanID {
        let plan_id = self.plans.insert(plan);
        assert!(self.function_plans.insert(function_id, plan_id).is_none());
        plan_id
    }

    async fn visit_statement(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        statement: &Statement,
        plan: &mut FunctionCapturePlan,
    ) {
        match statement {
            Statement::Let(statement) => {
                if let Some(expr_id) = statement.expression() {
                    self.visit_expression(
                        function_id,
                        functions,
                        expr_id,
                        UseMode::new_value_implicit(),
                        plan,
                    )
                    .await;
                }
            }
            Statement::Break(_) | Statement::Continue(_) => {}
            Statement::Expression(statement) => {
                self.visit_expression(
                    function_id,
                    functions,
                    statement.expression(),
                    UseMode::new_value_implicit(),
                    plan,
                )
                .await;
            }
            Statement::Return(statement) => {
                if let Some(value) = statement.value() {
                    self.visit_expression(
                        function_id,
                        functions,
                        value,
                        UseMode::new_value_implicit(),
                        plan,
                    )
                    .await;
                }
            }
        }
    }

    /// Every recursive traversal passes through here, so this is where the
    /// recursion is boxed.
    async fn visit_expression(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        expression_id: TypedExprID,
        use_mode: UseMode,
        plan: &mut FunctionCapturePlan,
    ) {
        Box::pin(self.visit_expression_kind(function_id, functions, expression_id, use_mode, plan))
            .await;
    }

    #[expect(clippy::too_many_lines)]
    async fn visit_expression_kind(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        expression_id: TypedExprID,
        use_mode: UseMode,
        plan: &mut FunctionCapturePlan,
    ) {
        let expression = functions.get_expression(function_id, expression_id);
        let use_mode = self.refine_use_mode(use_mode, expression.ty()).await;

        match expression.kind() {
            TypedExprKind::Identifier(identifier) => {
                self.visit_identifier(
                    function_id,
                    functions,
                    identifier.name_binding(),
                    use_mode,
                    plan,
                );
            }
            TypedExprKind::Literal(_) => {}
            TypedExprKind::TupleIndex(tuple_index) => {
                self.visit_projection(
                    function_id,
                    functions,
                    tuple_index.operand(),
                    use_mode,
                    plan,
                )
                .await;
            }
            TypedExprKind::FieldAccess(field_access) => {
                self.visit_projection(
                    function_id,
                    functions,
                    field_access.operand(),
                    use_mode,
                    plan,
                )
                .await;
            }
            TypedExprKind::Tuple(tuple) => {
                for element in tuple.elements() {
                    self.visit_expression(
                        function_id,
                        functions,
                        *element,
                        UseMode::new_value_implicit(),
                        plan,
                    )
                    .await;
                }
            }
            TypedExprKind::Call(call) => {
                self.visit_call(function_id, functions, call, plan).await;
            }
            TypedExprKind::Closure(lambda) => {
                self.visit_nested_function(function_id, functions, lambda.function_id(), plan)
                    .await;
            }
            TypedExprKind::Binary(binary) => {
                self.visit_binary(function_id, functions, *binary, plan).await;
            }
            TypedExprKind::Unary(unary) => {
                self.visit_expression(
                    function_id,
                    functions,
                    unary.operand(),
                    UseMode::new_value_implicit(),
                    plan,
                )
                .await;
            }
            TypedExprKind::Cast(cast) => {
                self.visit_expression(
                    function_id,
                    functions,
                    cast.operand(),
                    UseMode::new_value_implicit(),
                    plan,
                )
                .await;
            }
            TypedExprKind::IfElse(if_else) => {
                self.visit_if_else(function_id, functions, if_else, plan).await;
            }
            TypedExprKind::While(while_loop) => {
                self.visit_while(function_id, functions, while_loop, plan).await;
            }
            TypedExprKind::StatementBlock(block) => {
                for statement in block.statements() {
                    self.visit_statement(function_id, functions, statement, plan).await;
                }
            }
            TypedExprKind::RefOf(reference) => {
                self.visit_expression(
                    function_id,
                    functions,
                    reference.pointee(),
                    UseMode::Address(reference.mutability()),
                    plan,
                )
                .await;
            }
            TypedExprKind::RefToPointer(coercion) => {
                self.visit_expression(
                    function_id,
                    functions,
                    coercion.reference(),
                    UseMode::new_value_implicit(),
                    plan,
                )
                .await;
            }
            TypedExprKind::Deref(deref) => {
                self.visit_expression(
                    function_id,
                    functions,
                    deref.pointee(),
                    UseMode::new_value_implicit(),
                    plan,
                )
                .await;
            }
            TypedExprKind::Paren(paren) => {
                self.visit_expression(function_id, functions, paren.expression(), use_mode, plan)
                    .await;
            }
            TypedExprKind::Move(move_expr) => {
                self.visit_expression(
                    function_id,
                    functions,
                    move_expr.operand(),
                    UseMode::Value(LoadKind::Move),
                    plan,
                )
                .await;
            }
            TypedExprKind::RunWith(run_with) => {
                self.visit_run_with(function_id, functions, run_with, plan).await;
            }
            TypedExprKind::StructInitialization(initialization) => {
                self.visit_struct_initialization(function_id, functions, initialization, plan)
                    .await;
            }
            TypedExprKind::Errored(errored) => {
                self.visit_errored(function_id, functions, errored, plan).await;
            }
        }
    }

    /// Weakens a plain value use of a `Copy` value to a shared reference.
    ///
    /// Reading a `Copy` value copies it through the reference, so the binding
    /// itself does not need to move into the closure.
    async fn refine_use_mode(&mut self, use_mode: UseMode, ty: &Interned<Ty>) -> UseMode {
        match use_mode {
            UseMode::Value(LoadKind::Implicit) if self.copy_oracle.is_copy(ty).await => {
                UseMode::Address(Mutability::Immutable)
            }
            UseMode::Value(_) | UseMode::Address(_) => use_mode,
        }
    }

    async fn visit_projection(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        operand: TypedExprID,
        use_mode: UseMode,
        plan: &mut FunctionCapturePlan,
    ) {
        self.visit_expression(function_id, functions, operand, use_mode, plan).await;
    }

    async fn visit_call(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        call: &Call,
        plan: &mut FunctionCapturePlan,
    ) {
        match call.target() {
            CallTarget::Direct { .. }
            | CallTarget::UnresolvedInstanceAssociated { .. }
            | CallTarget::EffectOperation { .. } => {}
        }
        for argument in call.arguments() {
            self.visit_expression(
                function_id,
                functions,
                *argument,
                UseMode::new_value_implicit(),
                plan,
            )
            .await;
        }
    }

    async fn visit_while(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        while_loop: &While,
        plan: &mut FunctionCapturePlan,
    ) {
        self.visit_expression(
            function_id,
            functions,
            while_loop.condition(),
            UseMode::new_value_implicit(),
            plan,
        )
        .await;
        for statement in while_loop.body() {
            self.visit_statement(function_id, functions, statement, plan).await;
        }
    }

    async fn visit_if_else(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        if_else: &IfElse,
        plan: &mut FunctionCapturePlan,
    ) {
        for conditional_arm in if_else.conditional_arms() {
            self.visit_expression(
                function_id,
                functions,
                conditional_arm.condition(),
                UseMode::new_value_implicit(),
                plan,
            )
            .await;
            self.visit_if_arm(function_id, functions, conditional_arm.arm(), plan).await;
        }
        if let Some(else_arm) = if_else.else_arm() {
            self.visit_if_arm(function_id, functions, else_arm, plan).await;
        }
    }

    async fn visit_struct_initialization(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        initialization: &StructInitialization,
        plan: &mut FunctionCapturePlan,
    ) {
        for initializer in initialization.initializers() {
            self.visit_expression(
                function_id,
                functions,
                initializer.expression(),
                UseMode::new_value_implicit(),
                plan,
            )
            .await;
        }
    }

    async fn visit_errored(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        errored: &crate::typed_expr::errored::Errored,
        plan: &mut FunctionCapturePlan,
    ) {
        for child in errored.children() {
            match child {
                crate::typed_expr::errored::ErroredChild::Expression(expression) => {
                    self.visit_expression(
                        function_id,
                        functions,
                        *expression,
                        UseMode::new_value_implicit(),
                        plan,
                    )
                    .await;
                }
                crate::typed_expr::errored::ErroredChild::Statement(statement) => {
                    self.visit_statement(function_id, functions, statement, plan).await;
                }
            }
        }
    }

    async fn visit_if_arm(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        arm: &crate::typed_expr::if_else::Arm,
        plan: &mut FunctionCapturePlan,
    ) {
        match arm {
            crate::typed_expr::if_else::Arm::Expression(expression) => {
                self.visit_expression(
                    function_id,
                    functions,
                    *expression,
                    UseMode::new_value_implicit(),
                    plan,
                )
                .await;
            }
            crate::typed_expr::if_else::Arm::Block(statements) => {
                for statement in statements {
                    self.visit_statement(function_id, functions, statement, plan).await;
                }
            }
        }
    }

    fn visit_identifier(
        &self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        binding_id: NameBindingID,
        use_mode: UseMode,
        plan: &mut FunctionCapturePlan,
    ) {
        let binding = functions.get_name_binding(binding_id);
        let source = *binding.source();
        if source.function_id() == function_id {
            return;
        }

        assert!(
            self.has_ancestor(function_id, source.function_id()),
            "a captured source should be owned by a lexical ancestor"
        );
        let mode = match use_mode {
            UseMode::Value(kind) => CaptureMode::Value(kind),
            UseMode::Address(mutability) => CaptureMode::Reference(mutability),
        };
        plan.require(CaptureRequirement::new(source, binding.ty().clone(), mode, *binding.span()));
    }

    async fn visit_run_with(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        run_with: &RunWith,
        plan: &mut FunctionCapturePlan,
    ) {
        self.visit_nested_function(function_id, functions, run_with.body(), plan).await;

        let handlers = run_with.operation_handlers().collect::<Vec<_>>();
        if handlers.is_empty() {
            return;
        }

        // Every operation callback in one handler record contributes directly
        // to the same capture layout.
        let mut shared_plan = FunctionCapturePlan::new();
        for handler in &handlers {
            self.analyze_nested_function(function_id, functions, *handler, &mut shared_plan).await;
        }

        Self::propagate_nested_captures(function_id, &shared_plan, plan);
        let shared_plan_id = self.plans.insert(shared_plan);
        for handler in handlers {
            assert!(self.function_plans.insert(handler, shared_plan_id).is_none());
        }
    }

    async fn analyze_nested_function(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        child_id: TypedFunctionID,
        plan: &mut FunctionCapturePlan,
    ) {
        assert_ne!(child_id, functions.root_id(), "the root function cannot be a nested child");
        assert!(
            self.parents.insert(child_id, function_id).is_none(),
            "each TypedAST nested function should have exactly one lexical parent"
        );

        self.analyze_function(child_id, functions, plan).await;
    }

    fn propagate_nested_captures(
        function_id: TypedFunctionID,
        child_plan: &FunctionCapturePlan,
        plan: &mut FunctionCapturePlan,
    ) {
        for (_, requirement) in child_plan.captures() {
            if requirement.source().function_id() != function_id {
                plan.require(requirement.clone());
            }
        }
    }

    async fn visit_nested_function(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        child_id: TypedFunctionID,
        plan: &mut FunctionCapturePlan,
    ) {
        let mut child_plan = FunctionCapturePlan::new();
        self.analyze_nested_function(function_id, functions, child_id, &mut child_plan).await;
        Self::propagate_nested_captures(function_id, &child_plan, plan);
        self.insert_plan(child_id, child_plan);
    }

    async fn visit_binary(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        binary: Binary,
        plan: &mut FunctionCapturePlan,
    ) {
        match binary.operator() {
            BinaryOp::Assign => {
                self.visit_expression(
                    function_id,
                    functions,
                    binary.left(),
                    UseMode::Address(Mutability::Mutable),
                    plan,
                )
                .await;
                self.visit_expression(
                    function_id,
                    functions,
                    binary.right(),
                    UseMode::new_value_implicit(),
                    plan,
                )
                .await;
            }
            BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::Plus
            | BinaryOp::Minus
            | BinaryOp::Multiply
            | BinaryOp::Divide
            | BinaryOp::And
            | BinaryOp::Or => {
                self.visit_expression(
                    function_id,
                    functions,
                    binary.left(),
                    UseMode::new_value_implicit(),
                    plan,
                )
                .await;
                self.visit_expression(
                    function_id,
                    functions,
                    binary.right(),
                    UseMode::new_value_implicit(),
                    plan,
                )
                .await;
            }
        }
    }

    fn has_ancestor(&self, mut function_id: TypedFunctionID, ancestor: TypedFunctionID) -> bool {
        while let Some(parent) = self.parents.get(&function_id).copied() {
            if parent == ancestor {
                return true;
            }
            function_id = parent;
        }
        false
    }
}

#[cfg(test)]
mod tests;

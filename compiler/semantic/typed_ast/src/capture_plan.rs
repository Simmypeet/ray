use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    capture::CaptureMode,
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::{Mutability, Ty},
};

use crate::{
    name_binding::{NameBindingID, Source},
    statement::Statement,
    typed_expr::{
        TypedExprID, TypedExprKind,
        binary::{Binary, BinaryOp},
        call::CallTarget,
        run_with::RunWith,
    },
    typed_function::{TypedFunctionID, TypedFunctionMap},
};

/// Stable capture layouts for every function in a typed AST.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct CapturePlan {
    plans: FxHashMap<TypedFunctionID, FunctionCapturePlan>,
}

impl CapturePlan {
    /// Computes capture layouts from a complete typed AST.
    #[must_use]
    pub fn analyze(functions: &TypedFunctionMap) -> Self {
        let mut analyzer = Analyzer::default();
        analyzer.analyze_function(functions.root_id(), functions);
        Self::new(analyzer.plans)
    }

    #[must_use]
    pub const fn new(plans: FxHashMap<TypedFunctionID, FunctionCapturePlan>) -> Self {
        Self { plans }
    }

    #[must_use]
    pub fn plan(&self, function_id: TypedFunctionID) -> &FunctionCapturePlan {
        self.plans.get(&function_id).expect("capture analysis should cover every reached function")
    }
}

impl MutSubstitutable for CapturePlan {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        for plan in self.plans.values_mut() {
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
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct FunctionCapturePlan {
    /// Stable closure-field order, determined by first encounter.
    captures: Vec<CaptureRequirement>,
    /// Deduplication and lookup only; never iterate this map to create a
    /// layout.
    capture_slots: FxHashMap<Source, CaptureSlot>,
}

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
}

impl Default for FunctionCapturePlan {
    fn default() -> Self { Self::new() }
}

impl FunctionCapturePlan {
    /// The concrete environment layout, including pointers for borrowed
    /// captures.
    #[must_use]
    pub fn captured_tuple(&self, engine: &TrackedEngine) -> Interned<Ty> {
        Ty::new_tuple(
            engine.intern_unsized(
                self.captures
                    .iter()
                    .map(|capture| match capture.mode {
                        CaptureMode::Value => capture.binding_ty.clone(),
                        CaptureMode::Reference(mutability) => {
                            Ty::new_pointer(capture.binding_ty.clone(), mutability, engine)
                        }
                    })
                    .collect::<Vec<_>>(),
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
    Value,
    Address(Mutability),
}

#[derive(Debug, Default)]
struct Analyzer {
    plans: FxHashMap<TypedFunctionID, FunctionCapturePlan>,
    parents: FxHashMap<TypedFunctionID, TypedFunctionID>,
    visiting: FxHashSet<TypedFunctionID>,
}

impl Analyzer {
    fn analyze_function(&mut self, function_id: TypedFunctionID, functions: &TypedFunctionMap) {
        assert!(
            self.visiting.insert(function_id),
            "TypedAST nested-function graph should not contain a cycle"
        );
        assert!(
            !self.plans.contains_key(&function_id),
            "each TypedAST function should be analyzed exactly once"
        );

        let mut plan = FunctionCapturePlan::new();
        for statement in functions.statements(function_id) {
            self.visit_statement(function_id, functions, statement, &mut plan);
        }

        assert!(self.visiting.remove(&function_id));
        self.plans.insert(function_id, plan);
    }

    fn visit_statement(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        statement: &Statement,
        plan: &mut FunctionCapturePlan,
    ) {
        match statement {
            Statement::Let(statement) => {
                if let Some(expr_id) = statement.expression() {
                    self.visit_expression(function_id, functions, expr_id, UseMode::Value, plan);
                }
            }
            Statement::Expression(expression) => {
                self.visit_expression(function_id, functions, *expression, UseMode::Value, plan);
            }
            Statement::Return(statement) => {
                if let Some(value) = statement.value() {
                    self.visit_expression(function_id, functions, value, UseMode::Value, plan);
                }
            }
        }
    }

    fn visit_expression(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        expression_id: TypedExprID,
        use_mode: UseMode,
        plan: &mut FunctionCapturePlan,
    ) {
        match functions.get_expression(function_id, expression_id).kind() {
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
                self.visit_expression(
                    function_id,
                    functions,
                    tuple_index.operand(),
                    use_mode,
                    plan,
                );
            }
            TypedExprKind::Tuple(tuple) => {
                for element in tuple.elements() {
                    self.visit_expression(function_id, functions, *element, UseMode::Value, plan);
                }
            }
            TypedExprKind::Call(call) => {
                match call.target() {
                    CallTarget::Direct { .. }
                    | CallTarget::UnresolvedInstanceAssociated { .. }
                    | CallTarget::EffectOperation { .. } => {}
                }
                for argument in call.arguments() {
                    self.visit_expression(function_id, functions, *argument, UseMode::Value, plan);
                }
            }

            TypedExprKind::Closure(lambda) => {
                self.visit_nested_function(function_id, functions, lambda.function_id(), plan);
            }
            TypedExprKind::Binary(binary) => {
                self.visit_binary(function_id, functions, *binary, plan);
            }
            TypedExprKind::IfElse(if_else) => {
                for child in
                    [if_else.condition(), if_else.then_expression(), if_else.else_expression()]
                {
                    self.visit_expression(function_id, functions, child, UseMode::Value, plan);
                }
            }
            TypedExprKind::RefOf(reference) => {
                self.visit_expression(
                    function_id,
                    functions,
                    reference.pointee(),
                    UseMode::Address(reference.mutability()),
                    plan,
                );
            }
            TypedExprKind::Deref(deref) => {
                self.visit_expression(
                    function_id,
                    functions,
                    deref.pointee(),
                    UseMode::Value,
                    plan,
                );
            }
            TypedExprKind::Paren(paren) => {
                self.visit_expression(function_id, functions, paren.expression(), use_mode, plan);
            }
            TypedExprKind::RunWith(run_with) => {
                self.visit_run_with(function_id, functions, run_with, plan);
            }
            TypedExprKind::Errored(errored) => {
                for child in errored.children() {
                    self.visit_expression(function_id, functions, *child, UseMode::Value, plan);
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
            UseMode::Value => CaptureMode::Value,
            UseMode::Address(mutability) => CaptureMode::Reference(mutability),
        };
        plan.require(CaptureRequirement::new(source, binding.ty().clone(), mode, *binding.span()));
    }

    fn visit_run_with(
        &mut self,
        function_id: TypedFunctionID,
        functions: &TypedFunctionMap,
        run_with: &RunWith,
        plan: &mut FunctionCapturePlan,
    ) {
        self.visit_nested_function(function_id, functions, run_with.body(), plan);
        for handler in run_with.operation_handlers() {
            self.visit_nested_function(function_id, functions, handler, plan);
        }
    }

    fn visit_nested_function(
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

        self.analyze_function(child_id, functions);

        let child_plan = self.plans.get(&child_id).expect("child capture plan should exist");
        for (_, requirement) in child_plan.captures() {
            if requirement.source().function_id() != function_id {
                plan.require(requirement.clone());
            }
        }
    }

    fn visit_binary(
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
                );
                self.visit_expression(function_id, functions, binary.right(), UseMode::Value, plan);
            }
            BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::Plus
            | BinaryOp::Minus
            | BinaryOp::Multiply
            | BinaryOp::Divide
            | BinaryOp::And
            | BinaryOp::Or => {
                self.visit_expression(function_id, functions, binary.left(), UseMode::Value, plan);
                self.visit_expression(function_id, functions, binary.right(), UseMode::Value, plan);
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

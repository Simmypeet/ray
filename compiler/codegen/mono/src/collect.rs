use std::collections::VecDeque;

use qbice::storage::intern::Interned;
use rayc_hash::FxHashSet;
use rayc_ir::{
    expression::{ExpressionKind, call::CallTarget},
    function::{Context as IrFunctionContext, FunctionID as IrFunctionID},
    get_ir,
    lambda::LambdaContext,
    visit::VisitType,
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{parameter::get_parameter_map, return_type::get_return_type};
use rayc_symbol::{GlobalSymbolID, symbol_kind::get_all_def_ids};
use rayc_target::TargetID;
use rayc_type::{
    poly_var::get_poly_var_map,
    subst::{Subst, Substitutable},
    ty::{Ty, TyApplicationView},
};

use crate::{MonoFunction, MonoFunctionKind, MonoProgram};

/// Collects all concrete function and tuple instantiations reachable in a
/// target.
///
/// This phase assumes semantic analysis has already rejected invalid programs.
///
/// # Panics
///
/// Panics when an error type or a type containing unresolved inference or
/// polymorphic variables reaches collection. Such a type violates the compiler
/// pipeline contract and is not a recoverable user error at this phase.
pub async fn collect_target(engine: &TrackedEngine, target_id: TargetID) -> MonoProgram {
    Collector::new(engine).collect(target_id).await
}

struct Collector<'engine> {
    engine: &'engine TrackedEngine,
    program: MonoProgram,
    pending: VecDeque<MonoFunction>,
}

impl<'engine> Collector<'engine> {
    fn new(engine: &'engine TrackedEngine) -> Self {
        Self { engine, program: MonoProgram::default(), pending: VecDeque::new() }
    }

    async fn collect(mut self, target_id: TargetID) -> MonoProgram {
        let def_ids = self.engine.get_all_def_ids(target_id).await;

        for def_id in def_ids.iter().copied() {
            let def_id = target_id.make_global(def_id);
            if self.engine.get_poly_var_map(def_id).await.is_empty() {
                self.enqueue(MonoFunction::new(def_id, Subst::new_empty()));
            }
        }

        while let Some(function) = self.pending.pop_front() {
            self.collect_function(&function).await;
        }

        self.program
    }

    fn enqueue(&mut self, function: MonoFunction) {
        assert_eq!(
            function.kind(),
            MonoFunctionKind::Def,
            "only def-level functions should enter the global mono work queue"
        );
        if self.program.insert_function(function.clone()) {
            self.pending.push_back(function);
        }
    }

    async fn collect_function(&mut self, function: &MonoFunction) {
        let parameters = self.engine.get_parameter_map(function.def_id()).await;
        for (_, parameter) in parameters.iter() {
            self.collect_substituted_type(parameter.ty(), function);
        }

        let return_type = self.engine.get_return_type(function.def_id()).await;
        self.collect_substituted_type(&return_type, function);

        let ir = self.engine.get_ir(function.def_id()).await;
        match ir.root().context() {
            IrFunctionContext::Def => {}
            IrFunctionContext::Lambda(_) => {
                panic!(
                    "compiler-internal invariant violation: root IR function should be a def \
                     while collecting {function:?}"
                );
            }
        }

        let mut visited = FxHashSet::default();
        let mut pending = VecDeque::from([ir.root_id()]);
        while let Some(ir_function_id) = pending.pop_front() {
            if !visited.insert(ir_function_id) {
                continue;
            }

            let ir_function = ir.get_function(ir_function_id);
            match ir_function.context() {
                IrFunctionContext::Def => {
                    assert_eq!(
                        ir_function_id,
                        ir.root_id(),
                        "compiler-internal invariant violation: non-root IR function should be a \
                         lambda while collecting {function:?}"
                    );
                }
                IrFunctionContext::Lambda(_) => {}
            }

            {
                let mut type_visitor = |ty: &Interned<Ty>| {
                    self.collect_substituted_type(ty, function);
                };
                ir_function.visit_types(&mut type_visitor);
            }

            for expression_id in ir_function.reachables().expressions() {
                let expression = ir_function.get_expression(expression_id);
                match expression.kind() {
                    ExpressionKind::Call(call) => match call.target() {
                        CallTarget::Direct { function_id, subst } => {
                            self.collect_call(*function_id, subst, function);
                        }
                        CallTarget::Lambda { .. } => {}
                    },
                    ExpressionKind::MakeLambda(make_lambda) => {
                        let target_id = make_lambda.function_id();
                        let target = ir.get_function(target_id);
                        let target_context = match target.context() {
                            IrFunctionContext::Def => {
                                panic!(
                                    "compiler-internal invariant violation: MakeLambda target \
                                     {target_id:?} should be a lambda while collecting \
                                     {function:?}"
                                );
                            }
                            IrFunctionContext::Lambda(context) => context,
                        };
                        assert_eq!(
                            make_lambda.captures().len(),
                            target_context.captures().len(),
                            "compiler-internal invariant violation: MakeLambda targeting \
                             {target_id:?} has a capture-count mismatch while collecting \
                             {function:?}"
                        );
                        self.validate_lambda_signature(
                            expression.ty(),
                            target_context,
                            function,
                            target_id,
                        );
                        self.program.insert_function(MonoFunction::new_lambda(function, target_id));
                        pending.push_back(target_id);
                    }
                    ExpressionKind::Error
                    | ExpressionKind::Literal(_)
                    | ExpressionKind::RefOf(_)
                    | ExpressionKind::Load(_)
                    | ExpressionKind::Phi(_)
                    | ExpressionKind::Binary(_)
                    | ExpressionKind::Tuple(_) => {}
                }
            }
        }
    }

    fn validate_lambda_signature(
        &self,
        expression_ty: &Interned<Ty>,
        context: &LambdaContext,
        owner: &MonoFunction,
        function_id: IrFunctionID,
    ) {
        let expression_ty = expression_ty.apply_subst_or_clone(owner.subst(), self.engine);
        let lambda = match &*expression_ty {
            Ty::Application(application) => match application.view() {
                TyApplicationView::Lambda(lambda) => lambda,
                TyApplicationView::Primitive(_)
                | TyApplicationView::Tuple(_)
                | TyApplicationView::Pointer(_)
                | TyApplicationView::Error => {
                    panic!(
                        "compiler-internal invariant violation: MakeLambda targeting \
                         {function_id:?} should have a lambda expression type while collecting \
                         {owner:?}, found {expression_ty:?}"
                    );
                }
            },
            Ty::Inference(_) | Ty::PolyVar(_) => {
                panic!(
                    "compiler-internal invariant violation: MakeLambda targeting {function_id:?} \
                     should have a concrete lambda expression type while collecting {owner:?}, \
                     found {expression_ty:?}"
                );
            }
        };

        let parameters = context.parameters();
        assert_eq!(
            lambda.parameter_types().len(),
            parameters.len(),
            "compiler-internal invariant violation: MakeLambda targeting {function_id:?} has a \
             parameter-count mismatch while collecting {owner:?}"
        );
        for (index, (expression_parameter, (_, target_parameter))) in
            lambda.parameter_types().iter().zip(parameters).enumerate()
        {
            let target_parameter =
                target_parameter.ty().apply_subst_or_clone(owner.subst(), self.engine);
            assert_eq!(
                expression_parameter, &target_parameter,
                "compiler-internal invariant violation: MakeLambda targeting {function_id:?} has \
                 a type mismatch for parameter {index} while collecting {owner:?}"
            );
        }

        let target_return = context.return_ty().apply_subst_or_clone(owner.subst(), self.engine);
        assert_eq!(
            lambda.return_type(),
            &target_return,
            "compiler-internal invariant violation: MakeLambda targeting {function_id:?} has a \
             return-type mismatch while collecting {owner:?}"
        );
    }

    fn collect_call(&mut self, def_id: GlobalSymbolID, call_subst: &Subst, caller: &MonoFunction) {
        self.enqueue(caller.instantiate_call(def_id, call_subst, self.engine));
    }

    fn collect_substituted_type(&mut self, ty: &Interned<Ty>, function: &MonoFunction) {
        let ty = ty.apply_subst_or_clone(function.subst(), self.engine);
        self.collect_concrete_type(&ty, function);
    }

    fn collect_concrete_type(&mut self, ty: &Interned<Ty>, function: &MonoFunction) {
        for ty in Ty::recursive_iter(ty) {
            match &**ty {
                Ty::Application(application) => match application.view() {
                    TyApplicationView::Primitive(_) | TyApplicationView::Pointer(_) => {}
                    TyApplicationView::Tuple(tuple) => {
                        self.program
                            .insert_tuple(self.engine.intern_unsized(tuple.args().to_vec()));
                    }
                    TyApplicationView::Lambda(lambda) => {
                        self.program.insert_lambda_type(
                            self.engine.intern_unsized(lambda.parameter_types().to_vec()),
                            lambda.return_type().clone(),
                        );
                    }
                    TyApplicationView::Error => {
                        panic!(
                            "compiler-internal invariant violation: error type reached \
                             monomorphization while collecting {function:?}"
                        );
                    }
                },
                Ty::Inference(_) | Ty::PolyVar(_) => {
                    panic!(
                        "compiler-internal invariant violation: non-concrete type reached \
                         monomorphization while collecting {function:?}: {ty:?}"
                    );
                }
            }
        }
    }
}

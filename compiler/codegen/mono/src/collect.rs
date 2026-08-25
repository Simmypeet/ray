use std::collections::VecDeque;

use qbice::storage::intern::Interned;
use rayc_ir::{
    get_ir,
    ir_expr::{IRExpr, IRExprKind, call::CallTarget},
    visit::{VisitExpr, VisitType},
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{parameter::get_parameter_map, return_type::get_return_type};
use rayc_symbol::{
    GlobalSymbolID,
    symbol_kind::{SymbolKind, get_all_def_ids, get_symbol_kind},
};
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
    seen: rayc_hash::FxHashSet<MonoFunction>,
}

impl<'engine> Collector<'engine> {
    fn new(engine: &'engine TrackedEngine) -> Self {
        Self {
            engine,
            program: MonoProgram::default(),
            pending: VecDeque::new(),
            seen: rayc_hash::FxHashSet::default(),
        }
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
            match self.engine.get_symbol_kind(function.def_id()).await {
                SymbolKind::Def => {
                    self.program.insert_function(function.clone());
                    self.collect_function(&function).await;
                }
                SymbolKind::ExternDef => {
                    self.program.insert_function(MonoFunction::new_extern(
                        function.def_id(),
                        function.subst().clone(),
                    ));
                }
                SymbolKind::Module => panic!("module reached monomorphization as a callable"),
            }
        }

        self.program
    }

    fn enqueue(&mut self, function: MonoFunction) {
        assert_eq!(
            function.kind(),
            MonoFunctionKind::Def,
            "only def-level functions should enter the global mono work queue"
        );
        if self.seen.insert(function.clone()) {
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

        ir.visit_types(&mut |ty: &Interned<Ty>| {
            self.collect_substituted_type(ty, function);
        });

        ir.visit_exprs(&mut |_function_id, _expression_id, expression: &IRExpr| match expression
            .kind()
        {
            IRExprKind::Call(call) => match call.target() {
                CallTarget::Direct { function_id, subst } => {
                    self.collect_call(*function_id, subst, function);
                }
                CallTarget::Lambda { .. } => {}
            },
            IRExprKind::MakeLambda(lambda) => {
                self.program
                    .insert_function(MonoFunction::new_lambda(function, lambda.function_id()));
            }
            IRExprKind::Error
            | IRExprKind::Literal(_)
            | IRExprKind::RefOf(_)
            | IRExprKind::Load(_)
            | IRExprKind::Phi(_)
            | IRExprKind::Binary(_)
            | IRExprKind::Tuple(_) => {}
        });
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

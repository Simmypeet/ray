use std::sync::Arc;

use qbice::{Identifiable, StableHash, storage::intern::Interned};
use rayc_ir::{get_ir, ir_function::IRFunctionMap, ir_lambda::Capture};
use rayc_mono::{MonoFunction, MonoProgram};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{
    parameter::{ParameterID, get_parameter_map},
    return_type::get_return_type,
};
use rayc_symbol::{
    GlobalSymbolID,
    name::get_name,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::is_variadic_def,
};
use rayc_type::{
    subst::{Subst, Substitutable},
    ty::Ty,
};

use crate::{
    c_ty::{CAbiReturn, CTy},
    context::instantiation::CTupleID,
};

pub mod instantiation;

#[derive(Debug, Clone)]
pub struct Context {
    engine: TrackedEngine,
    mono_program: MonoProgram,
}

impl Context {
    pub const fn new(engine: TrackedEngine, mono_program: MonoProgram) -> Self {
        Self { engine, mono_program }
    }
}

impl Context {
    pub fn intern<T>(&self, value: T) -> Interned<T>
    where
        T: StableHash + Identifiable + Send + Sync + 'static,
    {
        self.engine.intern(value)
    }

    pub fn intern_unsized<
        T: StableHash + Identifiable + Send + Sync + 'static + ?Sized,
        Q: std::borrow::Borrow<T> + Send + Sync + 'static,
    >(
        &self,
        value: Q,
    ) -> Interned<T>
    where
        Arc<T>: From<Q>,
    {
        self.engine.intern_unsized(value)
    }

    pub async fn get_def_name(&self, def_id: GlobalSymbolID) -> Interned<str> {
        self.engine.get_name(def_id).await
    }

    pub async fn get_symbol_kind(&self, def_id: GlobalSymbolID) -> SymbolKind {
        self.engine.get_symbol_kind(def_id).await
    }

    pub async fn is_variadic_def(&self, def_id: GlobalSymbolID) -> bool {
        self.engine.is_variadic_def(def_id).await
    }

    pub async fn get_ir(&self, def_id: GlobalSymbolID) -> Interned<IRFunctionMap> {
        self.engine.get_ir(def_id).await
    }

    pub fn get_unit_tuple_id(&self) -> CTupleID { self.get_ctuple_id(&[]) }

    pub fn instantiate_call(
        &self,
        caller: &MonoFunction,
        def_id: GlobalSymbolID,
        call_subst: &Subst,
    ) -> MonoFunction {
        caller.instantiate_call(def_id, call_subst, &self.engine)
    }

    pub async fn get_mono_return_cty(&self, function: &MonoFunction) -> Interned<CTy> {
        let ty = self.engine.get_return_type(function.def_id()).await;
        let ty = ty.apply_subst_or_clone(function.subst(), &self.engine);
        self.ty_to_cty(&ty)
    }

    pub async fn get_extern_return(&self, function: &MonoFunction) -> CAbiReturn {
        let ty = self.engine.get_return_type(function.def_id()).await;
        let ty = ty.apply_subst_or_clone(function.subst(), &self.engine);
        let is_unit = matches!(
            &*ty,
            Ty::Application(application)
                if matches!(application.view(), rayc_type::ty::TyApplicationView::Tuple(tuple) if tuple.args().is_empty())
        );
        if is_unit { CAbiReturn::Void } else { CAbiReturn::Value(self.ty_to_cty(&ty)) }
    }

    pub async fn get_mono_parameters(
        &self,
        function: &MonoFunction,
    ) -> Vec<(ParameterID, Interned<CTy>)> {
        let parameters = self.engine.get_parameter_map(function.def_id()).await;
        parameters
            .iter()
            .map(|(parameter_id, parameter)| {
                let ty = parameter.ty().apply_subst_or_clone(function.subst(), &self.engine);
                (parameter_id, self.ty_to_cty(&ty))
            })
            .collect()
    }

    pub fn instantiate_type(&self, ty: &Interned<Ty>, function: &MonoFunction) -> Interned<Ty> {
        ty.apply_subst_or_clone(function.subst(), &self.engine)
    }

    pub fn instantiate_capture_pointer_type(
        &self,
        capture: &Capture,
        function: &MonoFunction,
    ) -> Interned<Ty> {
        let pointer_ty = capture.pointer_ty(&self.engine);
        pointer_ty.apply_subst_or_clone(function.subst(), &self.engine)
    }
}

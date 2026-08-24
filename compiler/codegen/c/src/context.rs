use std::sync::Arc;

use qbice::{Identifiable, StableHash, storage::intern::Interned};
use rayc_ir::{function::Function, get_ir};
use rayc_mono::MonoProgram;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{GlobalSymbolID, name::get_name};

use crate::context::instantiation::{CTupleID, InstantiationTable};

pub mod instantiation;

#[derive(Debug, Clone)]
pub struct Context {
    inst_table: InstantiationTable,
    engine: TrackedEngine,
    mono_program: MonoProgram,
}

impl Context {
    pub fn new(engine: TrackedEngine, mono_program: MonoProgram) -> Self {
        Self { inst_table: InstantiationTable::default(), engine, mono_program }
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

    pub async fn get_ir(&self, def_id: GlobalSymbolID) -> Interned<Function> {
        self.engine.get_ir(def_id).await
    }

    pub fn get_unit_tuple_id(&self) -> CTupleID { self.get_ctuple_id(&[]) }
}

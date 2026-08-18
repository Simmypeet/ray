use std::sync::Arc;

use qbice::{Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;

use crate::generator::instantiation::InstantiationTable;

pub mod instantiation;

#[derive(Debug, Clone)]
pub struct Generator {
    inst_table: InstantiationTable,
    engine: TrackedEngine,
}

impl Generator {
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
}

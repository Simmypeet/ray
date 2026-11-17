//! The buffer generated C is written into, and the trait of the C syntax
//! whose spelling needs the engine.

use std::{
    fmt::{Display, Write as _},
    mem,
};

use rayc_mono_ir::{
    layout::repr::{TypeRepr, get_type_repr},
    ty::{AggregateType, MonoType},
};
use rayc_qbice::TrackedEngine;

use crate::aggregates::AggregateRegistry;

const WRITE_TO_STRING: &str = "writing to a String cannot fail";

/// C syntax whose spelling may need the engine, such as a type: the
/// representation of a type decides how it is spelled, and only a query
/// knows.
///
/// It is the asynchronous counterpart of [`Display`]. C syntax that never
/// needs the engine, such as a name, implements [`Display`] instead.
pub(crate) trait WriteC {
    /// Writes `self` into `writer`.
    fn write_c(&self, writer: &mut CWriter<'_>) -> impl Future<Output = ()> + Send;
}

/// Where the C of one function is written, and which aggregate types it has
/// spelled so far.
///
/// Spelling an aggregate records it, so every aggregate in the output is
/// known to need a `typedef`; nothing has to be registered beforehand.
#[derive(Debug)]
pub(crate) struct CWriter<'engine> {
    engine: &'engine TrackedEngine,
    out: String,
    aggregates: AggregateRegistry,
}

impl<'engine> CWriter<'engine> {
    pub(crate) fn new(engine: &'engine TrackedEngine) -> Self {
        Self { engine, out: String::new(), aggregates: AggregateRegistry::default() }
    }

    pub(crate) const fn engine(&self) -> &'engine TrackedEngine { self.engine }

    /// Writes text that needs no query: names, punctuation, operators.
    pub(crate) fn push(&mut self, text: impl Display) {
        write!(self.out, "{text}").expect(WRITE_TO_STRING);
    }

    /// Writes `item`, which may query the engine.
    pub(crate) async fn write(&mut self, item: &impl WriteC) { item.write_c(self).await }

    /// How a value of `ty` is represented.
    pub(crate) async fn repr(&self, ty: &MonoType) -> TypeRepr {
        self.engine.get_type_repr(ty).await
    }

    /// Whether a value of `ty` holds nothing, so it is never stored, passed
    /// or returned.
    pub(crate) async fn is_zero_sized(&self, ty: &MonoType) -> bool {
        self.repr(ty).await.is_zero_sized()
    }

    /// Records that the output spells `aggregate`.
    pub(super) fn record_aggregate(&mut self, aggregate: &AggregateType) {
        self.aggregates.insert(aggregate);
    }

    /// Takes the text written so far, leaving the buffer empty; the recorded
    /// aggregates are kept.
    pub(crate) fn take_text(&mut self) -> String { mem::take(&mut self.out) }

    /// The text written since the last [`Self::take_text`], and every
    /// aggregate spelled.
    pub(crate) fn finish(self) -> (String, Vec<AggregateType>) {
        (self.out, self.aggregates.into_aggregates().collect())
    }
}

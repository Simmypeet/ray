use std::{borrow::Borrow, fmt::Debug, hash::Hash, marker::PhantomData, sync::Arc};

use qbice::{Decode, Encode, Executor, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_qbice::{Config, TrackedEngine};

macro_rules! impl_key {
    ($k:ident, $key:ident, $value:ty) => {
        #[derive(Identifiable, StableHash, Encode, Decode)]
        pub struct $key<$k> {
            pub id: $k,
            _marker: PhantomData<$k>,
        }

        impl<$k> $key<$k> {
            /// Creates a new key for the given ID.
            #[must_use]
            #[allow(unused)]
            pub const fn new(id: $k) -> Self { Self { id, _marker: PhantomData } }
        }

        impl<$k: Debug> Debug for $key<$k> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("$key").field("id", &self.id).finish_non_exhaustive()
            }
        }

        impl<$k: Clone> Clone for $key<$k> {
            fn clone(&self) -> Self { Self { id: self.id.clone(), _marker: PhantomData } }
        }

        impl<$k: PartialEq> PartialEq for $key<$k> {
            fn eq(&self, other: &Self) -> bool { self.id == other.id }
        }

        impl<$k: Eq> Eq for $key<$k> {}

        impl<$k: Ord> Ord for $key<$k> {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering { self.id.cmp(&other.id) }
        }

        impl<$k: PartialOrd> PartialOrd for $key<$k> {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                self.id.partial_cmp(&other.id)
            }
        }

        impl<$k: Hash> Hash for $key<$k> {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) { self.id.hash(state); }
        }

        impl<$k: $crate::build::Build> Query for $key<$k> {
            type Value = $value;
        }
    };
}

impl_key!(T, Key, Output<T>);
impl_key!(T, DiagnosticKey, Interned<[T::Diagnostic]>);
impl_key!(T, ObligationKey, Interned<[rayc_resolution::Obligation]>);

#[derive(Debug, Default)]
#[allow(missing_copy_implementations)]
pub struct BuildExecutor;

impl<T: Build> Executor<Key<T>, Config> for BuildExecutor {
    async fn execute(&self, key: &Key<T>, engine: &TrackedEngine) -> Output<T> {
        T::execute(engine, &key.id).await
    }
}

#[derive(Debug, Default)]
#[allow(missing_copy_implementations)]
pub struct DiagnosticExecutor;

impl<T: Build> Executor<DiagnosticKey<T>, Config> for DiagnosticExecutor {
    async fn execute(
        &self,
        key: &DiagnosticKey<T>,
        engine: &TrackedEngine,
    ) -> Interned<[T::Diagnostic]> {
        engine.query(&Key::<T>::new(key.id.clone())).await.diagnostics
    }
}

#[derive(Debug, Default)]
#[allow(missing_copy_implementations)]
pub struct ObligationExecutor;

impl<T: Build> Executor<ObligationKey<T>, Config> for ObligationExecutor {
    async fn execute(
        &self,
        key: &ObligationKey<T>,
        engine: &TrackedEngine,
    ) -> Interned<[rayc_resolution::Obligation]> {
        engine.query(&Key::<T>::new(key.id.clone())).await.obligations
    }
}

#[derive(Debug, Default)]
#[allow(missing_copy_implementations)]
pub struct ElementExtractExecutor;

impl<T: Build> Executor<T, Config> for ElementExtractExecutor {
    async fn execute(&self, key: &T, engine: &TrackedEngine) -> T::Value {
        let build_result = engine.query(&Key::<T>::new(key.clone())).await;

        build_result.item
    }

    fn execution_style() -> qbice::ExecutionStyle { qbice::ExecutionStyle::Normal }
}

/// A helper structs that groups the side-effects of building a query.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash)]
pub struct Output<T: Build> {
    /// The main result of the query.
    pub item: T::Value,

    /// The diagnostics produced while building the query.
    pub diagnostics: Interned<[T::Diagnostic]>,

    /// Deferred obligations generated while building this element.
    pub obligations: Interned<[rayc_resolution::Obligation]>,
}

impl<T: Build> Output<T> {
    /// Attaches the obligations to be solved after semantic construction.
    #[must_use]
    pub fn with_obligations(
        mut self,
        obligations: Vec<rayc_resolution::Obligation>,
        engine: &TrackedEngine,
    ) -> Self {
        self.obligations = engine.intern_unsized(obligations);
        self
    }

    /// Creates a new output with the given item and empty diagnostics and
    /// obligations.
    #[must_use]
    pub fn new(item: T::Value, engine: &TrackedEngine) -> Self {
        Self {
            item,
            diagnostics: engine.intern_unsized([]),
            obligations: engine.intern_unsized::<[rayc_resolution::Obligation], _>([]),
        }
    }

    /// Creates a new output with the given item and diagnostics, without
    /// obligations.
    #[must_use]
    pub fn new_with<
        Q: Borrow<[T::Diagnostic]> + Send + Sync + 'static,
        R: Borrow<[rayc_resolution::Obligation]> + Send + Sync + 'static,
    >(
        item: T::Value,
        diagnostics: Q,
        obligations: R,
        engine: &TrackedEngine,
    ) -> Self
    where
        Arc<[T::Diagnostic]>: From<Q>,
        Arc<[rayc_resolution::Obligation]>: From<R>,
    {
        Self {
            item,
            diagnostics: engine.intern_unsized(diagnostics),
            obligations: engine.intern_unsized(obligations),
        }
    }
}

impl<T: Encode + Build> Encode for Output<T> {
    fn encode<E: qbice::serialize::Encoder + ?Sized>(
        &self,
        encoder: &mut E,
        plugin: &qbice::serialize::Plugin,
        session: &mut qbice::serialize::session::Session,
    ) -> std::io::Result<()> {
        self.item.encode(encoder, plugin, session)?;
        self.diagnostics.encode(encoder, plugin, session)?;
        self.obligations.encode(encoder, plugin, session)?;
        Ok(())
    }
}

impl<T: Decode + Build> Decode for Output<T> {
    fn decode<D: qbice::serialize::Decoder + ?Sized>(
        decoder: &mut D,
        plugin: &qbice::serialize::Plugin,
        session: &mut qbice::serialize::session::Session,
    ) -> std::io::Result<Self> {
        let item = T::Value::decode(decoder, plugin, session)?;
        let diagnostics = Interned::<[T::Diagnostic]>::decode(decoder, plugin, session)?;

        let obligations =
            Interned::<[rayc_resolution::Obligation]>::decode(decoder, plugin, session)?;
        Ok(Self { item, diagnostics, obligations })
    }
}

impl<T: Build> Clone for Output<T> {
    fn clone(&self) -> Self {
        Self {
            item: self.item.clone(),
            diagnostics: self.diagnostics.clone(),
            obligations: self.obligations.clone(),
        }
    }
}

pub trait Build: Query {
    type Diagnostic: Debug + StableHash + Send + Sync + 'static + Identifiable + Encode + Decode;

    fn execute<'x, 'y>(
        engine: &'x TrackedEngine,
        key: &'y Self,
    ) -> impl std::future::Future<Output = Output<Self>> + use<'x, 'y, Self> + Send;
}

/// Registers the necessary executors for the given type.
#[macro_export]
macro_rules! register_build {
    ($ty:ty) => {
        const _: () = {
            #[::linkme::distributed_slice(::rayc_qbice::RAY_PROGRAM)]
            static BUILD_EXECUTOR: ::qbice::program::Registration<::rayc_qbice::Config> =
                ::qbice::program::Registration::<::rayc_qbice::Config>::new::<
                    $crate::build::Key<$ty>,
                    $crate::build::BuildExecutor,
                >();

            #[::linkme::distributed_slice(::rayc_qbice::RAY_PROGRAM)]
            static DIAGNOSTIC_EXECUTOR: ::qbice::program::Registration<::rayc_qbice::Config> =
                ::qbice::program::Registration::<::rayc_qbice::Config>::new::<
                    $crate::build::DiagnosticKey<$ty>,
                    $crate::build::DiagnosticExecutor,
                >();

            #[::linkme::distributed_slice(::rayc_qbice::RAY_PROGRAM)]
            static OBLIGATION_EXECUTOR: ::qbice::program::Registration<::rayc_qbice::Config> =
                ::qbice::program::Registration::<::rayc_qbice::Config>::new::<
                    $crate::build::ObligationKey<$ty>,
                    $crate::build::ObligationExecutor,
                >();

            #[::linkme::distributed_slice(::rayc_qbice::RAY_PROGRAM)]
            static ELEMENT_EXTRACT_EXECUTOR: ::qbice::program::Registration<::rayc_qbice::Config> =
                ::qbice::program::Registration::<::rayc_qbice::Config>::new::<
                    $ty,
                    $crate::build::ElementExtractExecutor,
                >();
        };
    };
}

pub use register_build;

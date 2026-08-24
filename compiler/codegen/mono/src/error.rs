use qbice::storage::intern::Interned;
use rayc_type::ty::Ty;
use thiserror::Error;

use crate::MonoFunction;

/// An internal failure encountered while collecting concrete instantiations.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MonoError {
    /// A type still contained an inference or polymorphic variable after the
    /// current function substitution was applied.
    #[error("non-concrete type while collecting {function:?}: {ty:?}")]
    NonConcreteType { function: MonoFunction, ty: Interned<Ty> },

    /// An error type reached code generation after semantic checking.
    #[error("error type while collecting {function:?}")]
    ErrorType { function: MonoFunction },
}

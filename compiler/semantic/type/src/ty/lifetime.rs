//! Lifetimes that are not lifetime parameters.
//!
//! A lifetime parameter is an ordinary [`Ty::PolyVar`](super::Ty::PolyVar) of
//! kind [`TyKind::Lifetime`](super::TyKind::Lifetime), and a lifetime that
//! failed to resolve is an error of that kind. Every other lifetime is a
//! [`Ty::Lifetime`](super::Ty::Lifetime).

use std::fmt;

use qbice::{Decode, Encode, StableHash};
use rayc_arena::ID;

/// A region variable of the IR type check. Nothing creates one yet.
#[derive(Debug, Clone, Copy)]
pub struct Region;

/// Identifies a region variable within the definition being borrow checked.
pub type RegionID = ID<Region>;

/// A lifetime in the interface of a nested IR function: a lambda, a thunk or
/// an operation handler.
#[derive(Debug, Clone, Copy)]
pub struct ExternalRegion;

/// Identifies an external lifetime by its position in the interface of the
/// nested IR function whose types mention it.
pub type ExternalRegionID = ID<ExternalRegion>;

/// A lifetime that is not a lifetime parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Lifetime {
    /// The `'static` lifetime.
    Static,

    /// A lifetime inside a function body before borrow checking. Type
    /// inference ignores lifetimes, so every lifetime it creates is erased and
    /// only the borrow checker gives it a region.
    Erased,

    /// A region variable, only created by the IR type check.
    ///
    /// Region variables are deliberately not inference variables: type
    /// inference binds inference variables through substitution, while
    /// region variables are never bound and only collect outlives
    /// constraints.
    Region(RegionID),

    /// A lifetime in the interface of a nested IR function, such as the
    /// lifetime of a by-reference capture.
    ///
    /// It acts as a lifetime parameter of the nested function and is only
    /// replaced by substitution: with a universal region when the nested
    /// function is checked, and with a region of the parent where the parent
    /// creates it. Its ID is only meaningful within that one function.
    External(ExternalRegionID),
}

impl fmt::Display for Lifetime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Static => f.write_str("'static"),
            Self::Erased => f.write_str("'_"),
            Self::Region(region) => write!(f, "'?{}", region.index()),
            Self::External(external) => write!(f, "'^{}", external.index()),
        }
    }
}

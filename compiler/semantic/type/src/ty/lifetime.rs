//! Lifetimes that are not lifetime parameters.
//!
//! A lifetime parameter is an ordinary [`Ty::PolyVar`](super::Ty::PolyVar) of
//! kind [`TyKind::Lifetime`](super::TyKind::Lifetime), and a lifetime that
//! failed to resolve is an error of that kind. Every other lifetime is a
//! [`Ty::Lifetime`](super::Ty::Lifetime).

use qbice::{Decode, Encode, StableHash};
use rayc_arena::ID;

/// A region variable of the IR type check. Nothing creates one yet.
#[derive(Debug, Clone, Copy)]
pub struct Region;

/// Identifies a region variable within the definition being borrow checked.
pub type RegionID = ID<Region>;

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
}

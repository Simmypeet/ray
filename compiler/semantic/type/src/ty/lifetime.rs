//! Lifetimes other than lifetime parameters, which are ordinary
//! [`Ty::PolyVar`](super::Ty::PolyVar)s.

use std::fmt;

use qbice::{Decode, Encode, StableHash};
use rayc_arena::ID;

/// A region variable of the IR type check. Nothing creates one yet.
#[derive(Debug, Clone, Copy)]
pub struct Region;

/// Identifies a region variable within the definition being borrow checked.
pub type RegionID = ID<Region>;

/// A lifetime in the interface of a nested IR function: a closure, a thunk or
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

    /// A lifetime before borrow checking, since type inference ignores
    /// lifetimes.
    Erased,

    /// A region variable from the IR type check, kept apart from inference
    /// variables because it is never bound and only collects constraints.
    Region(RegionID),

    /// A lifetime in a nested IR function's interface (e.g. a by-reference
    /// capture), acting as one of its lifetime parameters.
    External(ExternalRegionID),
}

impl Lifetime {
    /// Returns whether this lifetime is universal: a region the function
    /// mentioning it does not choose, but is given.
    #[must_use]
    pub const fn is_universal(&self) -> bool {
        match self {
            Self::Static | Self::External(_) => true,
            Self::Erased | Self::Region(_) => false,
        }
    }

    /// Returns whether this lifetime is an erased lifetime.
    #[must_use]
    pub const fn is_erased(&self) -> bool { matches!(self, Self::Erased) }
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

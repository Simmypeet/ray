use std::borrow::Borrow;

use qbice::storage::intern::Interned;
use rayc_hash::FxHashSet;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{subst::Subst, ty::Ty};

/// A concrete instantiation of a Ray function definition.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonoFunction {
    def_id: GlobalSymbolID,
    subst: Subst,
}

impl MonoFunction {
    /// Creates a function-instantiation key.
    #[must_use]
    pub const fn new(def_id: GlobalSymbolID, subst: Subst) -> Self { Self { def_id, subst } }

    /// Returns the source definition instantiated by this item.
    #[must_use]
    pub const fn def_id(&self) -> GlobalSymbolID { self.def_id }

    /// Returns the concrete substitution forming part of this item's identity.
    #[must_use]
    pub const fn subst(&self) -> &Subst { &self.subst }
}

/// A concrete Ray tuple type encountered by monomorphization.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonoTuple {
    ty: Interned<Ty>,
}

impl MonoTuple {
    /// Returns the full concrete Ray tuple type.
    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }
}

impl Borrow<Interned<Ty>> for MonoTuple {
    fn borrow(&self) -> &Interned<Ty> { &self.ty }
}

/// The immutable inventory of concrete target instantiations.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MonoProgram {
    functions: FxHashSet<MonoFunction>,
    tuples: FxHashSet<MonoTuple>,
}

impl MonoProgram {
    pub(crate) fn insert_function(&mut self, function: MonoFunction) -> bool {
        self.functions.insert(function)
    }

    pub(crate) fn insert_tuple(&mut self, ty: Interned<Ty>) {
        self.tuples.insert(MonoTuple { ty });
    }

    /// Iterates over the concrete function instantiations.
    #[must_use]
    pub fn functions(&self) -> impl ExactSizeIterator<Item = &'_ MonoFunction> {
        self.functions.iter()
    }

    /// Iterates over the concrete tuple instantiations.
    #[must_use]
    pub fn tuples(&self) -> impl ExactSizeIterator<Item = &'_ MonoTuple> { self.tuples.iter() }

    /// Retrieves a collected function instantiation.
    #[must_use]
    pub fn function(&self, function: &MonoFunction) -> Option<&MonoFunction> {
        self.functions.get(function)
    }

    /// Retrieves a collected tuple instantiation by its concrete Ray type.
    #[must_use]
    pub fn tuple(&self, ty: &Interned<Ty>) -> Option<&MonoTuple> { self.tuples.get(ty) }
}

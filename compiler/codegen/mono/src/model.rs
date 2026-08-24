use qbice::storage::intern::Interned;
use rayc_hash::FxHashSet;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    subst::{MutSubstitutable, Subst},
    ty::Ty,
};

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

    /// Instantiates a call made from this concrete function.
    #[must_use]
    pub fn instantiate_call(
        &self,
        def_id: GlobalSymbolID,
        call_subst: &Subst,
        engine: &TrackedEngine,
    ) -> Self {
        let mut subst = call_subst.clone();
        subst.apply_mut_subst(self.subst(), engine);
        Self::new(def_id, subst)
    }
}

/// A concrete Ray tuple type encountered by monomorphization.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonoTuple {
    args: Interned<[Interned<Ty>]>,
}

impl MonoTuple {
    /// Iterates over the tuple's concrete type arguments.
    #[must_use]
    pub fn args(&self) -> impl ExactSizeIterator<Item = &'_ Interned<Ty>> { self.args.iter() }
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

    pub(crate) fn insert_tuple(&mut self, args: Interned<[Interned<Ty>]>) {
        self.tuples.insert(MonoTuple { args });
    }

    /// Iterates over the concrete function instantiations.
    #[must_use]
    pub fn functions(&self) -> impl ExactSizeIterator<Item = &'_ MonoFunction> {
        self.functions.iter()
    }

    /// Iterates over the concrete tuple instantiations.
    #[must_use]
    pub fn tuples(&self) -> impl ExactSizeIterator<Item = &'_ MonoTuple> { self.tuples.iter() }
}

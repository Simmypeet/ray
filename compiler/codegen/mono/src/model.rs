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

/// A concrete lambda signature encountered by monomorphization.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonoLambdaType {
    parameter_types: Interned<[Interned<Ty>]>,
    return_type: Interned<Ty>,
}

impl MonoLambdaType {
    pub(crate) const fn new(
        parameter_types: Interned<[Interned<Ty>]>,
        return_type: Interned<Ty>,
    ) -> Self {
        Self { parameter_types, return_type }
    }

    /// Iterates over the signature's concrete parameter types.
    #[must_use]
    pub fn parameter_types(&self) -> impl ExactSizeIterator<Item = &'_ Interned<Ty>> {
        self.parameter_types.iter()
    }

    /// Returns the signature's concrete return type.
    #[must_use]
    pub const fn return_type(&self) -> &Interned<Ty> { &self.return_type }
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
    lambda_types: FxHashSet<MonoLambdaType>,
}

impl MonoProgram {
    pub(crate) fn insert_function(&mut self, function: MonoFunction) -> bool {
        self.functions.insert(function)
    }

    pub(crate) fn insert_tuple(&mut self, args: Interned<[Interned<Ty>]>) {
        self.tuples.insert(MonoTuple { args });
    }

    pub(crate) fn insert_lambda_type(
        &mut self,
        parameter_types: Interned<[Interned<Ty>]>,
        return_type: Interned<Ty>,
    ) {
        self.lambda_types.insert(MonoLambdaType::new(parameter_types, return_type));
    }

    /// Iterates over the concrete function instantiations.
    #[must_use]
    pub fn functions(&self) -> impl ExactSizeIterator<Item = &'_ MonoFunction> {
        self.functions.iter()
    }

    /// Iterates over the concrete tuple instantiations.
    #[must_use]
    pub fn tuples(&self) -> impl ExactSizeIterator<Item = &'_ MonoTuple> { self.tuples.iter() }

    /// Iterates over the concrete lambda signatures.
    #[must_use]
    pub fn lambda_types(&self) -> impl ExactSizeIterator<Item = &'_ MonoLambdaType> {
        self.lambda_types.iter()
    }
}

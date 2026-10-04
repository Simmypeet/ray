//! C identifiers for generated items.
//!
//! Every name is a small `Copy` value that renders itself through
//! [`fmt::Display`], so names are written straight into the output buffer
//! instead of being materialized as intermediate `String`s. Names derived from
//! semantic identities embed a stable hash, which keeps them deterministic
//! across compilations and independent of discovery order.

use std::fmt;

use qbice::{
    StableHash,
    stable_hash::{Sip128Hasher, StableHasher},
};
use rayc_mono_ir::{
    MonoClosureInstance, MonoDefInstance, MonoFragmentInstance, MonoIR, MonoNominalDropInstance,
    cfg::BlockID,
    function::{LocalID, MonoFunctionID, MonoFunctionKind},
    place::Projection,
    ty::AggregateType,
};
use rayc_semantic_element::struct_body::FieldID;
use rayc_symbol::GlobalSymbolID;

/// A domain-separated 128-bit stable hash, rendered in base 62.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct StableID(u128);

impl StableID {
    fn of<T: StableHash + ?Sized>(domain: &'static str, value: &T) -> Self {
        let mut hasher = Sip128Hasher::default();
        domain.stable_hash(&mut hasher);
        value.stable_hash(&mut hasher);
        Self(hasher.finish())
    }
}

impl fmt::Display for StableID {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        const ALPHABET: &[u8; 62] =
            b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        const MAX_ENCODED_LENGTH: usize = 22;

        // Fill the digits from the least significant end of a stack buffer.
        let mut value = self.0;
        let mut encoded = [0; MAX_ENCODED_LENGTH];
        let mut start = encoded.len();
        loop {
            start -= 1;
            encoded[start] = ALPHABET[(value % 62) as usize];
            value /= 62;
            if value == 0 {
                break;
            }
        }

        let encoded = str::from_utf8(&encoded[start..]).map_err(|_| fmt::Error)?;
        formatter.write_str(encoded)
    }
}

/// The C struct tag of an aggregate type, e.g. `RayTuple_<id>`.
///
/// The derived ordering follows the stable ID, which is what the aggregate
/// definitions are sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct AggregateName {
    id: StableID,
    category: &'static str,
}

impl AggregateName {
    pub(crate) fn of(aggregate: &AggregateType) -> Self {
        let (category, domain) = match aggregate {
            AggregateType::EffectHandler(_) => ("EffectHandler", "rayc_c2::EffectHandlerLayout:v1"),
            AggregateType::Tuple(_) => ("Tuple", "rayc_c2::TupleLayout:v1"),
            AggregateType::Environment(_) => ("Environment", "rayc_c2::EnvironmentLayout:v1"),
            AggregateType::Struct(_) => ("Struct", "rayc_c2::StructLayout:v1"),
        };
        Self { id: StableID::of(domain, aggregate), category }
    }

    /// The `typedef` alias used everywhere the aggregate is spelled as a type.
    pub(crate) const fn typedef(self) -> AggregateTypedefName { AggregateTypedefName(self) }
}

impl fmt::Display for AggregateName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Ray{}_{}", self.category, self.id)
    }
}

/// The `typedef` alias of an aggregate, e.g. `RayTuple_<id>_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AggregateTypedefName(AggregateName);

impl fmt::Display for AggregateTypedefName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}_t", self.0)
    }
}

/// The root function of a source definition instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DefinitionName {
    symbol: StableID,
    substitution: StableID,
}

impl DefinitionName {
    pub(crate) fn of(instance: &MonoDefInstance) -> Self {
        Self {
            symbol: StableID::of("rayc_c2::DefinitionSymbol:v1", &instance.def_id()),
            substitution: StableID::of(
                "rayc_c2::DefinitionSubstitution:v1",
                instance.substitution(),
            ),
        }
    }
}

impl fmt::Display for DefinitionName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ray_def_{}_{}", self.symbol, self.substitution)
    }
}

/// The root function of a compiler-generated nominal `Drop.drop` fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NominalDropName(StableID);

impl NominalDropName {
    pub(crate) fn of(instance: &MonoNominalDropInstance) -> Self {
        Self(StableID::of("rayc_c2::NominalDrop:v1", instance))
    }
}

impl fmt::Display for NominalDropName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ray_drop_{}", self.0)
    }
}

/// The body of a source closure, which other fragments can reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClosureName(StableID);

impl ClosureName {
    pub(crate) fn of(instance: &MonoClosureInstance) -> Self {
        Self(StableID::of("rayc_c2::NominalClosure:v1", instance))
    }
}

impl fmt::Display for ClosureName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ray_nominal_{}", self.0)
    }
}

/// The root function of an independently lowered fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FragmentName {
    Definition(DefinitionName),
    NominalDrop(NominalDropName),
}

impl FragmentName {
    pub(crate) fn of(instance: &MonoFragmentInstance) -> Self {
        match instance {
            MonoFragmentInstance::Definition(instance) => {
                Self::Definition(DefinitionName::of(instance))
            }
            MonoFragmentInstance::NominalDrop(instance) => {
                Self::NominalDrop(NominalDropName::of(instance))
            }
        }
    }
}

impl fmt::Display for FragmentName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Definition(name) => name.fmt(formatter),
            Self::NominalDrop(name) => name.fmt(formatter),
        }
    }
}

/// Any function owned by a `MonoIR` fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FunctionName {
    /// The fragment's root function.
    Root(FragmentName),
    /// A lambda, thunk, or operation handler nested in the fragment.
    Nested { fragment: FragmentName, category: &'static str, index: u64 },
    /// A source closure body, named independently of its owning fragment so
    /// other fragments can reference it.
    Closure(ClosureName),
}

impl FunctionName {
    /// The name of `function_id` in `ir`, whose root function is called
    /// `fragment`.
    pub(crate) fn of(ir: &MonoIR, fragment: FragmentName, function_id: MonoFunctionID) -> Self {
        if let Some(closure) = ir.closure_instance(function_id) {
            return Self::Closure(ClosureName::of(&closure));
        }
        if function_id == ir.root_id() {
            return Self::Root(fragment);
        }
        Self::Nested {
            fragment,
            category: nested_function_category(ir.get_function(function_id).kind()),
            index: function_id.index(),
        }
    }
}

impl fmt::Display for FunctionName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root(fragment) => fragment.fmt(formatter),
            Self::Nested { fragment, category, index } => {
                write!(formatter, "{fragment}_{category}_{index:X}")
            }
            Self::Closure(name) => name.fmt(formatter),
        }
    }
}

/// The name segment distinguishing a nested function's origin.
fn nested_function_category(kind: MonoFunctionKind) -> &'static str {
    match kind {
        MonoFunctionKind::Def => {
            panic!("compiler-internal invariant violation: nested function has def kind")
        }
        MonoFunctionKind::Lambda => "lambda",
        MonoFunctionKind::Thunk => "thunk",
        MonoFunctionKind::OperationHandler => "handler",
    }
}

/// A function-local variable, including parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LocalName(pub(crate) LocalID);

impl fmt::Display for LocalName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ray_local_{:X}", self.0.index())
    }
}

/// The label of a basic block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockName(pub(crate) BlockID);

impl fmt::Display for BlockName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ray_block_{:X}", self.0.index())
    }
}

/// A member of a generated aggregate struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldName {
    Tuple(u32),
    Environment(u32),
    Struct(FieldID),
    OperationEnvironment(GlobalSymbolID),
    OperationFunction(GlobalSymbolID),
}

impl FieldName {
    /// The member selected by a place projection, or `None` for a
    /// dereference.
    pub(crate) const fn of_projection(projection: Projection) -> Option<Self> {
        match projection {
            Projection::Dereference => None,
            Projection::EnvironmentFieldIndex(index) => Some(Self::Environment(index.index())),
            Projection::TupleFieldIndex(index) => Some(Self::Tuple(index.index())),
            Projection::StructFieldIndex(field) => Some(Self::Struct(field)),
            Projection::OperationRecordEnvironmentField(operation) => {
                Some(Self::OperationEnvironment(operation))
            }
            Projection::OperationRecordFunctionPointerField(operation) => {
                Some(Self::OperationFunction(operation))
            }
        }
    }

    /// The member at `index` of a tuple or environment layout.
    pub(crate) fn positional(index: usize, make: fn(u32) -> Self) -> Self {
        make(index.try_into().expect("aggregate field index should fit in u32"))
    }
}

impl fmt::Display for FieldName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tuple(index) => write!(formatter, "elem{index:X}"),
            Self::Environment(index) => write!(formatter, "capture{index:X}"),
            Self::Struct(field) => write!(formatter, "field_{}", field.index()),
            Self::OperationEnvironment(operation) => {
                write!(formatter, "operation_{}_environment", operation_id(*operation))
            }
            Self::OperationFunction(operation) => {
                write!(formatter, "operation_{}_function", operation_id(*operation))
            }
        }
    }
}

fn operation_id(operation: GlobalSymbolID) -> StableID {
    StableID::of("rayc_c2::EffectOperation:v1", &operation)
}

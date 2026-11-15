//! C identifiers for generated items.
//!
//! Every name is a small value that renders itself through [`fmt::Display`],
//! so names are written straight into the output buffer instead of being
//! materialized as intermediate `String`s.
//!
//! Names derived from semantic identities embed a stable hash rather than a
//! counter. A name then depends only on what it names, never on which function
//! happened to be generated first, which keeps the output the same however
//! functions are scheduled.
//!
//! The hash domains are spelled as in `rayc_c2`, so that both backends give
//! every item the same name and their output can be compared directly.

use std::fmt;

use qbice::{
    StableHash,
    stable_hash::{Sip128Hasher, StableHasher},
    storage::intern::Interned,
};
use rayc_mono_ir::{
    MonoBodyInstance, MonoCoroutineInstance, MonoFunctionInstance, MonoGeneratedAdtDropInstance,
};

/// A domain-separated 128-bit stable hash, rendered in base 62.
///
/// The domain keeps equal values of different kinds of items, such as a
/// coroutine's resume and drop functions, from sharing a name.
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
        // 62^22 > 2^128, so 22 digits hold any u128
        const MAX_ENCODED_LENGTH: usize = 22;

        // fill the digits from the least significant end of a stack buffer
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

/// A function of the IR of a source definition instance, named by the
/// definition, its substitution and the index of the function in the IR.
///
/// One definition can lower to several IR functions, such as the closures
/// it contains, which the index tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BodyName {
    symbol: StableID,
    substitution: StableID,
    function: u64,
}

impl BodyName {
    pub(crate) fn of(instance: &MonoBodyInstance) -> Self {
        Self {
            symbol: StableID::of("rayc_c2::DefinitionSymbol:v1", &instance.def_id()),
            substitution: StableID::of(
                "rayc_c2::DefinitionSubstitution:v1",
                instance.substitution(),
            ),
            function: instance.ir_function_id().index(),
        }
    }
}

impl fmt::Display for BodyName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ray_def_{}_{}_fn{:X}", self.symbol, self.substitution, self.function)
    }
}

/// A compiler-generated nominal `Drop.drop` function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NominalDropName(StableID);

impl NominalDropName {
    pub(crate) fn of(instance: &MonoGeneratedAdtDropInstance) -> Self {
        Self(StableID::of("rayc_c2::NominalDrop:v1", instance))
    }
}

impl fmt::Display for NominalDropName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ray_drop_{}", self.0)
    }
}

/// The `coroutine_resume` function of a coroutine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CoroutineResumeName(StableID);

impl CoroutineResumeName {
    pub(crate) fn of(instance: &MonoCoroutineInstance) -> Self {
        Self(StableID::of("rayc_c2::CoroutineResume:v1", instance))
    }
}

impl fmt::Display for CoroutineResumeName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ray_coroutine_resume_{}", self.0)
    }
}

/// The `coroutine_drop` function of a coroutine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CoroutineDropName(StableID);

impl CoroutineDropName {
    pub(crate) fn of(instance: &MonoCoroutineInstance) -> Self {
        Self(StableID::of("rayc_c2::CoroutineDrop:v1", instance))
    }
}

impl fmt::Display for CoroutineDropName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ray_coroutine_drop_{}", self.0)
    }
}

/// Any independently lowered function, which this translation unit defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FunctionName {
    Body(BodyName),
    NominalDrop(NominalDropName),
    CoroutineResume(CoroutineResumeName),
    CoroutineDrop(CoroutineDropName),
}

impl FunctionName {
    /// The name of the function `instance`.
    ///
    /// # Panics
    ///
    /// Panics for an `extern def`, which keeps its source name (see
    /// [`LinkName::Extern`]).
    pub(crate) fn of(instance: &MonoFunctionInstance) -> Self {
        match instance {
            MonoFunctionInstance::Body(instance) => Self::Body(BodyName::of(instance)),
            MonoFunctionInstance::ExternDef(_) => {
                panic!("an `extern def` keeps its source name and is never defined")
            }
            MonoFunctionInstance::NominalDrop(instance) => {
                Self::NominalDrop(NominalDropName::of(instance))
            }
            MonoFunctionInstance::CoroutineResume(instance) => {
                Self::CoroutineResume(CoroutineResumeName::of(instance))
            }
            MonoFunctionInstance::CoroutineDrop(instance) => {
                Self::CoroutineDrop(CoroutineDropName::of(instance))
            }
        }
    }
}

impl fmt::Display for FunctionName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Body(name) => name.fmt(formatter),
            Self::NominalDrop(name) => name.fmt(formatter),
            Self::CoroutineResume(name) => name.fmt(formatter),
            Self::CoroutineDrop(name) => name.fmt(formatter),
        }
    }
}

/// The name a function is linked under.
///
/// Generated functions get a [`FunctionName`]; an `extern def` keeps its
/// source name, since the C environment defines it under that name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkName {
    Generated(FunctionName),
    Extern(Interned<str>),
}

impl fmt::Display for LinkName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Generated(name) => name.fmt(formatter),
            Self::Extern(name) => formatter.write_str(name),
        }
    }
}

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
use rayc_coroutine_frame_layout::savable::Savable;
use rayc_ir::{address::Local, ir_closure::CaptureID};
use rayc_mono_ir::{
    MonoBodyInstance, MonoCoroutineInstance, MonoFunctionInstance, MonoGeneratedAdtDropInstance,
    cfg::BlockID,
    function::LocalID,
    layout::{
        HandlerRecordID,
        coroutine::{ResumeVariantID, RowOperationID},
    },
    place::{CoroutineFrame, CoroutineProgress, CoroutineYield, Projection, RunWithFrames},
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
            AggregateType::EffectHandlerCallback(_) => {
                ("EffectHandler", "rayc_c2::EffectHandlerLayout:v1")
            }
            AggregateType::Tuple(_) => ("Tuple", "rayc_c2::TupleLayout:v1"),
            AggregateType::Environment(_) => ("Environment", "rayc_c2::EnvironmentLayout:v1"),
            AggregateType::Struct(_) => ("Struct", "rayc_c2::StructLayout:v1"),
            AggregateType::CoroutineFrame(_) => {
                ("CoroutineFrame", "rayc_c2::CoroutineFrameLayout:v1")
            }
            AggregateType::CoroutineResume(_) => {
                ("CoroutineResume", "rayc_c2::CoroutineResumeLayout:v1")
            }
            AggregateType::CoroutineYield(_) => {
                ("CoroutineYield", "rayc_c2::CoroutineYieldLayout:v1")
            }
            AggregateType::CoroutineProgress(_) => {
                ("CoroutineProgress", "rayc_c2::CoroutineProgressLayout:v1")
            }
            AggregateType::RunWithFrames(_) => ("RunWithFrames", "rayc_c2::RunWithFramesLayout:v1"),
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

/// A function of the IR of a source definition instance, named by the
/// definition, its substitution and the index of the function in the IR.
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
    /// Panics for an `extern def`, which keeps its source name.
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
    EnvironmentCapture(CaptureID),
    EnvironmentHandlerRecord(HandlerRecordID),
    Struct(FieldID),
    OperationEnvironment(GlobalSymbolID),
    OperationFunction(GlobalSymbolID),
    CoroutineFrameState,
    /// The member of a frame that holds a value. The structs and the union
    /// around it are anonymous, so accessing it does not depend on where the
    /// layout places the value, and every member of a frame has a unique name.
    CoroutineFrameSavable(Savable),
    CoroutineResumeValue(ResumeVariantID),
    /// The tag of a `coroutine_progress_t` or a `coroutine_yield_t`.
    CoroutineTag,
    CoroutineProgressYield,
    CoroutineProgressReturn,
    CoroutineYieldOperation(RowOperationID),
    RunWithHandlerEnvironment,
    RunWithBody,
    RunWithPhase,
    /// The frame of the clause of an operation, in an anonymous union.
    RunWithClause(GlobalSymbolID),
}

impl FieldName {
    /// The member selected by a place projection, or `None` for a
    /// dereference.
    pub(crate) const fn of_projection(projection: Projection) -> Option<Self> {
        match projection {
            Projection::Dereference => None,
            Projection::EnvironmentCapture(capture) => Some(Self::EnvironmentCapture(capture)),
            Projection::EnvironmentHandlerRecord(record) => {
                Some(Self::EnvironmentHandlerRecord(record))
            }
            Projection::TupleFieldIndex(index) => Some(Self::Tuple(index.index())),
            Projection::StructFieldIndex(field) => Some(Self::Struct(field)),
            Projection::OperationRecordEnvironmentField(operation) => {
                Some(Self::OperationEnvironment(operation))
            }
            Projection::OperationRecordFunctionPointerField(operation) => {
                Some(Self::OperationFunction(operation))
            }
            Projection::CoroutineFrame(CoroutineFrame::Tag) => Some(Self::CoroutineFrameState),
            Projection::CoroutineFrame(CoroutineFrame::Savable(savable)) => {
                Some(Self::CoroutineFrameSavable(savable))
            }
            Projection::CoroutineResumeValue(variant) => Some(Self::CoroutineResumeValue(variant)),
            Projection::CoroutineProgress(CoroutineProgress::Tag)
            | Projection::CoroutineYield(CoroutineYield::Tag) => Some(Self::CoroutineTag),
            Projection::CoroutineProgress(CoroutineProgress::YieldPayload) => {
                Some(Self::CoroutineProgressYield)
            }
            Projection::CoroutineProgress(CoroutineProgress::ReturnPayload) => {
                Some(Self::CoroutineProgressReturn)
            }
            Projection::CoroutineYield(CoroutineYield::OperationPayload(operation)) => {
                Some(Self::CoroutineYieldOperation(operation))
            }
            Projection::RunWithFrames(RunWithFrames::HandlerEnvironment) => {
                Some(Self::RunWithHandlerEnvironment)
            }
            Projection::RunWithFrames(RunWithFrames::Body) => Some(Self::RunWithBody),
            Projection::RunWithFrames(RunWithFrames::Phase) => Some(Self::RunWithPhase),
            Projection::RunWithFrames(RunWithFrames::Clause(operation)) => {
                Some(Self::RunWithClause(operation))
            }
        }
    }

    /// The member at `index` of a tuple layout.
    pub(crate) fn positional(index: usize, make: fn(u32) -> Self) -> Self {
        make(index.try_into().expect("aggregate field index should fit in u32"))
    }
}

impl fmt::Display for FieldName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tuple(index) => write!(formatter, "elem{index:X}"),
            Self::EnvironmentCapture(capture) => write!(formatter, "capture{:X}", capture.index()),
            Self::EnvironmentHandlerRecord(record) => {
                write!(formatter, "handler{:X}", record.index())
            }
            Self::Struct(field) => write!(formatter, "field_{}", field.index()),
            Self::OperationEnvironment(operation) => {
                write!(formatter, "operation_{}_environment", operation_id(*operation))
            }
            Self::OperationFunction(operation) => {
                write!(formatter, "operation_{}_function", operation_id(*operation))
            }
            Self::CoroutineFrameState => formatter.write_str("state"),
            Self::CoroutineFrameSavable(savable) => write_savable_name(formatter, *savable),
            Self::CoroutineResumeValue(variant) => {
                write!(formatter, "value{:X}", variant.index())
            }
            Self::CoroutineTag => formatter.write_str("tag"),
            Self::CoroutineProgressYield => formatter.write_str("yield_value"),
            Self::CoroutineProgressReturn => formatter.write_str("return_value"),
            Self::CoroutineYieldOperation(operation) => {
                write!(formatter, "operation{:X}", operation.index())
            }
            Self::RunWithHandlerEnvironment => formatter.write_str("handler_environment"),
            Self::RunWithBody => formatter.write_str("body"),
            Self::RunWithPhase => formatter.write_str("phase"),
            Self::RunWithClause(operation) => {
                write!(formatter, "clause_{}", operation_id(*operation))
            }
        }
    }
}

/// Writes the name of the member of a frame that holds `savable`.
fn write_savable_name(formatter: &mut fmt::Formatter<'_>, savable: Savable) -> fmt::Result {
    match savable {
        Savable::Expression(expression) => write!(formatter, "expr{:X}", expression.index()),
        Savable::ChildFrame(point) => {
            write!(formatter, "child_frame{:X}", point.block_id().index())
        }
        Savable::Local(Local::Variable(variable)) => write!(formatter, "var{:X}", variable.index()),
        Savable::Local(Local::Parameter(parameter)) => {
            write!(formatter, "param{:X}", parameter.index())
        }
        Savable::Local(Local::ClosureParameter(parameter)) => {
            write!(formatter, "closure_param{:X}", parameter.index())
        }
        Savable::Local(Local::OperationHandlerParameter(parameter)) => {
            write!(formatter, "handler_param{:X}", parameter.index())
        }
        Savable::Local(Local::Capture(capture)) => {
            write!(formatter, "capture{:X}", capture.index())
        }
        Savable::Environment => formatter.write_str("environment"),
    }
}

fn operation_id(operation: GlobalSymbolID) -> StableID {
    StableID::of("rayc_c2::EffectOperation:v1", &operation)
}

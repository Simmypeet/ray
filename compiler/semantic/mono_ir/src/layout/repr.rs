//! How a value of each type is represented at runtime.
//!
//! The representation is a property of the type alone, so every backend
//! agrees on it: a zero-sized value is never stored, and a transparent one is
//! stored as the only value it holds, whichever backend spells it.
//!
//! A type may also have a niche: a scalar inside it whose type has values that
//! no value of the type ever stores, such as a null reference. An enclosing
//! enum can store its discriminant in those values instead of in a tag of its
//! own, so `Maybe[&T]` is a pointer that is null for `Nothing`.

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_extend::extend;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};

use crate::{
    instance::MonoEnumInstance,
    layout::{
        TagType, VariantIndex, build_enum_layout, build_environment_layout, build_handler_layout,
        build_struct_layout, coroutine::build_effect_row_layout,
    },
    place::{FieldIndex, Projection},
    ty::{AggregateType, MonoType, Nullability},
};

/// The kind of scalar a niche is in, which tells how its values are read and
/// written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, StableHash, Encode, Decode)]
pub enum NicheScalar {
    /// A pointer that is never null, at the niche's path: null is its only
    /// free value, numbered 0.
    NonNullPointer,

    /// A `bool` at the niche's path, stored as a byte that holds only 0 or 1.
    Bool,

    /// The tag of the enum at the niche's path, which holds only the indices
    /// of the enum's variants.
    Tag(TagType),
}

impl NicheScalar {
    /// The unsigned integer type the scalar's values are read and written
    /// as, or `None` for a pointer, whose only free value is null.
    #[must_use]
    pub const fn integer_type(self) -> Option<TagType> {
        match self {
            Self::NonNullPointer => None,
            Self::Bool => Some(TagType::Uint8),
            Self::Tag(tag_type) => Some(tag_type),
        }
    }
}

/// Values that a scalar inside a type never holds in a value of the type,
/// which an enclosing enum can give meanings of its own.
///
/// The free values are `start..start + count`, in the numbering of the scalar:
/// a pointer's only free value is null, numbered 0. Only this module makes a
/// niche, and never one without a free value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode)]
pub struct Niche {
    /// Where the scalar is, as projections from a value of the type, which
    /// pass through zero-sized and transparent members like any other place.
    path: Interned<[Projection]>,
    scalar: NicheScalar,

    start: u64,
    count: u64,
}

impl Niche {
    /// The projections from a value of the type to the scalar, or to the enum
    /// whose tag the scalar is.
    #[must_use]
    pub fn path(&self) -> &[Projection] { &self.path }

    #[must_use]
    pub const fn scalar(&self) -> NicheScalar { self.scalar }

    /// The first free value.
    #[must_use]
    pub const fn start(&self) -> u64 { self.start }

    /// How many values from [`Self::start`] on are free; never 0.
    #[must_use]
    pub const fn count(&self) -> u64 { self.count }

    /// The niche of a scalar of type `ty`, if the type has values that no
    /// value of it ever holds.
    fn of_scalar(ty: &MonoType, engine: &TrackedEngine) -> Option<Self> {
        // a pointer's only free value is null; a `bool` is a byte
        let (scalar, start, count) = match ty {
            MonoType::Pointer(pointer) => match pointer.nullability() {
                Nullability::NonNull => (NicheScalar::NonNullPointer, 0, 1),
                Nullability::Nullable => return None,
            },
            // a function pointer only ever comes from naming a function
            MonoType::FunctionPointer(_) => (NicheScalar::NonNullPointer, 0, 1),
            MonoType::Bool => (NicheScalar::Bool, 2, u64::from(u8::MAX) - 1),

            MonoType::Int8
            | MonoType::Int16
            | MonoType::Int32
            | MonoType::Int64
            | MonoType::Isize
            | MonoType::Uint8
            | MonoType::Uint16
            | MonoType::Uint32
            | MonoType::Uint64
            | MonoType::Usize
            | MonoType::Float32
            | MonoType::CInt
            | MonoType::CStr
            | MonoType::OpaquePointer(_)
            | MonoType::Aggregate(_) => return None,
        };
        Some(Self { path: engine.intern_unsized([]), scalar, start, count })
    }

    /// The niche of the tag of an enum of `variant_count` variants, at the
    /// enum itself, if the tag type has values above the last index.
    fn of_tag(variant_count: usize, engine: &TrackedEngine) -> Option<Self> {
        let tag_type = TagType::for_variant_count(variant_count);
        let start = u64::try_from(variant_count).expect("a variant count should fit in u64");
        // with exactly as many variants as the tag type has values, e.g. 256
        // for `u8`, every value is an index and none is free
        let count = tag_type.max_value().checked_sub(start)?.checked_add(1)?;
        Some(Self {
            path: engine.intern_unsized([]),
            scalar: NicheScalar::Tag(tag_type),
            start,
            count,
        })
    }

    /// The same niche, reached from a value that holds the current one at
    /// `projection`.
    fn within(&self, projection: Projection, engine: &TrackedEngine) -> Self {
        let path = std::iter::once(projection).chain(self.path.iter().copied()).collect::<Vec<_>>();
        Self { path: engine.intern_unsized(path), ..self.clone() }
    }

    /// What is left of the niche once its first `used` free values are taken,
    /// or `None` if none is left.
    fn after(&self, used: u64) -> Option<Self> {
        let count = self.count.checked_sub(used).filter(|count| *count != 0)?;
        Some(Self { start: self.start + used, count, ..self.clone() })
    }
}

/// How a value of a type is represented at runtime, and the niche it offers
/// an enclosing enum.
///
/// A value that holds nothing has no scalar to find a niche in, so only a
/// transparent or direct representation has one. Only this module derives a
/// representation, so outside this crate one can be matched but not made.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode)]
pub enum TypeRepr {
    /// The type has exactly one value, so a value of it holds nothing: it is
    /// never stored, passed or returned.
    ///
    /// A place of the type still has an address, which only tells it apart
    /// from no place at all.
    ZeroSized,

    /// A value of the type is stored, passed and returned as a value of
    /// `inner`, and selecting what it holds selects the whole value. That is
    /// the case for:
    ///
    /// - an aggregate that stores exactly one member, which is the value;
    /// - an enum whose only payload that holds something has a niche for the
    ///   other variants, which is the value;
    /// - an enum whose payloads all hold nothing, which is its tag.
    ///
    /// `inner` is never zero-sized or transparent itself: a wrapper of a
    /// wrapper is the innermost value.
    Transparent { inner: Interned<MonoType>, niche: Option<Niche> },

    /// The type is represented by itself, as a scalar or as an aggregate of
    /// its members whose types are not zero-sized.
    Direct { niche: Option<Niche> },
}

impl TypeRepr {
    /// Whether a value of the type holds nothing.
    #[must_use]
    pub const fn is_zero_sized(&self) -> bool {
        match self {
            Self::ZeroSized => true,
            Self::Transparent { .. } | Self::Direct { .. } => false,
        }
    }

    /// The type a value of a transparent type is stored as, or `None` if the
    /// type is not transparent.
    #[must_use]
    pub const fn transparent_inner(&self) -> Option<&Interned<MonoType>> {
        match self {
            Self::Transparent { inner, .. } => Some(inner),
            Self::ZeroSized | Self::Direct { .. } => None,
        }
    }

    /// Whether a value of the type is stored as itself, so an aggregate of
    /// the type is defined with its members.
    #[must_use]
    pub const fn is_direct(&self) -> bool {
        match self {
            Self::Direct { .. } => true,
            Self::ZeroSized | Self::Transparent { .. } => false,
        }
    }

    /// The niche with the most free values, if the type has one.
    #[must_use]
    pub const fn niche(&self) -> Option<&Niche> {
        match self {
            Self::ZeroSized => None,
            Self::Transparent { niche, .. } | Self::Direct { niche } => niche.as_ref(),
        }
    }
}

/// Where a tagged enum stores its tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagStorage {
    /// The payloads all hold nothing, so the enum is its tag alone.
    Alone,

    /// The tag is the enum's `tag` member, beside the payloads.
    BesidePayloads,
}

/// How an enum is represented: where it stores which variant a value holds,
/// which decides how it is represented as a type and which niche it offers.
/// Like a [`TypeRepr`], it can be matched but not made outside this crate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode)]
pub enum EnumRepr {
    /// The enum has at most one variant, so the discriminant is always 0 and
    /// is never stored. The variant's payload, if any, is the enum's
    /// representation.
    Inherit(TypeRepr),

    /// Every payload holds nothing, so the enum is transparent to its `tag`
    /// type, and offers the tag's free values.
    NumericTag { tag: Interned<MonoType>, niche: Option<Niche> },

    /// The enum is direct, with its tag as its `tag` member beside its
    /// payloads, and offers the tag's free values.
    NormalTagAndPayload { niche: Option<Niche> },

    /// Only the variant `data` has a payload that holds something, and the
    /// other variants are values of its niche, which is reached from the enum.
    /// The enum is transparent to `inner`, the stored type of the payload,
    /// and offers in `spare` what is left of the niche.
    ///
    /// The other variants take the first free values of the niche, in index
    /// order: the variant at index `v` is `niche.start() + v` below `data`,
    /// and `niche.start() + v - 1` above it. Any other value of the niche's
    /// scalar is a value of `data`.
    Niche { data: VariantIndex, niche: Niche, inner: Interned<MonoType>, spare: Option<Niche> },
}

impl EnumRepr {
    /// Where the enum stores its tag, or `None` if it stores none.
    #[must_use]
    pub const fn tag_storage(&self) -> Option<TagStorage> {
        match self {
            Self::NumericTag { .. } => Some(TagStorage::Alone),
            Self::NormalTagAndPayload { .. } => Some(TagStorage::BesidePayloads),
            Self::Inherit(_) | Self::Niche { .. } => None,
        }
    }

    /// The representation of the enum as a type, as any other type sees it.
    #[must_use]
    pub fn type_repr(&self) -> TypeRepr {
        match self {
            Self::Inherit(repr) => repr.clone(),
            Self::NumericTag { tag, niche } => {
                TypeRepr::Transparent { inner: tag.clone(), niche: niche.clone() }
            }
            Self::NormalTagAndPayload { niche } => TypeRepr::Direct { niche: niche.clone() },
            Self::Niche { inner, spare, .. } => {
                TypeRepr::Transparent { inner: inner.clone(), niche: spare.clone() }
            }
        }
    }
}

/// Derives the representation of an aggregate type.
///
/// A tuple, struct, environment or `coroutine_resume_t` is represented by
/// the members it stores, those whose types are not zero-sized: it is
/// zero-sized without any, transparent with exactly one, and direct with
/// more. Its niche is the largest of its members', unless its members
/// overlap. An effect handler record is zero-sized without operations and
/// direct otherwise. A coroutine aggregate with a tag is always direct.
///
/// An enum is represented by [`EnumReprKey`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(TypeRepr)]
#[extend(name = get_aggregate_repr, by_val)]
pub struct AggregateReprKey {
    /// The aggregate whose members determine its representation.
    pub aggregate: AggregateType,
}

#[executor(config = Config)]
async fn aggregate_repr_executor(key: &AggregateReprKey, engine: &TrackedEngine) -> TypeRepr {
    match &key.aggregate {
        AggregateType::Tuple(tuple) => {
            let fields = tuple.fields().iter().enumerate().map(|(index, field)| {
                (Projection::TupleFieldIndex(FieldIndex::from_usize(index)), field.clone())
            });
            repr_of_members(engine, fields, Overlap::Disjoint).await
        }
        AggregateType::Struct(instance) => {
            let layout = engine.build_struct_layout(instance.clone()).await;
            let fields = layout
                .fields()
                .iter()
                .map(|(field, ty)| (Projection::StructFieldIndex(*field), ty.clone()));
            repr_of_members(engine, fields, Overlap::Disjoint).await
        }
        AggregateType::Enum(instance) => engine.get_enum_repr(instance.clone()).await.type_repr(),

        // a handler record is a pointer, which is stored like a capture
        AggregateType::Environment(instance) => {
            let layout = engine.build_environment_layout(instance.clone()).await;
            let captures = layout
                .captures()
                .map(|(capture, ty)| (Projection::EnvironmentCapture(capture), ty.clone()));
            let records = layout.handler_records().map(|(id, record)| {
                let ty = MonoType::new_handler_pointer(record.effect().clone(), engine);
                (Projection::EnvironmentHandlerRecord(id), ty)
            });
            repr_of_members(engine, captures.chain(records), Overlap::Disjoint).await
        }

        // each operation is a callback and an opaque environment pointer, so
        // a record with an operation stores at least two members
        AggregateType::EffectHandlerCallback(handler) => {
            let layout = engine.build_handler_layout(handler.mono_effect_instance().clone()).await;
            if layout.operations().is_empty() {
                TypeRepr::ZeroSized
            } else {
                TypeRepr::Direct { niche: None }
            }
        }

        // the members of a `coroutine_resume_t` overlap, all at its start, so
        // with only one stored it is that member
        AggregateType::CoroutineResume(resume) => {
            let layout = engine.build_effect_row_layout(resume.row().clone()).await;
            let variants = layout
                .resume_variants()
                .map(|(id, variant)| (Projection::CoroutineResumeValue(id), variant.ty().clone()));
            repr_of_members(engine, variants, Overlap::Overlapping).await
        }

        // each of these stores a tag
        AggregateType::CoroutineFrame(_)
        | AggregateType::CoroutineYield(_)
        | AggregateType::CoroutineProgress(_)
        | AggregateType::RunWithFrames(_) => TypeRepr::Direct { niche: None },
    }
}

#[distributed_slice(RAY_PROGRAM)]
static AGGREGATE_REPR_EXECUTOR: Registration<Config> =
    Registration::new::<AggregateReprKey, AggregateReprExecutor>();

/// Derives the representation of an enum.
///
/// An enum of at most one variant is that variant's payload. Otherwise, when
/// exactly one variant has a payload that holds something, and its niche has
/// a free value for each other variant, the enum is that payload and the other
/// variants are values of the niche. Any other enum stores a tag, beside its
/// payloads or alone when they all hold nothing, and the tag's unused values
/// are its niche.
#[derive(Debug, Clone, PartialEq, Eq, Hash, StableHash, Encode, Decode, Query)]
#[value(EnumRepr)]
#[extend(name = get_enum_repr, by_val)]
pub struct EnumReprKey {
    /// The enum whose variants determine its representation.
    pub instance: MonoEnumInstance,
}

#[executor(config = Config)]
async fn enum_repr_executor(key: &EnumReprKey, engine: &TrackedEngine) -> EnumRepr {
    let layout = engine.build_enum_layout(key.instance.clone()).await;

    // at most one variant: its payload, if any, is the enum
    if !layout.has_tag() {
        let payloads = layout.variants().filter_map(|(index, variant)| {
            Some((Projection::VariantPayload(index), variant.payload()?.clone()))
        });
        let members = repr_of_members(engine, payloads, Overlap::Overlapping).await;
        return EnumRepr::Inherit(members);
    }

    // the variants whose payloads hold something
    let mut data = Vec::new();
    for (index, variant) in layout.variants() {
        let Some(payload) = variant.payload() else { continue };

        let payload_repr = engine.get_type_repr(payload).await;
        if !payload_repr.is_zero_sized() {
            data.push((index, payload.clone(), payload_repr));
        }
    }

    // every other variant takes a free value of the only payload's niche
    let others = u64::try_from(layout.variants().len() - 1).expect("a count should fit in u64");

    if let [(index, payload, payload_repr)] = data.as_slice()
        && let Some(niche) = payload_repr.niche()
        && niche.count >= others
    {
        let niche = niche.within(Projection::VariantPayload(*index), engine);
        let inner = payload_repr.transparent_inner().unwrap_or(payload).clone();
        let spare = niche.after(others);

        return EnumRepr::Niche { data: *index, niche, inner, spare };
    }

    // otherwise a tag is stored, alone when the payloads all hold nothing
    let niche = Niche::of_tag(layout.variants().len(), engine);

    if data.is_empty() {
        let tag = engine.intern(layout.discriminant_type().mono_type());
        EnumRepr::NumericTag { tag, niche }
    } else {
        EnumRepr::NormalTagAndPayload { niche }
    }
}

#[distributed_slice(RAY_PROGRAM)]
static ENUM_REPR_EXECUTOR: Registration<Config> =
    Registration::new::<EnumReprKey, EnumReprExecutor>();

/// The representation of `ty`. Only an aggregate can be zero-sized or
/// transparent, and only a non-null pointer, a function pointer, a `bool` or an
/// aggregate holding one of them has a niche.
///
/// A pointer is direct whatever it points to, so the representation of a type
/// never asks for that of a type it only points to, and a type that points to
/// itself is no cycle.
#[extend]
pub async fn get_type_repr(self: &TrackedEngine, ty: &MonoType) -> TypeRepr {
    match ty.as_aggregate() {
        Some(aggregate) => self.get_aggregate_repr(aggregate.clone()).await,
        None => TypeRepr::Direct { niche: Niche::of_scalar(ty, self) },
    }
}

/// Whether the members of an aggregate are stored side by side or all at its
/// start, overlapping, as the members of a union are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overlap {
    Disjoint,
    Overlapping,
}

/// The representation of an aggregate that stores `members`, each selected by
/// its projection: zero-sized when none holds anything, which includes having
/// no member at all, transparent when exactly one does, and direct otherwise.
///
/// The niche is the one of the stored members with the most free values, the
/// first on a tie. Members that overlap may occupy the bytes of each other's
/// niches, so with more than one stored they have none.
async fn repr_of_members(
    engine: &TrackedEngine,
    members: impl IntoIterator<Item = (Projection, Interned<MonoType>)>,
    overlap: Overlap,
) -> TypeRepr {
    let mut stored = 0_usize;
    let mut inner = None;
    let mut niche = None::<Niche>;

    for (projection, member) in members {
        let repr = engine.get_type_repr(&member).await;

        if repr.is_zero_sized() {
            continue;
        }
        stored += 1;

        // a transparent member is stored as its own only value, so the
        // aggregate would be too
        inner = Some(repr.transparent_inner().cloned().unwrap_or(member));

        if let Some(candidate) = repr.niche()
            && niche.as_ref().is_none_or(|niche| candidate.count > niche.count)
        {
            niche = Some(candidate.within(projection, engine));
        }
    }

    // overlapping members keep a niche only when one of them is stored
    let niche = match overlap {
        Overlap::Disjoint => niche,
        Overlap::Overlapping => niche.filter(|_| stored == 1),
    };

    match (stored, inner) {
        (0, _) => TypeRepr::ZeroSized,
        (1, Some(inner)) => TypeRepr::Transparent { inner, niche },
        _ => TypeRepr::Direct { niche },
    }
}

//! Printing of the discriminant of an enum, which the enum's representation
//! stores in one of three ways (see [`EnumRepr`]):
//!
//! - nowhere, for an enum of at most one variant, whose discriminant is always
//!   0;
//! - in a tag beside the payloads, which is written and read as it is;
//! - in the niche of the only payload that holds something, whose free values
//!   stand for the other variants. Setting the payload's own variant writes
//!   nothing, since the payload already holds a value of it, and reading the
//!   discriminant tells whether the niche's scalar holds one of its free
//!   values.
//!
//! ```c
//! /* Maybe[&int32]: Nothing is null */
//! value = ((const int32_t *)0);                               /* Nothing  */
//! discriminant = ((value == ((const int32_t *)0)) ? 1 : 0);   /* Some = 0 */
//! ```

use qbice::storage::intern::Interned;
use rayc_mono_ir::{
    function::LocalID,
    instruction::SetDiscriminant,
    layout::{
        TagType, VariantIndex, build_enum_layout,
        repr::{EnumRepr, Niche, NicheScalar, TagStorage, get_enum_repr},
    },
    operand::Constant,
    place::Place,
    ty::MonoType,
};

use crate::{
    c::{
        expr::{PlaceExpr, ProjectionExpr},
        name::FieldName,
        ty::TypeName,
    },
    function_print::FunctionPrinter,
};

/// Where the discriminant of the enum at a place is stored, as spellings of
/// the projections from the place's local.
enum Storage {
    /// Nowhere: the discriminant is always 0.
    Implicit,
    /// In the tag at these projections.
    Tag(Vec<ProjectionExpr>),
    /// In the niche of the payload of `data`, whose scalar is at
    /// `scalar_projections` and has the type `scalar_type`.
    Niche {
        data: VariantIndex,
        niche: Niche,
        scalar_projections: Vec<ProjectionExpr>,
        scalar_type: Interned<MonoType>,
    },
}

/// The discriminant of the enum at a place.
struct EnumDiscriminant {
    /// The type the discriminant is read as.
    ty: TagType,
    variant_count: u64,
    storage: Storage,
}

impl FunctionPrinter<'_, '_> {
    /// Writes the statement that makes the enum at the place hold the
    /// variant, if the variant must be stored at all.
    pub(super) async fn write_set_discriminant(&mut self, set: &SetDiscriminant) {
        let place = set.place();
        let discriminant = self.enum_discriminant(place).await;
        let variant = set.variant();

        match discriminant.storage {
            Storage::Implicit => {}
            Storage::Tag(tag) => {
                let value = discriminant.ty.constant(u64::from(variant.index()));
                self.write_store(place.local(), &tag, &value).await;
            }

            // the payload of `data` was written already, and holds a value
            // of `data` by itself
            Storage::Niche { data, niche, scalar_projections, scalar_type } => {
                if variant == data {
                    return;
                }
                let value = niche_value(&niche, data, variant, &scalar_type);
                self.write_store(place.local(), &scalar_projections, &value).await;
            }
        }
    }

    /// Writes the index of the variant that the enum at `place` holds, as a
    /// value of its discriminant type.
    pub(super) async fn write_get_discriminant(&mut self, place: &Place) {
        let discriminant = self.enum_discriminant(place).await;
        match discriminant.storage {
            Storage::Implicit => self.write_operand_constant(&discriminant.ty.constant(0)).await,
            Storage::Tag(tag) => self.writer.write(&PlaceExpr::new(place.local(), &tag)).await,
            Storage::Niche { data, niche, scalar_projections, scalar_type } => {
                let scalar = PlaceExpr::new(place.local(), &scalar_projections);
                let reading = NicheReading {
                    scalar,
                    scalar_type,
                    niche: &niche,
                    data,
                    ty: discriminant.ty,
                    others: discriminant.variant_count - 1,
                };
                self.write_niche_discriminant(&reading).await;
            }
        }
    }

    /// Writes the variant a niche's scalar tells, the data variant unless it
    /// holds one of the niche's values that stand for the other variants.
    ///
    /// With one other variant, the scalar is compared with its value:
    ///
    /// ```c
    /// ((scalar == start) ? other : data)
    /// ```
    ///
    /// With more, which only an integer niche has, the scalar's distance
    /// from the start tells which: the other variants take the niche's values
    /// in index order, skipping `data`.
    ///
    /// ```c
    /// ((rel < others) ? ((rel < data) ? rel : rel + 1) : data)
    /// /* where rel is ((scalar_t)(scalar - start)) */
    /// ```
    async fn write_niche_discriminant(&mut self, reading: &NicheReading<'_>) {
        let data = u64::from(reading.data.index());
        let start = reading.niche.start();
        let integer = reading.niche.scalar().integer_type();

        if reading.others == 1 {
            let other = u64::from(data == 0);
            self.writer.push("((");
            self.writer.write(&reading.scalar).await;
            self.writer.push(" == ");
            let start = niche_constant(integer, start, &reading.scalar_type);
            self.write_operand_constant(&start).await;
            self.writer.push(") ? ");
            self.write_operand_constant(&reading.ty.constant(other)).await;
            self.writer.push(" : ");
            self.write_operand_constant(&reading.ty.constant(data)).await;
            self.writer.push(")");
            return;
        }

        let integer = integer.expect("only an integer niche has more than one free value");
        let integer_type = self.writer.engine().intern(integer.mono_type());
        let ty = self.writer.engine().intern(reading.ty.mono_type());

        self.writer.push("((");
        self.write_niche_distance(reading, integer, &integer_type).await;
        self.writer.push(" < ");
        self.write_operand_constant(&integer.constant(reading.others)).await;
        self.writer.push(") ? ((");
        self.write_niche_distance(reading, integer, &integer_type).await;
        self.writer.push(" < ");
        self.write_operand_constant(&integer.constant(data)).await;
        self.writer.push(") ? ((");
        self.writer.write(&TypeName::new(&ty)).await;
        self.writer.push(")");
        self.write_niche_distance(reading, integer, &integer_type).await;
        self.writer.push(") : ((");
        self.writer.write(&TypeName::new(&ty)).await;
        self.writer.push(")(");
        self.write_niche_distance(reading, integer, &integer_type).await;
        self.writer.push(" + 1))) : ");
        self.write_operand_constant(&reading.ty.constant(data)).await;
        self.writer.push(")");
    }

    /// Writes `((scalar_t)(scalar - start))`: how far past the niche's start
    /// the scalar is, wrapping below it to a value no smaller than the
    /// niche's size.
    async fn write_niche_distance(
        &mut self,
        reading: &NicheReading<'_>,
        integer: TagType,
        integer_type: &MonoType,
    ) {
        self.writer.push("((");
        self.writer.write(&TypeName::new(integer_type)).await;
        self.writer.push(")(");
        self.writer.write(&reading.scalar).await;
        self.writer.push(" - ");
        self.write_operand_constant(&integer.constant(reading.niche.start())).await;
        self.writer.push("))");
    }

    /// Writes `lvalue = value;`, where the lvalue is the local's projections.
    async fn write_store(
        &mut self,
        local: LocalID,
        projections: &[ProjectionExpr],
        value: &Constant,
    ) {
        self.writer.push("    ");
        self.writer.write(&PlaceExpr::new(local, projections)).await;
        self.writer.push(" = ");
        self.write_operand_constant(value).await;
        self.writer.push(";\n");
    }

    /// Where the discriminant of the enum at `place` is stored, by the
    /// enum's representation.
    async fn enum_discriminant(&self, place: &Place) -> EnumDiscriminant {
        let engine = self.writer.engine();
        let (ty, projections) = self.spell_place(place).await;
        let instance = ty.assert_as_enum().clone();
        let enum_layout = engine.build_enum_layout(instance.clone()).await;
        let repr = engine.get_enum_repr(instance).await;

        let storage = match &repr {
            EnumRepr::Inherit { .. } => Storage::Implicit,
            EnumRepr::NumericTag { .. } => {
                Storage::Tag(tag_projections(projections, TagStorage::Alone))
            }
            EnumRepr::NormalTagAndPayload { .. } => {
                Storage::Tag(tag_projections(projections, TagStorage::BesidePayloads))
            }
            EnumRepr::Niche { data, niche, .. } => {
                let (scalar_type, scalar_projections) = self.spell_niche(place, niche).await;
                Storage::Niche {
                    data: *data,
                    niche: niche.clone(),
                    scalar_projections,
                    scalar_type,
                }
            }
        };
        let variant_count =
            u64::try_from(enum_layout.variants().len()).expect("a count should fit in u64");
        EnumDiscriminant { ty: enum_layout.discriminant_type(), variant_count, storage }
    }

    /// The type of the scalar of a niche reached from `place`, and the
    /// spelling of the projections to it. A tag niche's scalar is the tag of
    /// the enum at the niche's path.
    async fn spell_niche(
        &self,
        place: &Place,
        niche: &Niche,
    ) -> (Interned<MonoType>, Vec<ProjectionExpr>) {
        let engine = self.writer.engine();
        let mut scalar_place = place.clone();
        for projection in niche.path() {
            scalar_place = scalar_place.project(engine).with_projection(*projection);
        }
        let (ty, projections) = self.spell_place(&scalar_place).await;

        match niche.scalar() {
            NicheScalar::NonNullPointer | NicheScalar::Bool => (ty, projections),
            NicheScalar::Tag(_) => {
                let repr = engine.get_enum_repr(ty.assert_as_enum().clone()).await;
                let tag = repr.tag_storage().expect("a tag niche is in a tagged enum");
                (ty, tag_projections(projections, tag))
            }
        }
    }
}

/// The reading of a niche-encoded discriminant.
struct NicheReading<'a> {
    /// The niche's scalar.
    scalar: PlaceExpr<'a>,
    scalar_type: Interned<MonoType>,
    niche: &'a Niche,
    /// The variant whose payload holds the niche.
    data: VariantIndex,
    /// The type the discriminant is read as.
    ty: TagType,
    /// How many variants other than `data` the enum has.
    others: u64,
}

/// The projections to the tag of a tagged enum, given those to the enum: an
/// enum whose payloads all hold nothing is its tag.
fn tag_projections(mut projections: Vec<ProjectionExpr>, tag: TagStorage) -> Vec<ProjectionExpr> {
    projections.push(match tag {
        TagStorage::Alone => ProjectionExpr::Transparent,
        TagStorage::BesidePayloads => ProjectionExpr::Member(FieldName::Tag),
    });
    projections
}

/// The value of the niche's scalar that stands for `variant`, which is not
/// `data`: the variants other than `data` take the niche's values in index
/// order.
fn niche_value(
    niche: &Niche,
    data: VariantIndex,
    variant: VariantIndex,
    scalar_type: &Interned<MonoType>,
) -> Constant {
    let rank = u64::from(variant.index()) - u64::from(variant > data);
    niche_constant(niche.scalar().integer_type(), niche.start() + rank, scalar_type)
}

/// The constant `value` of a niche's scalar: of its integer type, or the null
/// pointer, which is a pointer niche's only value.
fn niche_constant(
    integer: Option<TagType>,
    value: u64,
    scalar_type: &Interned<MonoType>,
) -> Constant {
    integer.map_or_else(
        || {
            assert_eq!(value, 0, "null is a pointer niche's only value");
            Constant::NullPointer(scalar_type.clone())
        },
        |integer| integer.constant(value),
    )
}

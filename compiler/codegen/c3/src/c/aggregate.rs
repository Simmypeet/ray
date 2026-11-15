//! Rendering of aggregate `struct` definitions.

use qbice::storage::intern::Interned;
use rayc_mono_ir::ty::MonoType;

use crate::c::{
    name::{AggregateName, FieldName},
    ty::Declaration,
    writer::{CWriter, WriteC},
};

/// One member of an aggregate's `struct`.
#[derive(Debug, Clone)]
pub(crate) enum Member {
    /// A named member, e.g. `int32_t field_0`.
    Field { name: FieldName, ty: Interned<MonoType> },
    /// An anonymous `union` of members that overlap, so their names are
    /// accessed as members of the enclosing struct.
    Union(Members),
    /// An anonymous `struct` of members that do not overlap, as a variant of
    /// a union.
    Struct(Members),
}

impl Member {
    pub(crate) const fn field(name: FieldName, ty: Interned<MonoType>) -> Self {
        Self::Field { name, ty }
    }

    /// Writes the member at `depth` levels of indentation.
    async fn write_c(&self, writer: &mut CWriter<'_>, depth: usize) {
        let indent = Indent(depth);
        match self {
            Self::Field { name, ty } => {
                writer.push(indent);
                writer.write(&Declaration::new(ty, name)).await;
                writer.push(";\n");
            }
            Self::Union(members) => {
                writer.push(format_args!("{indent}union {{\n"));
                Box::pin(members.write_c(writer, depth + 1)).await;
                writer.push(format_args!("{indent}}};\n"));
            }
            Self::Struct(members) => {
                writer.push(format_args!("{indent}struct {{\n"));
                Box::pin(members.write_c(writer, depth + 1)).await;
                writer.push(format_args!("{indent}}};\n"));
            }
        }
    }
}

/// The members of an aggregate, or of a `union` or `struct` within one.
#[derive(Debug, Clone, Default)]
pub(crate) struct Members(Vec<Member>);

impl Members {
    pub(crate) fn push(&mut self, member: Member) { self.0.push(member); }

    /// Calls `visit` with the type of every field, in order, including the
    /// fields of nested unions and structs, which are all held by value.
    pub(crate) fn for_each_type<'a>(&'a self, visit: &mut impl FnMut(&'a Interned<MonoType>)) {
        for member in &self.0 {
            match member {
                Member::Field { ty, .. } => visit(ty),
                Member::Union(members) | Member::Struct(members) => members.for_each_type(visit),
            }
        }
    }

    /// Keeps only the fields whose type `is_stored`, then only the unions
    /// and structs left with a member: an empty one would hold nothing.
    pub(crate) fn retain_stored(&mut self, is_stored: &impl Fn(&MonoType) -> bool) {
        self.0.retain_mut(|member| match member {
            Member::Field { ty, .. } => is_stored(ty),
            Member::Union(members) | Member::Struct(members) => {
                members.retain_stored(is_stored);
                !members.0.is_empty()
            }
        });
    }

    async fn write_c(&self, writer: &mut CWriter<'_>, depth: usize) {
        for member in &self.0 {
            member.write_c(writer, depth).await;
        }
    }
}

impl FromIterator<Member> for Members {
    fn from_iter<T: IntoIterator<Item = Member>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// The `struct` definition of an aggregate that is not zero-sized, e.g.
/// `struct RayTuple_X { int32_t elem0; bool elem1; };`, whose members hold
/// no zero-sized value.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AggregateDefinition<'a> {
    name: AggregateName,
    members: &'a Members,
}

impl<'a> AggregateDefinition<'a> {
    pub(crate) const fn new(name: AggregateName, members: &'a Members) -> Self {
        Self { name, members }
    }

    pub(crate) const fn name(&self) -> AggregateName { self.name }
}

impl WriteC for AggregateDefinition<'_> {
    async fn write_c(&self, writer: &mut CWriter<'_>) {
        writer.push(format_args!("struct {} {{\n", self.name));
        self.members.write_c(writer, 1).await;
        writer.push("};");
    }
}

/// Four spaces per level of nesting.
#[derive(Debug, Clone, Copy)]
struct Indent(usize);

impl std::fmt::Display for Indent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for _ in 0..self.0 {
            formatter.write_str("    ")?;
        }
        Ok(())
    }
}

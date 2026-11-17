//! Rendering of context-free C expressions: places, constants, and operator
//! tokens.

use std::fmt::{self, Display, Write as _};

use qbice::storage::intern::Interned;
use rayc_mono_ir::{
    function::LocalID,
    operand::Constant,
    rvalue::{BinaryOperator, UnaryOperator},
    ty::MonoType,
};

use crate::c::{
    name::{FieldName, LocalName},
    ty::TypeName,
    writer::{CWriter, WriteC},
};

/// The address of every place of a zero-sized type, which has no storage of
/// its own. The runtime defines the anchor, whose address is valid and aligned
/// for any type; since such a place holds nothing, all of them may share it.
pub(crate) const ZERO_SIZED_ADDRESS: &str = "((void *)&ray_zst_anchor)";

/// How one projection of a place is spelled, which depends on the
/// representation of the types it goes between.
#[derive(Debug, Clone)]
pub(crate) enum ProjectionExpr {
    /// `.field`, a member of a direct aggregate.
    Member(FieldName),
    /// Nothing: a transparent aggregate is spelled as the only member it
    /// stores, so selecting that member selects the whole value. Any other
    /// member holds nothing, and a place that holds nothing is never spelled.
    Transparent,
    /// `(*pointer)`.
    Dereference,
    /// `(*(inner *)pointer)`, where the pointee is transparent: the pointer
    /// is spelled as a pointer to the pointee's struct, which is incomplete,
    /// so it is cast to the given pointer to the stored member first.
    DereferenceAs(Interned<MonoType>),
}

/// The lvalue expression designating a place: its local and the spelling of
/// each of its projections, e.g. `(*ray_local_0).elem1`.
///
/// Member access is postfix and binds tighter than a dereference or a cast,
/// so only dereferences need parentheses; the result is always a postfix or
/// parenthesized expression and can be used as an operand anywhere.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlaceExpr<'a> {
    local: LocalID,
    projections: &'a [ProjectionExpr],
}

impl<'a> PlaceExpr<'a> {
    pub(crate) const fn new(local: LocalID, projections: &'a [ProjectionExpr]) -> Self {
        Self { local, projections }
    }
}

impl WriteC for PlaceExpr<'_> {
    async fn write_c(&self, writer: &mut CWriter<'_>) {
        // the last projection is outermost: open every dereference in
        // reverse, then apply the projections in order around the local
        for projection in self.projections.iter().rev() {
            match projection {
                ProjectionExpr::Member(_) | ProjectionExpr::Transparent => {}
                ProjectionExpr::Dereference => writer.push("(*"),
                ProjectionExpr::DereferenceAs(pointer) => {
                    writer.push("(*(");
                    writer.write(&TypeName::new(pointer)).await;
                    writer.push(")");
                }
            }
        }

        writer.push(LocalName(self.local));

        for projection in self.projections {
            match projection {
                ProjectionExpr::Member(field) => writer.push(format_args!(".{field}")),
                ProjectionExpr::Transparent => {}
                ProjectionExpr::Dereference | ProjectionExpr::DereferenceAs(_) => {
                    writer.push(")");
                }
            }
        }
    }
}

/// A constant operand, which is not zero-sized: a value that holds nothing is
/// never spelled.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ConstantExpr<'a> {
    constant: &'a Constant,
}

impl<'a> ConstantExpr<'a> {
    pub(crate) const fn new(constant: &'a Constant) -> Self { Self { constant } }
}

impl WriteC for ConstantExpr<'_> {
    async fn write_c(&self, writer: &mut CWriter<'_>) {
        match self.constant {
            Constant::Unit | Constant::EmptyAggregate(_) => {
                panic!("a zero-sized constant is never spelled: {:?}", self.constant)
            }
            Constant::Bool(value) => writer.push(value),
            // the `<stdint.h>` macros give each literal its exact width
            Constant::Int8(value) => writer.push(format_args!("INT8_C({value})")),
            Constant::Int16(value) => writer.push(format_args!("INT16_C({value})")),
            Constant::Int32(value) => writer.push(format_args!("INT32_C({value})")),
            Constant::Int64(value) => writer.push(format_args!("INT64_C({value})")),
            Constant::Uint8(value) => writer.push(format_args!("UINT8_C({value})")),
            Constant::Uint16(value) => writer.push(format_args!("UINT16_C({value})")),
            Constant::Uint32(value) => writer.push(format_args!("UINT32_C({value})")),
            Constant::Uint64(value) => writer.push(format_args!("UINT64_C({value})")),
            Constant::Isize(value) => writer.push(format_args!("((intptr_t)INT64_C({value}))")),
            Constant::Usize(value) => writer.push(format_args!("((uintptr_t)UINT64_C({value}))")),
            Constant::Float32(bits) => writer.push(Float32Literal(f32::from_bits(*bits))),
            Constant::CInt(value) => writer.push(value),
            Constant::CStr(value) => writer.push(StringLiteral(value)),
            // the only constant that spells a type
            Constant::NullPointer(ty) => {
                writer.push("((");
                writer.write(&TypeName::new(ty)).await;
                writer.push(")0)");
            }
        }
    }
}

/// A `float` literal, spelling non-finite values with `<math.h>` macros.
struct Float32Literal(f32);

impl Display for Float32Literal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self.0;
        if value.is_nan() {
            formatter.write_str("NAN")
        } else if value == f32::INFINITY {
            formatter.write_str("INFINITY")
        } else if value == f32::NEG_INFINITY {
            formatter.write_str("(-INFINITY)")
        } else {
            write!(formatter, "{value:?}f")
        }
    }
}

/// A C string literal; bytes outside printable ASCII are octal-escaped.
struct StringLiteral<'a>(&'a str);

impl Display for StringLiteral<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_char('"')?;
        for byte in self.0.bytes() {
            match byte {
                b'\\' => formatter.write_str("\\\\")?,
                b'"' => formatter.write_str("\\\"")?,
                b'\n' => formatter.write_str("\\n")?,
                b'\r' => formatter.write_str("\\r")?,
                b'\t' => formatter.write_str("\\t")?,
                0x20..=0x7E => formatter.write_char(char::from(byte))?,
                _ => write!(formatter, "\\{byte:03o}")?,
            }
        }
        formatter.write_char('"')
    }
}

/// The C token of a unary operator.
pub(crate) const fn unary_operator_token(operator: UnaryOperator) -> &'static str {
    match operator {
        UnaryOperator::Negate => "-",
        UnaryOperator::LogicalNot => "!",
        UnaryOperator::BitwiseNot => "~",
    }
}

/// The C token of a binary operator.
pub(crate) const fn binary_operator_token(operator: BinaryOperator) -> &'static str {
    match operator {
        BinaryOperator::Equal => "==",
        BinaryOperator::NotEqual => "!=",
        BinaryOperator::Less => "<",
        BinaryOperator::LessEqual => "<=",
        BinaryOperator::Greater => ">",
        BinaryOperator::GreaterEqual => ">=",
        BinaryOperator::Add => "+",
        BinaryOperator::Subtract => "-",
        BinaryOperator::Multiply => "*",
        BinaryOperator::Divide => "/",
        BinaryOperator::Remainder => "%",
        BinaryOperator::LogicalAnd => "&&",
        BinaryOperator::LogicalOr => "||",
        BinaryOperator::BitwiseAnd => "&",
        BinaryOperator::BitwiseOr => "|",
        BinaryOperator::BitwiseXor => "^",
        BinaryOperator::ShiftLeft => "<<",
        BinaryOperator::ShiftRight => ">>",
    }
}

//! Rendering of context-free C expressions: places, constants, and operator
//! tokens.

use std::fmt::{self, Display, Write as _};

use rayc_mono_ir::{
    operand::Constant,
    place::{Place, Projection},
    rvalue::{BinaryOperator, UnaryOperator},
    ty::{AggregateType, MonoType},
};

use crate::c::{
    name::{AggregateName, FieldName, LocalName},
    ty::TypeName,
};

/// The lvalue expression designating a [`Place`], e.g.
/// `((*(ray_local_0))).elem1`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlaceExpr<'a>(pub(crate) &'a Place);

impl Display for PlaceExpr<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Each projection wraps the expression built so far, so the last
        // projection is outermost: open them in reverse, then close them in
        // order around the local.
        let projections = self.0.projections();
        for projection in projections.iter().rev() {
            formatter.write_str(match projection {
                Projection::Dereference => "(*(",
                Projection::EnvironmentFieldIndex(_)
                | Projection::TupleFieldIndex(_)
                | Projection::StructFieldIndex(_)
                | Projection::OperationRecordEnvironmentField(_)
                | Projection::OperationRecordFunctionPointerField(_) => "(",
            })?;
        }

        LocalName(self.0.local()).fmt(formatter)?;

        for projection in projections {
            match FieldName::of_projection(*projection) {
                Some(field) => write!(formatter, ").{field}")?,
                None => formatter.write_str("))")?,
            }
        }
        Ok(())
    }
}

/// A compound literal of an aggregate with no members, e.g.
/// `((RayTuple_<id>_t){ ._unit = 0 })`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EmptyAggregateLiteral<'a>(pub(crate) &'a AggregateType);

impl Display for EmptyAggregateLiteral<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "(({}){{ ._unit = 0 }})", AggregateName::of(self.0).typedef())
    }
}

/// A constant operand.
///
/// `expected` is the type the surrounding context requires; it is only
/// consulted for the unit constant, whose C type is not inherent in it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ConstantExpr<'a> {
    constant: &'a Constant,
    expected: Option<&'a MonoType>,
}

impl<'a> ConstantExpr<'a> {
    pub(crate) const fn new(constant: &'a Constant, expected: Option<&'a MonoType>) -> Self {
        Self { constant, expected }
    }

    fn write_unit(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(MonoType::Aggregate(aggregate @ AggregateType::Tuple(tuple))) = self.expected
        else {
            panic!("a unit constant requires its expected tuple type during C emission")
        };
        assert!(tuple.is_empty(), "a unit constant requires an empty tuple type");
        EmptyAggregateLiteral(aggregate).fmt(formatter)
    }
}

impl Display for ConstantExpr<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.constant {
            Constant::Unit => self.write_unit(formatter),
            Constant::Bool(value) => value.fmt(formatter),
            Constant::Int8(value) => write!(formatter, "INT8_C({value})"),
            Constant::Int16(value) => write!(formatter, "INT16_C({value})"),
            Constant::Int32(value) => write!(formatter, "INT32_C({value})"),
            Constant::Int64(value) => write!(formatter, "INT64_C({value})"),
            Constant::Uint8(value) => write!(formatter, "UINT8_C({value})"),
            Constant::Uint16(value) => write!(formatter, "UINT16_C({value})"),
            Constant::Uint32(value) => write!(formatter, "UINT32_C({value})"),
            Constant::Uint64(value) => write!(formatter, "UINT64_C({value})"),
            Constant::Isize(value) => write!(formatter, "((intptr_t)INT64_C({value}))"),
            Constant::Usize(value) => write!(formatter, "((uintptr_t)UINT64_C({value}))"),
            Constant::Float32(bits) => Float32Literal(f32::from_bits(*bits)).fmt(formatter),
            Constant::CInt(value) => value.fmt(formatter),
            Constant::CStr(value) => StringLiteral(value).fmt(formatter),
            Constant::NullPointer(ty) => write!(formatter, "(({})0)", TypeName(ty)),
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

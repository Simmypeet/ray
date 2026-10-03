use std::num::IntErrorKind;

use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_source_file::SourceElement;
use rayc_syntax::expression::{Boolean, Literal as LiteralSyntax, NumericLiteral, NumericSuffix};
use rayc_type::ty::{Float, InferenceConstraint, Integer, Primitive, Ty};
use rayc_typed_ast::typed_expr::{TypedExprID, TypedExprKind, literal::Literal};

use crate::{
    bind::Bind,
    diagnostic::{
        Diagnostic, EmbeddedNulString, FloatLiteralIntegerSuffix, NumericLiteralTooLarge,
    },
    tast_builder::{TAstBuilder, constraint_solver::NumericOperation},
};

impl Bind<LiteralSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: LiteralSyntax) -> TypedExprID {
        if let LiteralSyntax::String(token) = &syn
            && token.kind.0.as_bytes().contains(&0)
        {
            self.push_diagnostic(Diagnostic::EmbeddedNulString(
                EmbeddedNulString::builder().span(syn.span()).build(),
            ));
        }
        let (literal, ty) = match &syn {
            LiteralSyntax::Boolean(boolean) => {
                let value = match boolean {
                    Boolean::True(_) => true,
                    Boolean::False(_) => false,
                };

                (Literal::Bool(value), Ty::new_primitive(Primitive::Bool, self.engine()))
            }

            LiteralSyntax::Numeric(numeric) => {
                return self.bind_numeric_literal(numeric, syn.span(), false).await;
            }
            LiteralSyntax::String(token) => (
                Literal::String(token.kind.0.clone()),
                Ty::new_primitive(Primitive::CStr, self.engine()),
            ),
        };

        self.insert_expression(TypedExprKind::Literal(literal), syn.span(), ty).await
    }
}

impl TAstBuilder {
    /// Binds a numeric literal. A suffix such as `i8` in `23i8` fixes the
    /// literal's type; otherwise the type is inferred, defaulting to `int32`
    /// for an integer literal and to `float64` for a floating-point literal.
    ///
    /// When `negated` is set, the literal is the operand of a negation, as in
    /// `-128i8`, and is bound as a single negative literal spanning `span`.
    /// Its type must then be a signed numeric type.
    pub(crate) async fn bind_numeric_literal(
        &mut self,
        syn: &NumericLiteral,
        span: RelativeSpan,
        negated: bool,
    ) -> TypedExprID {
        let Some(numeric) = syn.numeric() else {
            return self.push_error_expression(span).await;
        };

        // The sign is part of the literal's digits, so a negated literal is
        // parsed and range checked as a negative value.
        let sign = if negated { "-" } else { "" };
        let id = if let Some(fraction) = syn.fraction() {
            let Some(fraction_digits) = fraction.numeric() else {
                return self.push_error_expression(span).await;
            };
            let digits = format!("{sign}{}.{}", &*numeric.kind.0, &*fraction_digits.kind.0);
            self.bind_float_literal(syn, digits, span).await
        } else {
            let digits = format!("{sign}{}", &*numeric.kind.0);
            self.bind_integer_literal(syn, digits, span).await
        };

        // A negative literal of an unsigned type, such as `-1u32`, is reported
        // as an invalid negation.
        if negated {
            self.push_numeric_operand_constraint(NumericOperation::Negation, id).await;
        }

        id
    }

    /// Binds a numeric literal without a fractional part, such as `23`,
    /// `-23i8`, or `23f32`.
    async fn bind_integer_literal(
        &mut self,
        syn: &NumericLiteral,
        digits: String,
        span: RelativeSpan,
    ) -> TypedExprID {
        let digits: Interned<str> = self.engine().intern_unsized(digits);

        let value = match digits.parse::<i128>() {
            Ok(value) => value,

            // The literal exceeds every integer type, so it is reported right
            // away instead of by the deferred range check.
            Err(error)
                if matches!(
                    error.kind(),
                    IntErrorKind::PosOverflow | IntErrorKind::NegOverflow
                ) =>
            {
                self.push_diagnostic(Diagnostic::NumericLiteralTooLarge(
                    NumericLiteralTooLarge::builder().literal(digits).span(span).build(),
                ));
                return self.push_error_expression(span).await;
            }

            // The lexer only produces non-empty sequences of ASCII digits.
            Err(error) => {
                panic!("numeric literal `{}` should've been valid digits: {error}", &*digits)
            }
        };

        let ty = match syn.suffix() {
            Some(suffix) => Ty::new_primitive(suffix_primitive(&suffix), self.engine()),
            None => self.new_numeric_type_inference(),
        };

        let id =
            self.insert_expression(TypedExprKind::Literal(Literal::Numeric(value)), span, ty).await;

        // The literal's type may still be an inference variable here, so its
        // range is checked once every type has been inferred.
        self.require_numeric_literal_range(id, digits, value);

        id
    }

    /// Binds a numeric literal with a fractional part, such as `1.5` or
    /// `-1.5f32`.
    async fn bind_float_literal(
        &mut self,
        syn: &NumericLiteral,
        digits: String,
        span: RelativeSpan,
    ) -> TypedExprID {
        let ty = match syn.suffix() {
            Some(suffix) => {
                let primitive = suffix_primitive(&suffix);

                // Only a floating-point suffix can fix the type of a literal
                // with a fractional part.
                if !primitive.satisfies_constraint(InferenceConstraint::FloatingPoint) {
                    self.push_diagnostic(Diagnostic::FloatLiteralIntegerSuffix(
                        FloatLiteralIntegerSuffix::builder()
                            .literal(self.engine().intern_unsized(digits))
                            .primitive(primitive)
                            .span(span)
                            .build(),
                    ));
                    return self.push_error_expression(span).await;
                }

                Ty::new_primitive(primitive, self.engine())
            }
            None => self.new_floating_point_type_inference(),
        };

        let digits = self.engine().intern_unsized(digits);
        self.insert_expression(TypedExprKind::Literal(Literal::Float(digits)), span, ty).await
    }
}

/// Returns the primitive type that a numeric literal suffix fixes.
const fn suffix_primitive(suffix: &NumericSuffix) -> Primitive {
    match suffix {
        NumericSuffix::I8(_) => Primitive::Integer(Integer::Int8),
        NumericSuffix::I16(_) => Primitive::Integer(Integer::Int16),
        NumericSuffix::I32(_) => Primitive::Integer(Integer::Int32),
        NumericSuffix::I64(_) => Primitive::Integer(Integer::Int64),
        NumericSuffix::Isize(_) => Primitive::Integer(Integer::Isize),
        NumericSuffix::U8(_) => Primitive::Integer(Integer::Uint8),
        NumericSuffix::U16(_) => Primitive::Integer(Integer::Uint16),
        NumericSuffix::U32(_) => Primitive::Integer(Integer::Uint32),
        NumericSuffix::U64(_) => Primitive::Integer(Integer::Uint64),
        NumericSuffix::Usize(_) => Primitive::Integer(Integer::Usize),
        NumericSuffix::F32(_) => Primitive::Float(Float::Float32),
        NumericSuffix::F64(_) => Primitive::Float(Float::Float64),
    }
}

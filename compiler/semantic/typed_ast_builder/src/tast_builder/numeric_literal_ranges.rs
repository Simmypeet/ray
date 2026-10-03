use qbice::storage::intern::Interned;
use rayc_type::ty::Primitive;
use rayc_typed_ast::{typed_expr::TypedExprID, typed_function::TypedFunctionLocalID};

use crate::{
    diagnostic::{Diagnostic, NumericLiteralOutOfRange},
    tast_builder::TAstBuilder,
};

/// The numeric literals whose value must fit in their type.
///
/// A literal's type may still be an inference variable when it is bound, so
/// the ranges are checked once every type has been inferred.
#[derive(Debug)]
pub(super) struct NumericLiteralRanges {
    queued: Vec<NumericLiteralRange>,
}

impl NumericLiteralRanges {
    #[must_use]
    pub(super) const fn new() -> Self { Self { queued: Vec::new() } }

    fn push(&mut self, range: NumericLiteralRange) { self.queued.push(range); }

    fn take(&mut self) -> Vec<NumericLiteralRange> { std::mem::take(&mut self.queued) }
}

#[derive(Debug, Clone)]
struct NumericLiteralRange {
    expression: TypedFunctionLocalID<TypedExprID>,

    /// The digits of the literal as written in the source code.
    digits: Interned<str>,

    value: u128,

    /// Whether the literal is the operand of a negation, as in `-128i8`.
    negated: bool,
}

impl NumericLiteralRange {
    /// Checks whether the literal fits in the given primitive type. Returns
    /// the largest value of the type too, if it is an integer type.
    ///
    /// A negated literal may be one more than the largest value of a signed
    /// integer type, since its negation is the smallest value of the type.
    const fn fits_in(&self, primitive: Primitive) -> (bool, Option<u128>) {
        match primitive {
            Primitive::Integer(integer) => {
                let max = integer.max_value();
                let limit = if self.negated && integer.is_signed() { max + 1 } else { max };
                (self.value <= limit, Some(max))
            }
            Primitive::Float(_) | Primitive::Bool | Primitive::CStr => (true, None),
        }
    }
}

impl TAstBuilder {
    /// Requires the value of the given numeric literal expression to fit in
    /// its type.
    pub(crate) fn require_numeric_literal_range(
        &mut self,
        expression: TypedExprID,
        digits: Interned<str>,
        value: u128,
        negated: bool,
    ) {
        self.numeric_literal_ranges.push(NumericLiteralRange {
            expression: TypedFunctionLocalID::new(self.current_typed_function_id(), expression),
            digits,
            value,
            negated,
        });
    }

    /// Checks every required numeric literal range against the literal's
    /// inferred type. Must run after numeric inference variables are
    /// defaulted.
    pub(super) async fn validate_numeric_literal_ranges(&mut self) {
        for range in self.numeric_literal_ranges.take() {
            let ty = self.latest_type(&self.type_of_local_expression(range.expression)).await;

            // A numeric literal whose type is not a primitive has already been
            // reported by type checking.
            let Some(primitive) = ty.as_primitive() else {
                continue;
            };

            let (fits, max) = range.fits_in(primitive);
            if fits {
                continue;
            }

            self.push_diagnostic(Diagnostic::NumericLiteralOutOfRange(
                NumericLiteralOutOfRange::builder()
                    .literal(range.digits)
                    .primitive(primitive)
                    .maybe_max(max)
                    .negated(range.negated)
                    .span(self.span_of_local_expression(range.expression))
                    .build(),
            ));
        }
    }
}

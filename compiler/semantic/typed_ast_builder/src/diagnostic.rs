use bon::Builder;
use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    GlobalSymbolID, name::get_qualified_name, source_map::to_absolute_span,
    symbol_kind::get_symbol_kind,
};
use rayc_type::ty::Ty;

use crate::tast_builder::constraint_solver::{SubtypeProvenance, SubtypeSource};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct UnboundName {
    name: Interned<str>,
    span: RelativeSpan,
}

impl Report for UnboundName {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let abs_span = engine.to_absolute_span(&self.span).await;

        Rendered::builder()
            .message(format!("unbound name `{}`", &*self.name))
            .primary_highlight(Highlight::builder().span(abs_span).build())
            .build()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct FunctionNotFound {
    name: Interned<str>,
    span: RelativeSpan,
}

impl Report for FunctionNotFound {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let abs_span = engine.to_absolute_span(&self.span).await;

        Rendered::builder()
            .message(format!("function `{}` not found", &*self.name))
            .primary_highlight(Highlight::builder().span(abs_span).build())
            .build()
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder,
)]
pub struct SymbolNotCallable {
    name: GlobalSymbolID,
    span: RelativeSpan,
}

impl Report for SymbolNotCallable {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let qual_name = engine.get_qualified_name(self.name).await;
        let kind = engine.get_symbol_kind(self.name).await;

        let abs_span = engine.to_absolute_span(&self.span).await;

        Rendered::builder()
            .message(format!("symbol `{} {}` is not callable", kind.str(), qual_name))
            .primary_highlight(Highlight::builder().span(abs_span).build())
            .build()
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder,
)]
pub struct MismatchedArgumentCount {
    calling_symbol: GlobalSymbolID,
    expected: usize,
    found: usize,
    span: RelativeSpan,
}

impl Report for MismatchedArgumentCount {
    async fn report(&self, parameter: &TrackedEngine) -> Rendered<ByteIndex> {
        let qual_name = parameter.get_qualified_name(self.calling_symbol).await;

        let abs_span = parameter.to_absolute_span(&self.span).await;

        Rendered::builder()
            .message(format!(
                "function `{}` expects {} arguments, but {} were provided",
                qual_name, self.expected, self.found
            ))
            .primary_highlight(Highlight::builder().span(abs_span).build())
            .build()
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder,
)]
pub struct MismatchedIndirectArgumentCount {
    expected: usize,
    found: usize,
    span: RelativeSpan,
}

impl Report for MismatchedIndirectArgumentCount {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let abs_span = engine.to_absolute_span(&self.span).await;

        Rendered::builder()
            .message(format!(
                "lambda expects {} arguments, but {} were provided",
                self.expected, self.found
            ))
            .primary_highlight(Highlight::builder().span(abs_span).build())
            .build()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct ExpectedLambdaType {
    ty: Interned<Ty>,
    span: RelativeSpan,
}

impl Report for ExpectedLambdaType {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let abs_span = engine.to_absolute_span(&self.span).await;

        Rendered::builder()
            .message(format!(
                "expected a lambda type, but found `{}`",
                self.ty.display(engine).await
            ))
            .primary_highlight(Highlight::builder().span(abs_span).build())
            .build()
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder,
)]
pub struct TypeMustBeKnownAtThisPoint {
    span: RelativeSpan,
}

impl Report for TypeMustBeKnownAtThisPoint {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let abs_span = engine.to_absolute_span(&self.span).await;

        Rendered::builder()
            .message("type must be known at this point")
            .primary_highlight(
                Highlight::builder()
                    .span(abs_span)
                    .message("consider annotating the explicit type for this expression")
                    .build(),
            )
            .build()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct ExpectedTupleType {
    ty: Interned<Ty>,
    span: RelativeSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct ExpectedPointerType {
    ty: Interned<Ty>,
    span: RelativeSpan,
}

impl Report for ExpectedPointerType {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let abs_span = engine.to_absolute_span(&self.span).await;

        Rendered::builder()
            .message(format!(
                "expected a pointer type, but found `{}`",
                self.ty.display(engine).await
            ))
            .primary_highlight(Highlight::builder().span(abs_span).build())
            .build()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum LvalueOperation {
    Assignment,
    Reference,
    MutableReference,
}

impl LvalueOperation {
    const fn description(self) -> &'static str {
        match self {
            Self::Assignment => "assignment",
            Self::Reference => "reference-of operation",
            Self::MutableReference => "mutable reference-of operation",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder,
)]
pub struct ExpectedLvalue {
    operation: LvalueOperation,
    span: RelativeSpan,
}

impl Report for ExpectedLvalue {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let abs_span = engine.to_absolute_span(&self.span).await;

        Rendered::builder()
            .message(format!("{} requires an lvalue", self.operation.description()))
            .primary_highlight(
                Highlight::builder()
                    .span(abs_span)
                    .message("this expression is not addressable")
                    .build(),
            )
            .build()
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder,
)]
pub struct ImmutableLvalue {
    operation: LvalueOperation,
    span: RelativeSpan,
}

impl Report for ImmutableLvalue {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let abs_span = engine.to_absolute_span(&self.span).await;

        Rendered::builder()
            .message(format!("{} requires a mutable lvalue", self.operation.description()))
            .primary_highlight(
                Highlight::builder()
                    .span(abs_span)
                    .message("this destination is immutable")
                    .build(),
            )
            .build()
    }
}

impl Report for ExpectedTupleType {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let abs_span = engine.to_absolute_span(&self.span).await;

        Rendered::builder()
            .message(format!(
                "expected a tuple type, but found `{}`",
                self.ty.display(engine).await
            ))
            .primary_highlight(Highlight::builder().span(abs_span).build())
            .build()
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder,
)]
pub struct OutOfBoundsTupleIndex {
    span: RelativeSpan,
    index: usize,
    tuple_len: usize,
}

impl Report for OutOfBoundsTupleIndex {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let abs_span = engine.to_absolute_span(&self.span).await;

        Rendered::builder()
            .message(format!(
                "tuple index {} is out of bounds for tuple of length {}",
                self.index, self.tuple_len
            ))
            .primary_highlight(Highlight::builder().span(abs_span).build())
            .build()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct DuplicateNameBinding {
    existing_name_binding: RelativeSpan,
    new_name_binding: RelativeSpan,
    new_name: Interned<str>,
}

impl Report for DuplicateNameBinding {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let existing_abs_span = engine.to_absolute_span(&self.existing_name_binding).await;
        let new_abs_span = engine.to_absolute_span(&self.new_name_binding).await;

        Rendered::builder()
            .message(format!("duplicate name binding for `{}`", &*self.new_name))
            .primary_highlight(
                Highlight::builder()
                    .span(new_abs_span)
                    .message("this name binding is a duplicate")
                    .build(),
            )
            .related(vec![
                Highlight::builder()
                    .span(existing_abs_span)
                    .message("the existing name binding is here")
                    .build(),
            ])
            .build()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct ResidualSubtype {
    provenance: SubtypeProvenance,
}

impl Report for ResidualSubtype {
    async fn report(&self, parameter: &TrackedEngine) -> Rendered<ByteIndex> {
        let header_msg = match self.provenance.source() {
            SubtypeSource::FunctionCall => "mismatched argument types in function call",
            SubtypeSource::LambdaInvocation => "mismatched argument types in lambda invocation",
            SubtypeSource::VariableAssignment => "mismatched types in variable assignment",
            SubtypeSource::BinaryOperator => "mismatched types in binary operation",
            SubtypeSource::IfCondition => "if expression condition must be `bool`",
            SubtypeSource::IfBranch => "mismatched types in if expression branches",
            SubtypeSource::ReturnType => "mismatched types in return expression",
        };

        let found = self.provenance.original_subtype().greater();
        let expected = self.provenance.original_subtype().lesser();

        let expected_display = expected.display(parameter).await;
        let found_display = found.display(parameter).await;
        let mismatch_str = format!("expected `{expected_display}`, but found `{found_display}`");
        let abs_span = parameter.to_absolute_span(self.provenance.span()).await;

        Rendered::builder()
            .message(format!("{header_msg}: {mismatch_str}"))
            .primary_highlight(Highlight::builder().span(abs_span).build())
            .build()
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable,
)]
pub enum Diagnostic {
    UnboundName(UnboundName),
    FunctionNotFound(FunctionNotFound),
    SymbolNotCallable(SymbolNotCallable),
    MismatchedArgumentCount(MismatchedArgumentCount),
    MismatchedIndirectArgumentCount(MismatchedIndirectArgumentCount),
    ExpectedLambdaType(ExpectedLambdaType),
    TypeMustBeKnownAtThisPoint(TypeMustBeKnownAtThisPoint),
    ExpectedTupleType(ExpectedTupleType),
    ExpectedPointerType(ExpectedPointerType),
    ExpectedLvalue(ExpectedLvalue),
    ImmutableLvalue(ImmutableLvalue),
    OutOfBoundsTupleIndex(OutOfBoundsTupleIndex),
    DuplicateNameBinding(DuplicateNameBinding),
    ResidualSubtype(ResidualSubtype),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::UnboundName(unbound_name) => unbound_name.report(engine).await,
            Self::FunctionNotFound(function_not_found) => function_not_found.report(engine).await,
            Self::SymbolNotCallable(symbol_not_callable) => {
                symbol_not_callable.report(engine).await
            }
            Self::MismatchedArgumentCount(mismatched_argument_count) => {
                mismatched_argument_count.report(engine).await
            }
            Self::MismatchedIndirectArgumentCount(mismatched_argument_count) => {
                mismatched_argument_count.report(engine).await
            }
            Self::ExpectedLambdaType(expected_lambda_type) => {
                expected_lambda_type.report(engine).await
            }
            Self::TypeMustBeKnownAtThisPoint(type_must_be_known) => {
                type_must_be_known.report(engine).await
            }
            Self::ExpectedTupleType(expected_tuple_type) => {
                expected_tuple_type.report(engine).await
            }
            Self::ExpectedPointerType(expected_pointer_type) => {
                expected_pointer_type.report(engine).await
            }
            Self::ExpectedLvalue(expected_lvalue) => expected_lvalue.report(engine).await,
            Self::ImmutableLvalue(immutable_lvalue) => immutable_lvalue.report(engine).await,
            Self::OutOfBoundsTupleIndex(out_of_bounds_tuple_index) => {
                out_of_bounds_tuple_index.report(engine).await
            }
            Self::DuplicateNameBinding(duplicate_name_binding) => {
                duplicate_name_binding.report(engine).await
            }
            Self::ResidualSubtype(residual_subtype) => residual_subtype.report(engine).await,
        }
    }
}

use bon::Builder;
use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    GlobalSymbolID, name::get_qualified_name, source_map::to_absolute_span,
    symbol_kind::get_symbol_kind,
};
use rayc_type::{constraint::ty_relate::TyRelate, ty::Ty};

use crate::tast_builder::constraint_solver::{
    ConstraintError, EffectUnificationSource, SubtypeSource,
};

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
                "callable expects {} arguments, but {} were provided",
                self.expected, self.found
            ))
            .primary_highlight(Highlight::builder().span(abs_span).build())
            .build()
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder,
)]
pub struct EmbeddedNulString {
    span: RelativeSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct MissingEffectOperationHandler {
    operations: Vec<GlobalSymbolID>,
    span: RelativeSpan,
}

impl Report for MissingEffectOperationHandler {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let mut operations = Vec::with_capacity(self.operations.len());
        for operation in &self.operations {
            operations.push(format!("`{}`", engine.get_qualified_name(*operation).await));
        }
        let operations = operations.join(", ");
        let (message, highlight) = if self.operations.len() == 1 {
            (
                format!("missing handler for effect operation {operations}"),
                format!("add a handler for {operations}"),
            )
        } else {
            (
                format!("missing handlers for effect operations {operations}"),
                format!("add handlers for {operations}"),
            )
        };

        Rendered::builder()
            .message(message)
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.span).await)
                    .message(highlight)
                    .build(),
            )
            .build()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct ExtraneousEffectOperationHandler {
    effect: GlobalSymbolID,
    name: Interned<str>,
    span: RelativeSpan,
}

impl Report for ExtraneousEffectOperationHandler {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let effect = engine.get_qualified_name(self.effect).await;
        Rendered::builder()
            .message(format!("effect `{effect}` has no operation named `{}`", &*self.name))
            .primary_highlight(
                Highlight::builder().span(engine.to_absolute_span(&self.span).await).build(),
            )
            .build()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct DuplicateEffectOperationHandler {
    name: Interned<str>,
    original_span: RelativeSpan,
    duplicate_span: RelativeSpan,
}

impl Report for DuplicateEffectOperationHandler {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message(format!("duplicate handler for effect operation `{}`", &*self.name))
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.duplicate_span).await)
                    .message("duplicate handler")
                    .build(),
            )
            .related(vec![
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.original_span).await)
                    .message("the first handler is here")
                    .build(),
            ])
            .build()
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder,
)]
pub struct MismatchedEffectOperationHandlerParameterCount {
    operation: GlobalSymbolID,
    expected: usize,
    found: usize,
    span: RelativeSpan,
}

impl Report for MismatchedEffectOperationHandlerParameterCount {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let operation = engine.get_qualified_name(self.operation).await;
        Rendered::builder()
            .message(format!(
                "handler for `{operation}` expects {} parameters, but {} were provided",
                self.expected, self.found
            ))
            .primary_highlight(
                Highlight::builder().span(engine.to_absolute_span(&self.span).await).build(),
            )
            .build()
    }
}

impl Report for EmbeddedNulString {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message("a C string literal must not contain an embedded NUL byte")
            .primary_highlight(
                Highlight::builder().span(engine.to_absolute_span(&self.span).await).build(),
            )
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
    span: RelativeSpan,
    source: SubtypeSource,
    subype: TyRelate,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct InstanceResolution {
    span: RelativeSpan,
    trait_ref: rayc_type::trait_ref::TraitRef,
    error: ConstraintError,
}

impl Report for InstanceResolution {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        use rayc_solver::{instance_resolution::InstanceResolutionError, ty_relate::Error};

        // Interpret the stored failure only when rendering the diagnostic.
        let message = match &self.error {
            ConstraintError::TyRelate(Error::Conflicted) => "conflicting implicit instance",
            ConstraintError::TyRelate(Error::OccursCheckFailed) => {
                "implicit instance fails the occurs check"
            }
            ConstraintError::InstanceResolve(error) => match error {
                InstanceResolutionError::NotReady(_) => "cannot infer instance requirement",
                InstanceResolutionError::ContainsError(_) => {
                    "instance requirement contains an error"
                }
                InstanceResolutionError::NoInstance { .. } => "no implicit instance found",
                InstanceResolutionError::AmbiguousLexical { .. } => "ambiguous lexical instances",
                InstanceResolutionError::AmbiguousGlobal { .. } => "ambiguous global instances",
                InstanceResolutionError::Cycle(_) => "cyclic instance resolution",
                InstanceResolutionError::Limit { .. } => "instance resolution limit exceeded",
            },
        };

        let name = engine.get_qualified_name(self.trait_ref.trait_id()).await;
        let mut args = Vec::new();
        for arg in self.trait_ref.args().iter() {
            args.push(arg.display(engine).await.to_string());
        }
        Rendered::builder()
            .message(format!("{message}: `{name}[{}]`", args.join(", ")))
            .primary_highlight(
                Highlight::builder().span(engine.to_absolute_span(&self.span).await).build(),
            )
            .build()
    }
}

impl Report for ResidualSubtype {
    async fn report(&self, parameter: &TrackedEngine) -> Rendered<ByteIndex> {
        let header_msg = match &self.source {
            SubtypeSource::FunctionCall => "mismatched argument types in function call",
            SubtypeSource::ClosureCaptures => "incompatible closure capture types",
            SubtypeSource::VariableAssignment => "mismatched types in variable assignment",
            SubtypeSource::BinaryOperator => "mismatched types in binary operation",
            SubtypeSource::IfCondition => "if expression condition must be `bool`",
            SubtypeSource::IfBranch => "mismatched types in if expression branches",
            SubtypeSource::ReturnType => "mismatched types in return expression",
        };

        let found = self.subype.greater();
        let expected = self.subype.lesser();

        let expected_display = expected.display(parameter).await;
        let found_display = found.display(parameter).await;
        let mismatch_str = format!("expected `{expected_display}`, but found `{found_display}`");
        let abs_span = parameter.to_absolute_span(&self.span).await;

        Rendered::builder()
            .message(format!("{header_msg}: {mismatch_str}"))
            .primary_highlight(Highlight::builder().span(abs_span).build())
            .build()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct EffectUnificationSite {
    effect_row: Interned<Ty>,
    span: RelativeSpan,
    source: EffectUnificationSource,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Builder)]
pub struct IncompatibleEffectRows {
    primary_span: RelativeSpan,
    lesser: Interned<Ty>,
    greater: Interned<Ty>,
    source: EffectUnificationSource,
    related_sites: Vec<EffectUnificationSite>,
}

impl Report for IncompatibleEffectRows {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let lesser = self.lesser.display(engine).await;
        let greater = self.greater.display(engine).await;
        let message = match &self.source {
            EffectUnificationSource::EffectSharing => {
                format!("incompatible effect rows `{lesser}` and `{greater}`")
            }
            EffectUnificationSource::EffectIntroduction { original_effect } => {
                let original_effect = original_effect.display(engine).await;
                format!("effect `{original_effect}` cannot be introduced into `{greater}`")
            }
            EffectUnificationSource::FunctionBodyEffect => {
                format!(
                    "function body effects do not match its signature: expected `{lesser}`, but \
                     found `{greater}`"
                )
            }
        };

        let primary_message = match self.source {
            EffectUnificationSource::EffectSharing => "these effect rows cannot be composed",
            EffectUnificationSource::EffectIntroduction { .. } => {
                "this expression introduces an incompatible effect"
            }
            EffectUnificationSource::FunctionBodyEffect => {
                "the function body has effects outside its signature"
            }
        };

        let mut related = Vec::with_capacity(self.related_sites.len());
        for site in &self.related_sites {
            let effect_row = site.effect_row.display(engine).await;
            let message = match &site.source {
                EffectUnificationSource::EffectIntroduction { original_effect } => {
                    let original_effect = original_effect.display(engine).await;
                    format!("effect `{original_effect}` introduced here")
                }
                EffectUnificationSource::FunctionBodyEffect => {
                    format!(
                        "function body effect `{effect_row}` is checked against its signature here"
                    )
                }
                EffectUnificationSource::EffectSharing => {
                    format!("effect row `{effect_row}` is composed here")
                }
            };

            related.push(
                Highlight::builder()
                    .span(engine.to_absolute_span(&site.span).await)
                    .message(message)
                    .build(),
            );
        }

        Rendered::builder()
            .message(message)
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.primary_span).await)
                    .message(primary_message)
                    .build(),
            )
            .related(related)
            .build()
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable, From,
)]
pub enum Diagnostic {
    InstanceResolution(InstanceResolution),
    Resolution(rayc_resolution::Diagnostic),
    UnboundName(UnboundName),
    FunctionNotFound(FunctionNotFound),
    SymbolNotCallable(SymbolNotCallable),
    MismatchedArgumentCount(MismatchedArgumentCount),
    MismatchedIndirectArgumentCount(MismatchedIndirectArgumentCount),
    TypeMustBeKnownAtThisPoint(TypeMustBeKnownAtThisPoint),
    ExpectedTupleType(ExpectedTupleType),
    ExpectedPointerType(ExpectedPointerType),
    ExpectedLvalue(ExpectedLvalue),
    ImmutableLvalue(ImmutableLvalue),
    OutOfBoundsTupleIndex(OutOfBoundsTupleIndex),
    DuplicateNameBinding(DuplicateNameBinding),
    ResidualSubtype(ResidualSubtype),
    IncompatibleEffectRows(IncompatibleEffectRows),
    EmbeddedNulString(EmbeddedNulString),
    MissingEffectOperationHandler(MissingEffectOperationHandler),
    ExtraneousEffectOperationHandler(ExtraneousEffectOperationHandler),
    DuplicateEffectOperationHandler(DuplicateEffectOperationHandler),
    MismatchedEffectOperationHandlerParameterCount(MismatchedEffectOperationHandlerParameterCount),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::InstanceResolution(diagnostic) => diagnostic.report(engine).await,
            Self::Resolution(diagnostic) => diagnostic.report(engine).await,
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
            Self::IncompatibleEffectRows(incompatible_effects) => {
                incompatible_effects.report(engine).await
            }
            Self::EmbeddedNulString(string) => string.report(engine).await,
            Self::MissingEffectOperationHandler(handler) => handler.report(engine).await,
            Self::ExtraneousEffectOperationHandler(handler) => handler.report(engine).await,
            Self::DuplicateEffectOperationHandler(handler) => handler.report(engine).await,
            Self::MismatchedEffectOperationHandlerParameterCount(handler) => {
                handler.report(engine).await
            }
        }
    }
}

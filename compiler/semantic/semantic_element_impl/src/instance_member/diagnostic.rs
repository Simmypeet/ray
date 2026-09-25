//! The diagnostics of an instance member that does not conform to its trait
//! member.

use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    name::get_qualified_name, source_map::to_absolute_span, symbol_kind::SymbolKind,
};
use rayc_type::{
    trait_ref::TraitRef,
    ty::{Ty, TyKind},
    where_clause::PredicateKind,
};

/// The particular contract violated by an instance member.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Mismatch {
    ParameterCount {
        expected: usize,
        actual: usize,
    },
    ParameterType {
        index: usize,
        expected: Interned<Ty>,
        actual: Interned<Ty>,
    },
    ReturnType {
        expected: Interned<Ty>,
        actual: Interned<Ty>,
    },
    EffectRow {
        expected: Interned<Ty>,
        actual: Interned<Ty>,
    },
    /// The members have different numbers of type, effect and lifetime
    /// parameters.
    PolyVarCount {
        expected: usize,
        actual: usize,
    },
    /// The members have different numbers of given parameters, counting the
    /// hidden dictionaries of callable parameters, which `callable` reports.
    GivenParameterCount {
        expected: usize,
        actual: usize,
        callable: bool,
    },
    PolyVarKind {
        index: usize,
        expected: TyKind,
        actual: TyKind,
    },
    /// A trait member variable has no counterpart at the same position of the
    /// implementation's signature.
    UnpairedPolyVar {
        name: Interned<str>,
    },
    /// A trait member variable that its signature does not mention cannot
    /// correspond to a variable the implementation discovers.
    ExplicitTypeParametersRequired {
        name: Interned<str>,
    },
    InstanceParameterTraitRef {
        index: usize,
        expected: TraitRef,
        actual: TraitRef,
    },
    MissingWhereClausePredicate {
        expected: PredicateKind,
    },
    ExtraneousWhereClausePredicate {
        actual: PredicateKind,
    },
    MemberKind {
        expected: SymbolKind,
        actual: SymbolKind,
    },
    AssociatedTypeKind {
        expected: TyKind,
        actual: TyKind,
    },
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Diagnostic {
    name: Interned<str>,
    instance_span: RelativeSpan,
    trait_span: RelativeSpan,
    mismatch: Mismatch,
}

const fn kind_name(kind: TyKind) -> &'static str {
    match kind {
        TyKind::Star => "type",
        TyKind::EffectRow => "effect row",
        TyKind::Instance => "instance",
        TyKind::Lifetime => "lifetime",
    }
}

/// Describes a type that differs from the expected one.
async fn expected_found(
    expected: &Interned<Ty>,
    actual: &Interned<Ty>,
    engine: &TrackedEngine,
) -> String {
    format!(
        "expected `{}`, found `{}`",
        expected.display(engine).await,
        actual.display(engine).await
    )
}

async fn display_trait_ref(reference: &TraitRef, engine: &TrackedEngine) -> String {
    let name = engine.get_qualified_name(reference.trait_id()).await;
    let mut args = Vec::new();
    for argument in reference.args().iter() {
        args.push(argument.display(engine).await.to_string());
    }
    format!("{name}[{}]", args.join(", "))
}

async fn display_predicate(predicate: &PredicateKind, engine: &TrackedEngine) -> String {
    match predicate {
        PredicateKind::AssociatedTypeEquality(equality) => format!(
            "`{}` must equal `{}`",
            equality.left().display(engine).await,
            equality.right().display(engine).await
        ),
        PredicateKind::Marker(predicate) => format!(
            "`{}` must satisfy marker `{}`",
            predicate.implementor().display(engine).await,
            engine.get_qualified_name(predicate.marker_id()).await
        ),
    }
}

impl Mismatch {
    /// Returns the message on the trait member's highlight.
    const fn related_message(&self) -> &'static str {
        match self {
            Self::MissingWhereClausePredicate { .. } => {
                "the required trait predicate is declared here"
            }
            Self::ExtraneousWhereClausePredicate { .. } => {
                "the corresponding trait member declares no equivalent requirement"
            }
            Self::UnpairedPolyVar { .. } => "the trait's variable occurs here",
            Self::ExplicitTypeParametersRequired { .. } => {
                "the trait declares this variable, but its signature does not mention it"
            }
            Self::ParameterCount { .. }
            | Self::ParameterType { .. }
            | Self::ReturnType { .. }
            | Self::EffectRow { .. }
            | Self::PolyVarCount { .. }
            | Self::GivenParameterCount { .. }
            | Self::PolyVarKind { .. }
            | Self::InstanceParameterTraitRef { .. }
            | Self::AssociatedTypeKind { .. }
            | Self::MemberKind { .. } => "the corresponding trait declaration is here",
        }
    }

    /// Returns the problem, for the diagnostic's message, and its detail, for
    /// the implementation's highlight.
    async fn describe(&self, engine: &TrackedEngine) -> (String, String) {
        match self {
            Self::ParameterCount { expected, actual } => (
                "parameter count mismatch".to_owned(),
                format!("expected {expected} parameters, found {actual}"),
            ),
            Self::ParameterType { index, expected, actual } => (
                format!("parameter {} type mismatch", index + 1),
                expected_found(expected, actual, engine).await,
            ),
            Self::ReturnType { expected, actual } => {
                ("return type mismatch".to_owned(), expected_found(expected, actual, engine).await)
            }
            Self::EffectRow { expected, actual } => {
                ("effect row mismatch".to_owned(), expected_found(expected, actual, engine).await)
            }
            Self::PolyVarCount { expected, actual } => (
                "polymorphic variable count mismatch".to_owned(),
                format!("expected {expected} polymorphic variables, found {actual}"),
            ),
            Self::GivenParameterCount { expected, actual, .. } => (
                "given parameter count mismatch".to_owned(),
                format!("expected {expected} given parameters, found {actual}"),
            ),
            Self::UnpairedPolyVar { name } => (
                format!("polymorphic variable `{}` has no counterpart", &**name),
                format!(
                    "no variable of this implementation's signature occurs where `{}` occurs in \
                     the trait's",
                    &**name
                ),
            ),
            Self::ExplicitTypeParametersRequired { name } => (
                "type parameters must be declared explicitly".to_owned(),
                format!(
                    "the trait's `{}` cannot be matched with a variable this implementation \
                     introduces from its parameter types",
                    &**name
                ),
            ),
            Self::PolyVarKind { index, expected, actual } => (
                format!("polymorphic variable {} kind mismatch", index + 1),
                format!("expected {}, found {}", kind_name(*expected), kind_name(*actual)),
            ),
            Self::InstanceParameterTraitRef { index, expected, actual } => (
                format!(
                    "instance parameter at polymorphic position {} trait reference mismatch",
                    index + 1
                ),
                format!(
                    "expected `{}`, found `{}`",
                    display_trait_ref(expected, engine).await,
                    display_trait_ref(actual, engine).await
                ),
            ),
            Self::MissingWhereClausePredicate { expected } => (
                "missing where-clause predicate".to_owned(),
                format!("missing requirement: {}", display_predicate(expected, engine).await),
            ),
            Self::ExtraneousWhereClausePredicate { actual } => (
                "extraneous where-clause predicate".to_owned(),
                format!("extra requirement: {}", display_predicate(actual, engine).await),
            ),
            Self::AssociatedTypeKind { expected, actual } => (
                "associated type kind mismatch".to_owned(),
                format!("expected {}, found {}", kind_name(*expected), kind_name(*actual)),
            ),
            Self::MemberKind { expected, actual } => (
                "member kind mismatch".to_owned(),
                format!("expected an implementation of {}, found {}", expected.str(), actual.str()),
            ),
        }
    }

    /// Returns a help message that explains how to fix the mismatch.
    const fn help(&self) -> Option<&'static str> {
        match self {
            Self::GivenParameterCount { callable: true, .. } => Some(
                "a callable parameter such as `def()` takes hidden `core.Def` and `core.Drop` \
                 given parameters",
            ),
            Self::UnpairedPolyVar { .. } => Some(
                "the implementation's variables correspond to the trait's by where they occur in \
                 the signature",
            ),
            Self::ExplicitTypeParametersRequired { .. } => Some(
                "declare this implementation's type parameters, as in `def name[a, b](...)`, to \
                 match them with the trait's in order",
            ),
            Self::ParameterCount { .. }
            | Self::ParameterType { .. }
            | Self::ReturnType { .. }
            | Self::EffectRow { .. }
            | Self::PolyVarCount { .. }
            | Self::GivenParameterCount { callable: false, .. }
            | Self::PolyVarKind { .. }
            | Self::InstanceParameterTraitRef { .. }
            | Self::MissingWhereClausePredicate { .. }
            | Self::ExtraneousWhereClausePredicate { .. }
            | Self::MemberKind { .. }
            | Self::AssociatedTypeKind { .. } => None,
        }
    }
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let (problem, detail) = self.mismatch.describe(engine).await;
        Rendered::builder()
            .message(format!("instance member `{}`: {problem}", &*self.name))
            .maybe_help_message(self.mismatch.help())
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.instance_span).await)
                    .message(detail)
                    .build(),
            )
            .related(vec![
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.trait_span).await)
                    .message(self.mismatch.related_message())
                    .build(),
            ])
            .build()
    }
}

/// Reports the mismatches of one instance member against its trait member.
pub(super) struct Compatibility<'a> {
    name: Interned<str>,
    instance_span: RelativeSpan,
    trait_span: RelativeSpan,
    diagnostics: &'a Storage<Diagnostic>,
}

impl<'a> Compatibility<'a> {
    /// Reports mismatches of the instance member `name`, declared at
    /// `instance_span`, against the trait member declared at `trait_span`.
    pub(super) const fn new(
        name: Interned<str>,
        instance_span: RelativeSpan,
        trait_span: RelativeSpan,
        diagnostics: &'a Storage<Diagnostic>,
    ) -> Self {
        Self { name, instance_span, trait_span, diagnostics }
    }

    /// Returns the span of the instance member's declaration.
    pub(super) const fn instance_span(&self) -> RelativeSpan { self.instance_span }

    /// Returns the span of the trait member's declaration.
    pub(super) const fn trait_span(&self) -> RelativeSpan { self.trait_span }

    /// Reports `mismatch` at the two members' declarations.
    pub(super) fn report(&self, mismatch: Mismatch) {
        self.report_at(mismatch, self.trait_span, self.instance_span);
    }

    /// Reports `mismatch` at the given spans of the two members.
    pub(super) fn report_at(
        &self,
        mismatch: Mismatch,
        trait_span: RelativeSpan,
        instance_span: RelativeSpan,
    ) {
        self.diagnostics.receive(Diagnostic {
            name: self.name.clone(),
            trait_span,
            instance_span,
            mismatch,
        });
    }
}

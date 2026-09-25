//! Resolution of lifetimes, including lifetime elision.

use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_source_file::SourceElement;
use rayc_symbol::source_map::to_absolute_span;
use rayc_syntax::r#type::{Lifetime as LifetimeSyntax, LifetimeName, Reference};
use rayc_type::{
    poly_var::{GlobalPolyVarID, PolyVarOrigin, get_poly_var_map},
    ty::{Ty, TyKind, lifetime::Lifetime},
};

use crate::{Diagnostic, resolver::Resolver};

/// How a resolver treats an elided lifetime: a reference written without a
/// lifetime, or the placeholder lifetime `'_`.
///
/// This follows Rust's elision rules for function signatures.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LifetimeElision {
    /// Elision is not allowed, as in struct fields, where clauses and
    /// instance heads.
    #[default]
    Forbidden,

    /// An elided lifetime is erased. This is used in function bodies, where
    /// type inference ignores lifetimes.
    Erased,

    /// Each elided lifetime is the fresh lifetime parameter of the site that
    /// was introduced for it (see [`PolyVarOrigin::ElidedLifetime`]). This is
    /// used in parameter types.
    FreshParameter,

    /// Every elided lifetime is the one lifetime the parameters mention. This
    /// is used in return types. It is `None` when the parameters mention no
    /// lifetime or more than one, which makes elision an error.
    Output(Option<Interned<Ty>>),

    /// Elision is not allowed because it would need a higher-ranked lifetime,
    /// as in the callable type of a `def(...)` parameter. Ray has no
    /// higher-ranked lifetimes yet, and rejecting elision here keeps it from
    /// silently choosing a weaker meaning.
    HigherRanked,
}

/// A named lifetime that is not declared.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct LifetimeNotFound {
    name: Interned<str>,
    span: RelativeSpan,
}

impl Report for LifetimeNotFound {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!("lifetime `{}` is not declared", &*self.name)),
            ))
            .message(format!("use of undeclared lifetime `{}`", &*self.name))
            .help_message(
                "a `def` introduces the lifetimes its parameter types mention; structs, traits, \
                 effects and instances must declare their lifetimes",
            )
            .build()
    }
}

/// Where an elided lifetime was not allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum MissingLifetimeContext {
    /// A declaration that does not allow elision at all.
    Declaration,
    /// A return type whose parameters do not mention exactly one lifetime.
    ReturnType,
    /// A callable type, where elision would need a higher-ranked lifetime.
    CallableType,
}

/// An elided lifetime where elision is not allowed.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub struct MissingLifetime {
    context: MissingLifetimeContext,
    span: RelativeSpan,
}

impl Report for MissingLifetime {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let help = match self.context {
            MissingLifetimeContext::Declaration => {
                "lifetimes can only be elided in function signatures and bodies"
            }
            MissingLifetimeContext::ReturnType => {
                "an elided lifetime in a return type needs the parameters to mention exactly one \
                 lifetime"
            }
            MissingLifetimeContext::CallableType => {
                "an elided lifetime in a callable type would need a higher-ranked lifetime, which \
                 is not supported yet; name the lifetime instead"
            }
        };
        Rendered::builder()
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some("expected a named lifetime".to_owned()),
            ))
            .message("missing lifetime specifier")
            .help_message(help)
            .build()
    }
}

/// Returns the span that identifies the lifetime elided in `reference`: the
/// span of `'_` when it is written, and the span of the whole reference type
/// otherwise.
#[must_use]
pub fn elided_reference_lifetime_span(reference: &Reference) -> RelativeSpan {
    reference.lifetime().map_or_else(|| reference.span(), |lifetime| lifetime.span())
}

/// Returns the name a lifetime parameter is stored under, including the
/// leading quote.
#[must_use]
pub fn lifetime_parameter_name(identifier: &str) -> String { format!("'{identifier}") }

impl Resolver<'_> {
    /// Resolves a written lifetime.
    pub async fn resolve_lifetime(&mut self, syntax: &LifetimeSyntax) -> Interned<Ty> {
        match syntax.name() {
            Some(LifetimeName::Static(_)) => Ty::new_lifetime(Lifetime::Static, self.engine()),
            Some(LifetimeName::Identifier(_)) if syntax.is_placeholder() => {
                self.elided_lifetime(syntax.span()).await
            }
            Some(LifetimeName::Identifier(identifier)) => {
                let name = lifetime_parameter_name(&identifier.kind.0);
                if let Some(id) = self.search_poly_var(&name) {
                    let ty = self.new_poly_var_type_from_id(id);
                    if self.type_kind(&ty).await == TyKind::Lifetime {
                        return ty;
                    }
                }

                self.report(Diagnostic::LifetimeNotFound(LifetimeNotFound {
                    name: self.engine().intern_unsized(name),
                    span: syntax.span(),
                }));
                Ty::new_error(TyKind::Lifetime, self.engine())
            }
            None => Ty::new_error(TyKind::Lifetime, self.engine()),
        }
    }

    /// Resolves the lifetime elided at `span`, according to this resolver's
    /// [`LifetimeElision`].
    pub(crate) async fn elided_lifetime(&mut self, span: RelativeSpan) -> Interned<Ty> {
        let context = match self.lifetime_elision() {
            LifetimeElision::Erased => return Ty::new_lifetime(Lifetime::Erased, self.engine()),
            LifetimeElision::Output(Some(lifetime)) => return lifetime.clone(),
            LifetimeElision::FreshParameter => {
                let poly_vars = self.engine().get_poly_var_map(self.site()).await;
                if let Some(id) = poly_vars.find_generated(&PolyVarOrigin::ElidedLifetime(span)) {
                    return self.new_poly_var_type_from_id(GlobalPolyVarID::new(self.site(), id));
                }

                // Only parameter types introduce lifetimes for elision.
                MissingLifetimeContext::Declaration
            }
            LifetimeElision::Output(None) => MissingLifetimeContext::ReturnType,
            LifetimeElision::HigherRanked => MissingLifetimeContext::CallableType,
            LifetimeElision::Forbidden => MissingLifetimeContext::Declaration,
        };

        self.report(Diagnostic::MissingLifetime(MissingLifetime { context, span }));
        Ty::new_error(TyKind::Lifetime, self.engine())
    }
}

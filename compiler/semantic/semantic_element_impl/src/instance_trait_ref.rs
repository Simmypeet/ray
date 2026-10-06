use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::{Handler, Storage};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_resolution::resolver::Resolver;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    accessibility::Accessibility,
    core_item::{CoreItem, get_core_item},
    member::{get_member_by_name, get_members},
    name::{get_name, get_qualified_name},
    source_map::to_absolute_span,
    span::get_span,
    syntax::{get_instance_trait_syntax, is_linear_struct},
};
use rayc_type::{
    accessibility::get_accessibility,
    poly_var::get_enclosing_poly_var_maps,
    trait_ref::{InstanceTraitRefKey, TraitRef},
    ty::{Ty, application::View as ApplicationView},
};

use crate::{
    build::{Build, Output},
    register_build,
};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MissingDefinition {
    name: Interned<str>,
    instance_span: RelativeSpan,
    trait_def_span: RelativeSpan,
}

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
enum ReservedDropHeadKind {
    Primitive,
    Pointer,
    Reference,
    Tuple,
}

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
pub struct ReservedDropImplementation {
    span: RelativeSpan,
    kind: ReservedDropHeadKind,
}

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
enum NonNominalDropHeadKind {
    TypeVariable,
    AssociatedType,
    Other,
}

/// A Drop instance whose head is not a struct type, such as `Drop[a]`.
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
pub struct NonNominalDropImplementation {
    span: RelativeSpan,
    kind: NonNominalDropHeadKind,
}

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
pub struct ForeignNominalDropImplementation {
    span: RelativeSpan,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct LinearDropImplementation {
    span: RelativeSpan,
    struct_name: Interned<str>,
    struct_span: Option<RelativeSpan>,
}

impl Report for MissingDefinition {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message(format!("instance is missing trait method `{}`", &*self.name))
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.instance_span).await)
                    .message(format!("implement `{}` in this instance", &*self.name))
                    .build(),
            )
            .related(vec![
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.trait_def_span).await)
                    .message("the trait method is declared here")
                    .build(),
            ])
            .build()
    }
}

/// An instance implementing a trait with a member that is not accessible
/// where the instance is declared, which seals the trait.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct InaccessibleTraitMember {
    name: Interned<str>,
    trait_span: RelativeSpan,
    trait_member_span: RelativeSpan,
    accessibility: Accessibility,
}

impl Report for InaccessibleTraitMember {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let help_message = match self.accessibility {
            Accessibility::Public => None,
            Accessibility::Scoped(scope) => Some(format!(
                "the trait can only be implemented within `{}`, where `{}` is accessible",
                engine.get_qualified_name(scope).await,
                &*self.name
            )),
        };

        Rendered::builder()
            .message(format!(
                "cannot implement the trait here, its member `{}` is not accessible",
                &*self.name
            ))
            .primary_highlight(
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.trait_span).await)
                    .message(format!("`{}` is not accessible here", &*self.name))
                    .build(),
            )
            .related(vec![
                Highlight::builder()
                    .span(engine.to_absolute_span(&self.trait_member_span).await)
                    .message(format!("`{}` is declared here", &*self.name))
                    .build(),
            ])
            .maybe_help_message(help_message)
            .build()
    }
}

impl Report for ReservedDropImplementation {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let type_kind = match self.kind {
            ReservedDropHeadKind::Primitive => "primitive",
            ReservedDropHeadKind::Pointer => "pointer",
            ReservedDropHeadKind::Reference => "reference",
            ReservedDropHeadKind::Tuple => "tuple",
        };

        Rendered::builder()
            .message("reserved Drop implementation")
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!("Drop for {type_kind} types is provided by the compiler")),
            ))
            .build()
    }
}

impl Report for NonNominalDropImplementation {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let found = match self.kind {
            NonNominalDropHeadKind::TypeVariable => "found a type variable",
            NonNominalDropHeadKind::AssociatedType => "found an associated type",
            NonNominalDropHeadKind::Other => "found a type that is not a struct",
        };

        Rendered::builder()
            .message("Drop instance must be declared for a struct type")
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some(format!("expected `Drop[SomeStruct[...]]`, {found}")),
            ))
            .build()
    }
}

impl Report for ForeignNominalDropImplementation {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message("Drop instance must be declared in the nominal type's target")
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some("move this Drop instance to the target that defines the type".into()),
            ))
            .build()
    }
}

impl Report for LinearDropImplementation {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let related = match &self.struct_span {
            Some(span) => Some(vec![Highlight::new(
                engine.to_absolute_span(span).await,
                Some(format!("`{}` is declared `@linear` here", &*self.struct_name)),
            )]),
            None => None,
        };

        Rendered::builder()
            .message(format!(
                "cannot implement Drop for the linear struct `{}`",
                &*self.struct_name
            ))
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some("a linear value must be consumed explicitly instead of dropped".into()),
            ))
            .maybe_related(related)
            .build()
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
    From,
)]
pub enum Diagnostic {
    Resolution(rayc_resolution::Diagnostic),
    MissingDefinition(MissingDefinition),
    InaccessibleTraitMember(InaccessibleTraitMember),
    ReservedDropImplementation(ReservedDropImplementation),
    NonNominalDropImplementation(NonNominalDropImplementation),
    ForeignNominalDropImplementation(ForeignNominalDropImplementation),
    LinearDropImplementation(LinearDropImplementation),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::Resolution(diagnostic) => diagnostic.report(engine).await,
            Self::MissingDefinition(diagnostic) => diagnostic.report(engine).await,
            Self::InaccessibleTraitMember(diagnostic) => diagnostic.report(engine).await,
            Self::ReservedDropImplementation(diagnostic) => diagnostic.report(engine).await,
            Self::NonNominalDropImplementation(diagnostic) => diagnostic.report(engine).await,
            Self::ForeignNominalDropImplementation(diagnostic) => diagnostic.report(engine).await,
            Self::LinearDropImplementation(diagnostic) => diagnostic.report(engine).await,
        }
    }
}

/// How the implementor of a Drop instance head relates to the rule that
/// only struct types may declare Drop instances.
enum DropHead {
    Struct,
    Reserved(ReservedDropHeadKind),
    NonNominal(NonNominalDropHeadKind),
    /// Already reported by type resolution.
    Error,
}

fn classify_drop_head(ty: &Ty) -> DropHead {
    match ty {
        Ty::Application(application) => match application.view() {
            ApplicationView::Struct(_) => DropHead::Struct,
            ApplicationView::Primitive(_) => DropHead::Reserved(ReservedDropHeadKind::Primitive),
            ApplicationView::Pointer(_) => DropHead::Reserved(ReservedDropHeadKind::Pointer),
            ApplicationView::Reference(_) => DropHead::Reserved(ReservedDropHeadKind::Reference),
            ApplicationView::Tuple(_) => DropHead::Reserved(ReservedDropHeadKind::Tuple),
            ApplicationView::InstanceAssociated(_) => {
                DropHead::NonNominal(NonNominalDropHeadKind::AssociatedType)
            }
            ApplicationView::Instance(_)
            | ApplicationView::Closure(_)
            | ApplicationView::DefInstance(_)
            | ApplicationView::NoOpDropInstance(_)
            | ApplicationView::TupleDropInstance(_)
            | ApplicationView::ClosureDropInstance(_)
            | ApplicationView::NominalDropInstance(_) => {
                DropHead::NonNominal(NonNominalDropHeadKind::Other)
            }
            ApplicationView::Error => DropHead::Error,
        },
        Ty::PolyVar(_) => DropHead::NonNominal(NonNominalDropHeadKind::TypeVariable),
        Ty::Inference(_) | Ty::SelfInstance(_) | Ty::EffectRow(_) | Ty::Lifetime(_) => {
            DropHead::NonNominal(NonNominalDropHeadKind::Other)
        }
    }
}

/// Checks that the instance `symbol_id` implements every member of the trait
/// it implements, `trait_ref` written at `trait_span`, and that each of them
/// is accessible where the instance is declared.
async fn check_trait_members(
    engine: &TrackedEngine,
    symbol_id: GlobalSymbolID,
    trait_ref: &TraitRef,
    trait_span: RelativeSpan,
    diagnostics: &Storage<Diagnostic>,
) {
    let instance_span =
        engine.get_span(symbol_id).await.expect("an instance symbol should have a source span");
    let trait_members = engine.get_members(trait_ref.trait_id()).await;

    let mut trait_definitions = Vec::new();
    for trait_def_id in trait_members.namable_members() {
        let trait_def_id = trait_ref.trait_id().target_id.make_global(trait_def_id);
        trait_definitions.push((engine.get_name(trait_def_id).await, trait_def_id));
    }

    for (name, trait_def_id) in trait_definitions {
        let trait_def_span = engine
            .get_span(trait_def_id)
            .await
            .expect("a trait definition symbol should have a source span");

        // Every member of the trait must be accessible where the instance
        // implements it, so a trait with a private member is sealed
        // outside of that member's scope.
        let accessibility = engine.get_accessibility(trait_def_id).await;
        if !accessibility.is_accessible_from(symbol_id, engine).await {
            diagnostics.receive(Diagnostic::InaccessibleTraitMember(InaccessibleTraitMember {
                name: name.clone(),
                trait_span,
                trait_member_span: trait_def_span,
                accessibility,
            }));
        }

        if engine.get_member_by_name(symbol_id, &name).await.is_none() {
            diagnostics.receive(Diagnostic::MissingDefinition(MissingDefinition {
                name,
                instance_span,
                trait_def_span,
            }));
        }
    }
}

impl Build for InstanceTraitRefKey {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { symbol_id }: &Self) -> Output<Self> {
        let diagnostics = Storage::new();
        let obligations = Storage::new();
        let Some(syntax) = engine.get_instance_trait_syntax(symbol_id).await else {
            return Output::new_with(None, diagnostics.into_vec(), obligations.into_vec(), engine);
        };
        let poly_vars = engine.get_enclosing_poly_var_maps(symbol_id).await;

        let mut resolver = Resolver::builder()
            .engine(engine)
            .poly_var_stack(&poly_vars)
            .site(symbol_id)
            .handler(&diagnostics)
            .obligation_handler(&obligations)
            .build();
        let Ok(trait_ref) = resolver.resolve_trait_path(&syntax).await else {
            return Output::new_with(None, diagnostics.into_vec(), obligations.into_vec(), engine);
        };

        // Drop instances may only be declared for struct types. Structural
        // types get their Drop from the compiler, and an opaque head such as
        // `Drop[a]` would compete with those built-in and generated
        // dictionaries.
        if trait_ref.trait_id() == engine.get_core_item(CoreItem::DropTrait).await
            && let Some(implementor) = trait_ref.args().interned_iter().next()
        {
            let diagnostic =
                match classify_drop_head(implementor) {
                    DropHead::Struct | DropHead::Error => None,
                    DropHead::Reserved(kind) => {
                        Some(Diagnostic::ReservedDropImplementation(ReservedDropImplementation {
                            span: syntax.span(),
                            kind,
                        }))
                    }
                    DropHead::NonNominal(kind) => Some(Diagnostic::NonNominalDropImplementation(
                        NonNominalDropImplementation { span: syntax.span(), kind },
                    )),
                };
            if let Some(diagnostic) = diagnostic {
                diagnostics.receive(diagnostic);
                return Output::new_with(
                    None,
                    diagnostics.into_vec(),
                    obligations.into_vec(),
                    engine,
                );
            }

            // A struct's Drop instance belongs to the target defining it, so
            // that target's Drop plans see every explicit instance.
            if let Some(struct_) = implementor.as_struct_view()
                && struct_.symbol_id().target_id != symbol_id.target_id
            {
                diagnostics.receive(Diagnostic::ForeignNominalDropImplementation(
                    ForeignNominalDropImplementation { span: syntax.span() },
                ));
                return Output::new_with(
                    None,
                    diagnostics.into_vec(),
                    obligations.into_vec(),
                    engine,
                );
            }

            // A linear struct has no Drop instance by definition; its values
            // must be consumed explicitly.
            if let Some(struct_) = implementor.as_struct_view()
                && engine.is_linear_struct(struct_.symbol_id()).await
            {
                diagnostics.receive(Diagnostic::LinearDropImplementation(
                    LinearDropImplementation {
                        span: syntax.span(),
                        struct_name: engine.get_name(struct_.symbol_id()).await,
                        struct_span: engine.get_span(struct_.symbol_id()).await,
                    },
                ));
                return Output::new_with(
                    None,
                    diagnostics.into_vec(),
                    obligations.into_vec(),
                    engine,
                );
            }
        }

        check_trait_members(engine, symbol_id, &trait_ref, syntax.span(), &diagnostics).await;

        Output::new_with(Some(trait_ref), diagnostics.into_vec(), obligations.into_vec(), engine)
    }
}

register_build!(InstanceTraitRefKey);

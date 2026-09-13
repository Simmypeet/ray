//! Semantic path-resolution results.

use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_source_file::SourceElement;
use rayc_symbol::{GlobalSymbolID, symbol_kind::SymbolKind};
use rayc_syntax::path::{Path, PathRoot, PathSegment};
use rayc_type::{
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    subst::Subst,
    trait_ref::TraitRef,
    ty::{Ty, application::View as ApplicationView, args::Args, self_instance::SelfInstance},
};

use crate::{WfCheck, resolver::Resolver};

/// The semantic result of resolving a path.
#[derive(Debug, Clone)]
pub enum PathResolution {
    /// A source definition.
    Def(Def),
    /// An external definition.
    ExternDef(ExternDef),
    /// A module.
    Module(Module),
    /// An effect.
    Effect(Effect),
    /// A trait.
    Trait(TraitRef),
    /// A marker.
    Marker(GlobalSymbolID),
    /// A trait instance.
    Instance(Instance),
    /// A polymorphic type or instance parameter.
    PolyVar(GlobalPolyVarID),
    /// The enclosing trait dictionary.
    SelfInstance(SelfInstance),
    /// A trait member selected through a named trait or its self dictionary.
    TraitMember(TraitMember),
    /// A method or associated type selected through a concrete trait instance.
    ResolvedInstanceMember(ResolvedInstanceMember),
    /// A method or associated type selected through an instance polymorphic
    /// variable.
    UnresolvedInstanceMember(UnresolvedInstanceMember),
    /// An operation belonging to an effect.
    EffectOperation(EffectOperation),
}

/// A resolved global trait instance and its arguments.
#[derive(Debug, Clone)]
pub struct Instance {
    symbol_id: GlobalSymbolID,
    args: Args,
}

impl Instance {
    const fn new(symbol_id: GlobalSymbolID, args: Args) -> Self { Self { symbol_id, args } }

    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }

    #[must_use]
    pub const fn args(&self) -> &Args { &self.args }
}

/// A trait member selected through a named trait or its self dictionary.
#[derive(Debug, Clone)]
pub enum TraitMemberParent {
    /// Trait identity without a dictionary.
    Named(TraitRef),
    /// The rigid dictionary supplied by the enclosing trait body.
    This(SelfInstance),
}

/// A method or associated type retaining its parent provenance.
#[derive(Debug, Clone)]
pub struct TraitMember {
    parent: TraitMemberParent,
    kind: SymbolKind,
    symbol_id: GlobalSymbolID,
    args: Args,
}

impl TraitMember {
    const fn new(
        parent: TraitMemberParent,
        kind: SymbolKind,
        symbol_id: GlobalSymbolID,
        args: Args,
    ) -> Self {
        Self { parent, kind, symbol_id, args }
    }

    #[must_use]
    pub const fn parent(&self) -> &TraitMemberParent { &self.parent }

    #[must_use]
    pub const fn args(&self) -> &Args { &self.args }

    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }

    /// Builds the substitution for the parent trait and selected definition.
    pub async fn substitution(&self, engine: &rayc_qbice::TrackedEngine) -> Subst {
        let trait_ref = match &self.parent {
            TraitMemberParent::Named(reference) => reference.clone(),
            TraitMemberParent::This(instance) => instance.trait_ref(engine).await,
        };
        let mut subst = substitution(trait_ref.trait_id(), trait_ref.args(), engine).await;
        subst.compose(&substitution(self.symbol_id, &self.args, engine).await, engine);
        subst
    }
}

/// A method or associated type selected through a concrete trait instance.
#[derive(Debug, Clone)]
pub struct ResolvedInstanceMember {
    kind: SymbolKind,
    instance: Interned<Ty>,
    symbol_id: GlobalSymbolID,
    args: Args,
}

impl ResolvedInstanceMember {
    const fn new(
        instance: Interned<Ty>,
        kind: SymbolKind,
        symbol_id: GlobalSymbolID,
        args: Args,
    ) -> Self {
        Self { kind, instance, symbol_id, args }
    }

    #[must_use]
    pub const fn args(&self) -> &Args { &self.args }

    /// Returns the parent instance type.
    ///
    /// This type is guaranteed to be a concrete instance application.
    #[must_use]
    pub const fn instance(&self) -> &Interned<Ty> { &self.instance }

    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }

    /// Builds the substitution for the parent instance and selected definition.
    pub async fn substitution(&self, engine: &rayc_qbice::TrackedEngine) -> Subst {
        let Ty::Application(application) = &*self.instance else {
            unreachable!("a resolved instance definition should have a concrete instance")
        };
        let ApplicationView::Instance(instance) = application.view() else {
            unreachable!("a resolved instance definition should have instance kind")
        };
        let args = Args::new(instance.args().iter().cloned(), engine);
        let mut subst = substitution(instance.symbol_id(), &args, engine).await;
        subst.compose(&substitution(self.symbol_id, &self.args, engine).await, engine);
        subst
    }
}

/// A method or associated type selected through an instance polymorphic
/// variable.
#[derive(Debug, Clone)]
pub struct UnresolvedInstanceMember {
    kind: SymbolKind,
    trait_ref: TraitRef,
    instance: GlobalPolyVarID,
    trait_member_id: GlobalSymbolID,
    args: Args,
}

impl UnresolvedInstanceMember {
    const fn new(
        instance: GlobalPolyVarID,
        trait_ref: TraitRef,
        kind: SymbolKind,
        trait_member_id: GlobalSymbolID,
        args: Args,
    ) -> Self {
        Self { kind, trait_ref, instance, trait_member_id, args }
    }

    #[must_use]
    pub const fn args(&self) -> &Args { &self.args }

    /// Returns the unresolved parent instance polymorphic variable.
    ///
    /// This variable is guaranteed to have instance kind.
    #[must_use]
    pub const fn instance(&self) -> GlobalPolyVarID { self.instance }

    /// Returns the selected symbol ID, which is guaranteed to identify a
    /// [`SymbolKind::TraitDef`] or [`SymbolKind::TraitType`].
    #[must_use]
    pub const fn trait_member_id(&self) -> GlobalSymbolID { self.trait_member_id }

    /// Builds the substitution for the unresolved parent trait and selected
    /// definition.
    pub async fn substitution(&self, engine: &rayc_qbice::TrackedEngine) -> Subst {
        let trait_ref = &self.trait_ref;
        let mut subst = substitution(trait_ref.trait_id(), trait_ref.args(), engine).await;
        subst.insert(
            SelfInstance::new(trait_ref.trait_id()),
            Ty::new_poly_var(self.instance, engine),
        );
        subst.compose(&substitution(self.trait_member_id, &self.args, engine).await, engine);
        subst
    }
}

/// A resolved source definition.
#[derive(Debug, Clone)]
pub struct Def {
    symbol_id: GlobalSymbolID,
    args: Args,
}

impl Def {
    const fn new(symbol_id: GlobalSymbolID, args: Args) -> Self { Self { symbol_id, args } }

    /// Returns the definition's symbol ID.
    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }

    /// Returns the definition's inferred type arguments.
    #[must_use]
    pub const fn args(&self) -> &Args { &self.args }

    /// Builds the definition's polymorphic substitution.
    pub async fn substitution(&self, engine: &rayc_qbice::TrackedEngine) -> Subst {
        substitution(self.symbol_id, &self.args, engine).await
    }
}

/// A resolved external definition.
#[derive(Debug, Clone, Copy)]
pub struct ExternDef {
    symbol_id: GlobalSymbolID,
}

impl ExternDef {
    const fn new(symbol_id: GlobalSymbolID) -> Self { Self { symbol_id } }

    /// Returns the external definition's symbol ID.
    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }
}

/// A resolved module.
#[derive(Debug, Clone, Copy)]
pub struct Module {
    symbol_id: GlobalSymbolID,
}

impl Module {
    const fn new(symbol_id: GlobalSymbolID) -> Self { Self { symbol_id } }

    /// Returns the module's symbol ID.
    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }
}

/// A resolved effect and its type arguments.
#[derive(Debug, Clone)]
pub struct Effect {
    symbol_id: GlobalSymbolID,
    args: Args,
}

impl Effect {
    const fn new(symbol_id: GlobalSymbolID, args: Args) -> Self { Self { symbol_id, args } }

    /// Returns the effect's symbol ID.
    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }

    /// Returns the effect's resolved type arguments.
    #[must_use]
    pub const fn args(&self) -> &Args { &self.args }

    /// Builds the effect's polymorphic substitution.
    pub async fn substitution(&self, engine: &rayc_qbice::TrackedEngine) -> Subst {
        substitution(self.symbol_id, &self.args, engine).await
    }
}

/// A resolved effect operation and its resolved parent effect.
#[derive(Debug, Clone)]
pub struct EffectOperation {
    effect: Effect,
    symbol_id: GlobalSymbolID,
}

impl EffectOperation {
    const fn new(effect: Effect, symbol_id: GlobalSymbolID) -> Self { Self { effect, symbol_id } }

    /// Returns the resolved parent effect.
    #[must_use]
    pub const fn effect(&self) -> &Effect { &self.effect }

    /// Returns the operation's symbol ID.
    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }

    /// Builds the substitution inherited from the parent effect.
    pub async fn substitution(&self, engine: &rayc_qbice::TrackedEngine) -> Subst {
        self.effect.substitution(engine).await
    }
}

/// The reason a path segment could not be resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PathResolutionError {
    /// The path segment has no identifier due to invalid syntax.
    MissingIdentifier,
    /// The identifier does not name a symbol in the searched scope.
    SymbolNotFound,
    /// The resolved symbol has a different kind than the path context requires.
    UnexpectedSymbolKind,
}

impl PathResolution {
    /// Builds the substitution for this path, including its enclosing trait,
    /// instance, or effect where applicable.
    pub async fn substitution(&self, engine: &rayc_qbice::TrackedEngine) -> Subst {
        match self {
            Self::Def(def) => def.substitution(engine).await,
            Self::ExternDef(_) | Self::Marker(_) | Self::Module(_) | Self::SelfInstance(_) => {
                Subst::new_empty()
            }
            Self::Effect(effect) => effect.substitution(engine).await,
            Self::Trait(trait_ref) => {
                substitution(trait_ref.trait_id(), trait_ref.args(), engine).await
            }
            Self::Instance(instance) => {
                substitution(instance.symbol_id(), instance.args(), engine).await
            }
            Self::PolyVar(id) => {
                let poly_vars = engine.get_poly_var_map(id.parent_id()).await;
                if let Some(trait_ref) = poly_vars.trait_ref_of(id.id()) {
                    substitution(trait_ref.trait_id(), trait_ref.args(), engine).await
                } else {
                    Subst::new_empty()
                }
            }
            Self::TraitMember(def) => def.substitution(engine).await,
            Self::ResolvedInstanceMember(def) => def.substitution(engine).await,
            Self::UnresolvedInstanceMember(def) => def.substitution(engine).await,
            Self::EffectOperation(operation) => operation.substitution(engine).await,
        }
    }

    /// Returns the kind of the resolved symbol.
    #[must_use]
    pub const fn symbol_kind(&self) -> Option<SymbolKind> {
        match self {
            Self::Def(_) => Some(SymbolKind::Def),
            Self::ExternDef(_) => Some(SymbolKind::ExternDef),
            Self::Module(_) => Some(SymbolKind::Module),
            Self::Effect(_) => Some(SymbolKind::Effect),
            Self::Trait(_) => Some(SymbolKind::Trait),
            Self::Marker(_) => Some(SymbolKind::Marker),
            Self::Instance(_) => Some(SymbolKind::Instance),
            Self::PolyVar(_) | Self::SelfInstance(_) => None,
            Self::TraitMember(member) => Some(member.kind),
            Self::UnresolvedInstanceMember(member) => Some(member.kind),
            Self::ResolvedInstanceMember(member) => Some(member.kind),
            Self::EffectOperation(_) => Some(SymbolKind::EffectOperation),
        }
    }

    /// Returns the global ID of the resolved symbol, if it has one.
    #[must_use]
    pub const fn global_id(&self) -> Option<GlobalSymbolID> {
        match self {
            Self::Def(def) => Some(def.symbol_id()),
            Self::ExternDef(def) => Some(def.symbol_id()),
            Self::Module(module) => Some(module.symbol_id()),
            Self::Effect(effect) => Some(effect.symbol_id()),
            Self::Trait(trait_ref) => Some(trait_ref.trait_id()),
            Self::Marker(marker_id) => Some(*marker_id),
            Self::Instance(instance) => Some(instance.symbol_id()),
            Self::PolyVar(_) | Self::SelfInstance(_) => None,
            Self::TraitMember(def) => Some(def.symbol_id()),
            Self::ResolvedInstanceMember(def) => Some(def.symbol_id()),
            Self::UnresolvedInstanceMember(def) => Some(def.trait_member_id()),
            Self::EffectOperation(operation) => Some(operation.symbol_id()),
        }
    }
}

async fn substitution(
    symbol_id: GlobalSymbolID,
    args: &Args,
    engine: &rayc_qbice::TrackedEngine,
) -> Subst {
    if args.is_empty() {
        return Subst::new_empty();
    }

    let poly_vars = engine.get_poly_var_map(symbol_id).await;
    poly_vars
        .iter()
        .zip(args.interned_iter())
        .map(|((poly_var_id, _), argument)| {
            (GlobalPolyVarID::new(symbol_id, poly_var_id), argument.clone())
        })
        .collect()
}

impl Resolver<'_> {
    /// Resolves a path and requires its final symbol to be a trait.
    pub async fn resolve_trait_path(
        &mut self,
        path: &Path,
    ) -> Result<TraitRef, PathResolutionError> {
        let resolution = self.resolve_path(path).await?;

        match resolution {
            PathResolution::Trait(trait_ref) => Ok(trait_ref),
            resolution => {
                if let Some(actual) = resolution.symbol_kind() {
                    self.report_expected_trait(path.span(), actual);
                }
                Err(PathResolutionError::UnexpectedSymbolKind)
            }
        }
    }

    /// Resolves a path and requires its final symbol to be a marker.
    pub async fn resolve_marker_path(
        &mut self,
        path: &Path,
    ) -> Result<GlobalSymbolID, PathResolutionError> {
        let resolution = self.resolve_path(path).await?;

        match resolution {
            PathResolution::Marker(marker_id) => Ok(marker_id),
            resolution => {
                if let Some(actual) = resolution.symbol_kind() {
                    self.report_expected_marker(path.span(), actual);
                }
                Err(PathResolutionError::UnexpectedSymbolKind)
            }
        }
    }

    /// Resolves a path and requires its final symbol to be an effect.
    pub async fn resolve_effect_path(
        &mut self,
        path: &Path,
    ) -> Result<Effect, PathResolutionError> {
        let resolution = self.resolve_path(path).await?;

        match resolution {
            PathResolution::Effect(effect) => Ok(effect),
            resolution => {
                if let Some(actual) = resolution.symbol_kind() {
                    self.report_expected_effect(path.span(), actual);
                }
                Err(PathResolutionError::UnexpectedSymbolKind)
            }
        }
    }

    /// Resolves a path to a strongly typed semantic result.
    pub async fn resolve_path(
        &mut self,
        path: &Path,
    ) -> Result<PathResolution, PathResolutionError> {
        let mut resolution = match path.root() {
            Some(PathRoot::Segment(first)) => self.resolve_path_segment(&first, None).await?,
            Some(PathRoot::This(keyword)) => {
                let Some(instance) = self.self_instance().await else {
                    self.report_invalid_this_path(keyword.span());
                    return Err(PathResolutionError::UnexpectedSymbolKind);
                };
                PathResolution::SelfInstance(instance)
            }
            None => return Err(PathResolutionError::MissingIdentifier),
        };
        for part in path.rest() {
            let Some(segment) = part.segment() else { continue };
            resolution = self.resolve_path_segment(&segment, Some(resolution)).await?;
        }

        Ok(resolution)
    }

    async fn resolve_path_segment(
        &mut self,
        path: &PathSegment,
        previous: Option<PathResolution>,
    ) -> Result<PathResolution, PathResolutionError> {
        let Some(identifier) = path.identifier() else {
            return Err(PathResolutionError::MissingIdentifier);
        };

        if previous.is_none()
            && let Some(poly_var_id) = self.search_poly_var(&identifier.kind.0)
        {
            if path.arguments().is_some() {
                self.report_explicit_type_arguments_not_allowed(path.span());
            }
            return Ok(PathResolution::PolyVar(poly_var_id));
        }

        // Resolve instance parameters using the in-progress map when constructing a
        // declaration, rather than querying that declaration recursively.
        let poly_var_trait = if let Some(PathResolution::PolyVar(id)) = previous.as_ref() {
            let Some(reference) = self.poly_var_trait_ref(*id).await else {
                self.report_type_kind_mismatch(
                    path.span(),
                    rayc_type::ty::TyKind::Instance,
                    rayc_type::ty::TyKind::Star,
                );
                return Err(PathResolutionError::UnexpectedSymbolKind);
            };
            Some(PathResolution::Trait(reference))
        } else if let Some(PathResolution::SelfInstance(instance)) = previous.as_ref() {
            Some(PathResolution::Trait(instance.trait_ref(self.engine()).await))
        } else {
            None
        };
        let parent = poly_var_trait.as_ref().or(previous.as_ref());
        let previous_parent = match parent {
            Some(resolution) => {
                Some(resolution.global_id().ok_or(PathResolutionError::UnexpectedSymbolKind)?)
            }
            None => None,
        };
        let symbol_id = self.find_path_symbol(previous_parent, &identifier.kind.0).await;

        let Some(symbol_id) = symbol_id else {
            self.report_path_segment_not_found(identifier);
            return Err(PathResolutionError::SymbolNotFound);
        };

        let symbol_kind = self.symbol_kind(symbol_id).await;
        let parameters = self.argument_parameters(symbol_id).await;
        // Member requirements may refer to their parent's polymorphic variables.
        let mut inherited = match parent {
            Some(parent) => parent.substitution(self.engine()).await,
            None => Subst::new_empty(),
        };
        if let (Some(PathResolution::PolyVar(id)), Some(PathResolution::Trait(reference))) =
            (&previous, &poly_var_trait)
        {
            inherited.insert(
                SelfInstance::new(reference.trait_id()),
                Ty::new_poly_var(*id, self.engine()),
            );
        }
        let args = self
            .resolve_arguments(symbol_id, path, &identifier, parameters.as_deref(), inherited)
            .await;

        let resolution = self.resolve_member(previous, symbol_kind, symbol_id, args).await?;
        self.emit_predicate_obligations(&resolution, symbol_kind, symbol_id, path.span()).await;
        Ok(resolution)
    }

    async fn emit_predicate_obligations(
        &self,
        resolution: &PathResolution,
        symbol_kind: SymbolKind,
        symbol_id: GlobalSymbolID,
        span: RelativeSpan,
    ) {
        if !symbol_kind.has_where_clause() {
            return;
        }

        // Defer querying the clause so resolving a declaration can require its own
        // instantiated contract without recursively querying the clause being built.
        let subst = resolution.substitution(self.engine()).await;
        self.require_wf_check(WfCheck::new(symbol_id, subst, span));
    }

    async fn resolve_member(
        &self,
        previous: Option<PathResolution>,
        symbol_kind: SymbolKind,
        symbol_id: GlobalSymbolID,
        args: Args,
    ) -> Result<PathResolution, PathResolutionError> {
        match symbol_kind {
            SymbolKind::Def => Ok(PathResolution::Def(Def::new(symbol_id, args))),
            SymbolKind::ExternDef => Ok(PathResolution::ExternDef(ExternDef::new(symbol_id))),
            SymbolKind::Module => Ok(PathResolution::Module(Module::new(symbol_id))),
            SymbolKind::Effect => Ok(PathResolution::Effect(Effect::new(symbol_id, args))),
            SymbolKind::Trait => Ok(PathResolution::Trait(TraitRef::new(symbol_id, args))),
            SymbolKind::Instance => Ok(PathResolution::Instance(Instance::new(symbol_id, args))),
            SymbolKind::Marker => Ok(PathResolution::Marker(symbol_id)),
            SymbolKind::MarkerImplementation => Err(PathResolutionError::UnexpectedSymbolKind),
            SymbolKind::EffectOperation => {
                let Some(PathResolution::Effect(effect)) = previous else {
                    unreachable!("an effect operation should be resolved through its parent effect")
                };
                Ok(PathResolution::EffectOperation(EffectOperation::new(effect, symbol_id)))
            }
            SymbolKind::InstanceDef | SymbolKind::InstanceType => {
                let Some(PathResolution::Instance(instance)) = previous else {
                    return Err(PathResolutionError::UnexpectedSymbolKind);
                };
                let instance =
                    self.new_instance_type(instance.symbol_id(), instance.args().clone());
                Ok(PathResolution::ResolvedInstanceMember(ResolvedInstanceMember::new(
                    instance,
                    symbol_kind,
                    symbol_id,
                    args,
                )))
            }
            SymbolKind::TraitDef | SymbolKind::TraitType => match previous {
                Some(PathResolution::Trait(trait_ref)) => {
                    Ok(PathResolution::TraitMember(TraitMember::new(
                        TraitMemberParent::Named(trait_ref),
                        symbol_kind,
                        symbol_id,
                        args,
                    )))
                }
                Some(PathResolution::SelfInstance(instance)) => {
                    Ok(PathResolution::TraitMember(TraitMember::new(
                        TraitMemberParent::This(instance),
                        symbol_kind,
                        symbol_id,
                        args,
                    )))
                }
                Some(PathResolution::PolyVar(poly_var_id)) => {
                    Ok(PathResolution::UnresolvedInstanceMember(UnresolvedInstanceMember::new(
                        poly_var_id,
                        self.poly_var_trait_ref(poly_var_id)
                            .await
                            .expect("resolved dictionary trait"),
                        symbol_kind,
                        symbol_id,
                        args,
                    )))
                }
                Some(
                    PathResolution::Def(_)
                    | PathResolution::ExternDef(_)
                    | PathResolution::Module(_)
                    | PathResolution::Effect(_)
                    | PathResolution::Instance(_)
                    | PathResolution::Marker(_)
                    | PathResolution::TraitMember(_)
                    | PathResolution::ResolvedInstanceMember(_)
                    | PathResolution::UnresolvedInstanceMember(_)
                    | PathResolution::EffectOperation(_),
                )
                | None => Err(PathResolutionError::UnexpectedSymbolKind),
            },
        }
    }
}

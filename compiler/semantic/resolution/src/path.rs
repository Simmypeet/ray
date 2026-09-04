//! Semantic path-resolution results.

use qbice::storage::intern::Interned;
use rayc_source_file::SourceElement;
use rayc_symbol::{GlobalSymbolID, symbol_kind::SymbolKind};
use rayc_syntax::path::{Path, PathSegment};
use rayc_type::{
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    subst::Subst,
    trait_ref::TraitRef,
    ty::{Ty, application::View as ApplicationView, args::Args},
};

use crate::resolver::Resolver;

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
    /// A trait instance.
    Instance(Instance),
    /// A polymorphic type or instance parameter.
    PolyVar(GlobalPolyVarID),
    /// A definition selected directly through its parent trait.
    TraitDef(TraitDef),
    /// A definition selected through a concrete trait instance.
    ResolvedInstanceDef(ResolvedInstanceDef),
    /// A trait definition selected through an instance polymorphic variable.
    UnsolvedInstanceDef(UnsolvedInstanceDef),
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

/// A definition selected directly through its parent trait.
#[derive(Debug, Clone)]
pub struct TraitDef {
    trait_ref: TraitRef,
    symbol_id: GlobalSymbolID,
}

impl TraitDef {
    const fn new(trait_ref: TraitRef, symbol_id: GlobalSymbolID) -> Self {
        Self { trait_ref, symbol_id }
    }

    #[must_use]
    pub const fn trait_ref(&self) -> &TraitRef { &self.trait_ref }

    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }

    /// Builds the substitution inherited from the parent trait.
    pub async fn substitution(&self, engine: &rayc_qbice::TrackedEngine) -> Subst {
        substitution(self.trait_ref.trait_id(), self.trait_ref.args(), engine).await
    }
}

/// A definition selected through a concrete trait instance.
#[derive(Debug, Clone)]
pub struct ResolvedInstanceDef {
    instance: Interned<Ty>,
    symbol_id: GlobalSymbolID,
}

impl ResolvedInstanceDef {
    const fn new(instance: Interned<Ty>, symbol_id: GlobalSymbolID) -> Self {
        Self { instance, symbol_id }
    }

    /// Returns the parent instance type.
    ///
    /// This type is guaranteed to be a concrete instance application.
    #[must_use]
    pub const fn instance(&self) -> &Interned<Ty> { &self.instance }

    #[must_use]
    pub const fn symbol_id(&self) -> GlobalSymbolID { self.symbol_id }

    /// Builds the substitution inherited from the parent instance.
    pub async fn substitution(&self, engine: &rayc_qbice::TrackedEngine) -> Subst {
        let Ty::Application(application) = &*self.instance else {
            unreachable!("a resolved instance definition should have a concrete instance")
        };
        let ApplicationView::Instance(instance) = application.view() else {
            unreachable!("a resolved instance definition should have instance kind")
        };
        let args = Args::new(instance.args().iter().cloned(), engine);
        substitution(instance.symbol_id(), &args, engine).await
    }
}

/// A trait definition selected through an instance polymorphic variable.
#[derive(Debug, Clone, Copy)]
pub struct UnsolvedInstanceDef {
    instance: GlobalPolyVarID,
    trait_def_id: GlobalSymbolID,
}

impl UnsolvedInstanceDef {
    const fn new(instance: GlobalPolyVarID, trait_def_id: GlobalSymbolID) -> Self {
        Self { instance, trait_def_id }
    }

    /// Returns the unresolved parent instance polymorphic variable.
    ///
    /// This variable is guaranteed to have instance kind.
    #[must_use]
    pub const fn instance(&self) -> GlobalPolyVarID { self.instance }

    /// Returns the selected symbol ID, which is guaranteed to identify a
    /// [`SymbolKind::TraitDef`].
    #[must_use]
    pub const fn trait_def_id(&self) -> GlobalSymbolID { self.trait_def_id }

    /// Builds the substitution inherited from the unresolved parent instance.
    pub async fn substitution(&self, engine: &rayc_qbice::TrackedEngine) -> Subst {
        let poly_vars = engine.get_poly_var_map(self.instance.parent_id()).await;
        let trait_ref = poly_vars
            .trait_ref_of(self.instance.id())
            .expect("an unsolved instance definition should have instance kind");
        substitution(trait_ref.trait_id(), trait_ref.args(), engine).await
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
    /// Returns the kind of the resolved symbol.
    #[must_use]
    pub const fn symbol_kind(&self) -> Option<SymbolKind> {
        match self {
            Self::Def(_) => Some(SymbolKind::Def),
            Self::ExternDef(_) => Some(SymbolKind::ExternDef),
            Self::Module(_) => Some(SymbolKind::Module),
            Self::Effect(_) => Some(SymbolKind::Effect),
            Self::Trait(_) => Some(SymbolKind::Trait),
            Self::Instance(_) => Some(SymbolKind::Instance),
            Self::PolyVar(_) => None,
            Self::TraitDef(_) | Self::UnsolvedInstanceDef(_) => Some(SymbolKind::TraitDef),
            Self::ResolvedInstanceDef(_) => Some(SymbolKind::InstanceDef),
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
            Self::Instance(instance) => Some(instance.symbol_id()),
            Self::PolyVar(_) => None,
            Self::TraitDef(def) => Some(def.symbol_id()),
            Self::ResolvedInstanceDef(def) => Some(def.symbol_id()),
            Self::UnsolvedInstanceDef(def) => Some(def.trait_def_id()),
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
        let mut segments = path.segments();
        let Some(first) = segments.next() else {
            return Err(PathResolutionError::MissingIdentifier);
        };
        let mut resolution = self.resolve_path_segment(&first, None).await?;

        for segment in segments {
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
            && let Some(poly_var_id) = self.poly_var(&identifier.kind.0)
        {
            return Ok(PathResolution::PolyVar(poly_var_id));
        }

        let previous_parent = match previous.as_ref() {
            Some(PathResolution::PolyVar(poly_var_id)) => Some(
                self.poly_var_trait_ref(*poly_var_id)
                    .map(TraitRef::trait_id)
                    .ok_or(PathResolutionError::UnexpectedSymbolKind)?,
            ),
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
        let args = self.resolve_arguments(symbol_kind, path, &identifier, &parameters).await;

        match symbol_kind {
            SymbolKind::Def => Ok(PathResolution::Def(Def::new(symbol_id, args))),
            SymbolKind::ExternDef => Ok(PathResolution::ExternDef(ExternDef::new(symbol_id))),
            SymbolKind::Module => Ok(PathResolution::Module(Module::new(symbol_id))),
            SymbolKind::Effect => Ok(PathResolution::Effect(Effect::new(symbol_id, args))),
            SymbolKind::Trait => Ok(PathResolution::Trait(TraitRef::new(symbol_id, args))),
            SymbolKind::Instance => Ok(PathResolution::Instance(Instance::new(symbol_id, args))),
            SymbolKind::EffectOperation => {
                let Some(PathResolution::Effect(effect)) = previous else {
                    unreachable!("an effect operation should be resolved through its parent effect")
                };
                Ok(PathResolution::EffectOperation(EffectOperation::new(effect, symbol_id)))
            }
            SymbolKind::InstanceDef => {
                let Some(PathResolution::Instance(instance)) = previous else {
                    return Err(PathResolutionError::UnexpectedSymbolKind);
                };
                let instance =
                    self.new_instance_type(instance.symbol_id(), instance.args().clone());
                Ok(PathResolution::ResolvedInstanceDef(ResolvedInstanceDef::new(
                    instance, symbol_id,
                )))
            }
            SymbolKind::TraitDef => match previous {
                Some(PathResolution::Trait(trait_ref)) => {
                    Ok(PathResolution::TraitDef(TraitDef::new(trait_ref, symbol_id)))
                }
                Some(PathResolution::PolyVar(poly_var_id)) => {
                    Ok(PathResolution::UnsolvedInstanceDef(UnsolvedInstanceDef::new(
                        poly_var_id,
                        symbol_id,
                    )))
                }
                Some(
                    PathResolution::Def(_)
                    | PathResolution::ExternDef(_)
                    | PathResolution::Module(_)
                    | PathResolution::Effect(_)
                    | PathResolution::Instance(_)
                    | PathResolution::TraitDef(_)
                    | PathResolution::ResolvedInstanceDef(_)
                    | PathResolution::UnsolvedInstanceDef(_)
                    | PathResolution::EffectOperation(_),
                )
                | None => Err(PathResolutionError::UnexpectedSymbolKind),
            },
        }
    }
}

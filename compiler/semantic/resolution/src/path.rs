//! Semantic path-resolution results.

use rayc_source_file::SourceElement;
use rayc_symbol::{GlobalSymbolID, symbol_kind::SymbolKind};
use rayc_syntax::path::{Path, PathSegment};
use rayc_type::{
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    subst::Subst,
    ty::args::Args,
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
    /// An operation belonging to an effect.
    EffectOperation(EffectOperation),
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
    const fn symbol_id(&self) -> GlobalSymbolID {
        match self {
            Self::Def(def) => def.symbol_id(),
            Self::ExternDef(def) => def.symbol_id(),
            Self::Module(module) => module.symbol_id(),
            Self::Effect(effect) => effect.symbol_id(),
            Self::EffectOperation(operation) => operation.symbol_id(),
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
    /// Resolves a path and requires its final symbol to be an effect.
    pub async fn resolve_effect_path(
        &mut self,
        path: &Path,
    ) -> Result<Effect, PathResolutionError> {
        let resolution = self.resolve_path(path).await?;

        match resolution {
            PathResolution::Effect(effect) => Ok(effect),
            PathResolution::Def(_) => {
                self.report_expected_effect(path.span(), SymbolKind::Def);
                Err(PathResolutionError::UnexpectedSymbolKind)
            }
            PathResolution::ExternDef(_) => {
                self.report_expected_effect(path.span(), SymbolKind::ExternDef);
                Err(PathResolutionError::UnexpectedSymbolKind)
            }
            PathResolution::Module(_) => {
                self.report_expected_effect(path.span(), SymbolKind::Module);
                Err(PathResolutionError::UnexpectedSymbolKind)
            }
            PathResolution::EffectOperation(_) => {
                self.report_expected_effect(path.span(), SymbolKind::EffectOperation);
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
        let symbol_id = self
            .find_path_symbol(previous.as_ref().map(PathResolution::symbol_id), &identifier.kind.0)
            .await;

        let Some(symbol_id) = symbol_id else {
            self.report_path_segment_not_found(identifier);
            return Err(PathResolutionError::SymbolNotFound);
        };

        let symbol_kind = self.symbol_kind(symbol_id).await;
        let expected = self.poly_var_kinds(symbol_id).await;
        let args = self.resolve_type_arguments(symbol_kind, path, &identifier, &expected).await;

        match symbol_kind {
            SymbolKind::Def => Ok(PathResolution::Def(Def::new(symbol_id, args))),
            SymbolKind::ExternDef => Ok(PathResolution::ExternDef(ExternDef::new(symbol_id))),
            SymbolKind::Module => Ok(PathResolution::Module(Module::new(symbol_id))),
            SymbolKind::Effect => Ok(PathResolution::Effect(Effect::new(symbol_id, args))),
            SymbolKind::EffectOperation => {
                let Some(PathResolution::Effect(effect)) = previous else {
                    unreachable!("an effect operation should be resolved through its parent effect")
                };
                Ok(PathResolution::EffectOperation(EffectOperation::new(effect, symbol_id)))
            }
            SymbolKind::Instance
            | SymbolKind::InstanceDef
            | SymbolKind::Trait
            | SymbolKind::TraitDef => Err(PathResolutionError::UnexpectedSymbolKind),
        }
    }
}

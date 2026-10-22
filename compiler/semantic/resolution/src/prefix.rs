//! Finds what the beginning of a path names, so that the segments that can
//! follow it can be listed while the path is typed.
//!
//! The lookups are the resolver's own, but they ignore the arguments of the
//! segments and report nothing.

use qbice::storage::intern::Interned;
use rayc_hash::FxHashSet;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    GlobalSymbolID, get_target_root_module_id,
    member::{get_members, try_get_members},
    name::get_name,
    parent::{get_closest_module_id, get_parent_global},
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_syntax::path::PathRoot;
use rayc_target::get_linked_targets;
use rayc_type::{
    accessibility::is_symbol_accessible_from,
    poly_var::{GlobalPolyVarID, get_enclosing_poly_var_maps},
    ty::TyKind,
};

use crate::resolver::{find_path_symbol, find_super_module, find_target_root};

/// What a path starts with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixRoot {
    /// `target`, the root module of the target of the site.
    Target,

    /// `super`, the parent of the module enclosing the site.
    Super,

    /// `this`, the trait enclosing the site.
    This,

    /// The name of a first segment.
    Name(Interned<str>),
}

/// The beginning of a path, up to the segment being completed: its root and
/// the names of the segments after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPrefix {
    root: PrefixRoot,
    segments: Vec<Interned<str>>,
}

impl PathPrefix {
    /// Creates the prefix made of `root` and the segments named `segments`.
    #[must_use]
    pub const fn new(root: PrefixRoot, segments: Vec<Interned<str>>) -> Self {
        Self { root, segments }
    }

    /// Creates the prefix made of the written root `root` and the segments
    /// named `segments`.
    ///
    /// Returns `None` if the root is a segment whose name is missing.
    #[must_use]
    pub fn with_root_syntax(root: &PathRoot, segments: Vec<Interned<str>>) -> Option<Self> {
        let root = match root {
            PathRoot::This(_) => PrefixRoot::This,
            PathRoot::Target(_) => PrefixRoot::Target,
            PathRoot::Super(_) => PrefixRoot::Super,
            PathRoot::Segment(segment) => PrefixRoot::Name(segment.identifier()?.kind.0),
        };

        Some(Self::new(root, segments))
    }

    /// Finds the symbol whose members can follow this prefix in a path
    /// written at `site`.
    ///
    /// Returns `None` if the prefix names nothing, or names something that
    /// has no members, such as a type parameter.
    pub async fn find_parent(
        &self,
        engine: &TrackedEngine,
        site: GlobalSymbolID,
    ) -> Option<GlobalSymbolID> {
        let mut parent = match &self.root {
            PrefixRoot::Target => find_target_root(engine, site).await,
            PrefixRoot::Super => find_super_module(engine, site).await?,
            PrefixRoot::This => find_this_trait(engine, site).await?,
            PrefixRoot::Name(name) => find_first_segment(engine, site, name).await?,
        };

        // each segment names a member of the symbol before it
        for name in &self.segments {
            parent = find_path_symbol(engine, site, Some(parent), name).await?;
        }

        Some(parent)
    }
}

/// Finds the trait that `this` names in a path written at `site`, the trait
/// whose member the site is, or `None` outside of a trait.
pub async fn find_this_trait(
    engine: &TrackedEngine,
    site: GlobalSymbolID,
) -> Option<GlobalSymbolID> {
    let parent = engine.get_parent_global(site).await?;
    (engine.get_symbol_kind(parent).await == SymbolKind::Trait).then_some(parent)
}

/// Finds the symbol whose members can follow a first segment named `name`.
async fn find_first_segment(
    engine: &TrackedEngine,
    site: GlobalSymbolID,
    name: &str,
) -> Option<GlobalSymbolID> {
    // a type parameter in scope shadows the symbols of the same name, and
    // only an instance parameter has members: those of its trait
    let poly_vars = engine.get_enclosing_poly_var_maps(site).await;
    if let Some(poly_var_id) = poly_vars.find_by_name(name) {
        return Some(poly_vars.trait_ref_of(poly_var_id)?.trait_id());
    }

    find_path_symbol(engine, site, None, name).await
}

/// What the first segment of a path can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStart {
    /// A type, effect or instance parameter in scope.
    PolyVar(GlobalPolyVarID),

    /// A member of the module enclosing the site, or the root module of a
    /// linked target.
    Symbol(GlobalSymbolID),
}

/// Lists what the first segment of a path written at `site` can name, in the
/// order the resolver looks names up.
///
/// A name is listed once: what is found first shadows the rest.
pub async fn path_starts(
    engine: &TrackedEngine,
    site: GlobalSymbolID,
) -> impl Iterator<Item = PathStart> {
    let mut starts = Vec::new();
    let mut names = FxHashSet::default();

    // the type parameters in scope, from the innermost symbol outwards;
    // lifetimes are written with a quote, and generated variables have no
    // name to write
    let poly_vars = engine.get_enclosing_poly_var_maps(site).await;
    for (poly_var_id, poly_var) in poly_vars.iter() {
        if poly_var.is_source()
            && poly_var.kind() != TyKind::Lifetime
            && names.insert(poly_var.name().clone())
        {
            starts.push(PathStart::PolyVar(poly_var_id));
        }
    }

    // the members of the closest module, which are all accessible from
    // inside it
    let module_id = site.target_id.make_global(engine.get_closest_module_id(site).await);
    for member_id in engine.get_members(module_id).await.namable_members() {
        let member_id = site.target_id.make_global(member_id);
        if names.insert(engine.get_name(member_id).await) {
            starts.push(PathStart::Symbol(member_id));
        }
    }

    // the root modules of the linked targets, named after their targets
    for target_id in engine.get_linked_targets(site.target_id).await.iter() {
        let root_id = target_id.make_global(engine.get_target_root_module_id(*target_id).await);
        if names.insert(engine.get_name(root_id).await) {
            starts.push(PathStart::Symbol(root_id));
        }
    }

    starts.into_iter()
}

/// Lists the members of `parent` that a path written at `site` can name.
pub async fn accessible_members(
    engine: &TrackedEngine,
    site: GlobalSymbolID,
    parent: GlobalSymbolID,
) -> impl Iterator<Item = GlobalSymbolID> {
    let mut members = Vec::new();

    if let Some(all_members) = engine.try_get_members(parent).await {
        for member_id in all_members.namable_members() {
            let member_id = parent.target_id.make_global(member_id);
            if engine.is_symbol_accessible_from(member_id, site).await {
                members.push(member_id);
            }
        }
    }

    members.into_iter()
}

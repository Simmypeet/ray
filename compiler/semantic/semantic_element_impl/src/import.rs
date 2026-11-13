//! Resolves the imports written at the top of a module into the symbols that
//! their names are bound to.

use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_diagnostic::{ByteIndex, Highlight, Rendered, Report};
use rayc_handler::Storage;
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_resolution::import::ImportResolver;
use rayc_semantic_element::import::{Imports, Key};
use rayc_symbol::{
    GlobalSymbolID, member::get_members, name::get_qualified_name, source_map::to_absolute_span,
    span::get_span, syntax::get_imports_syntax,
};
use rayc_syntax::{import::ImportItemName, path::Path};

use crate::{
    build::{Build, Output},
    register_build,
};

/// A diagnostic reported while resolving the imports of a module.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Diagnostic {
    /// An imported path could not be resolved into something importable.
    Resolution(rayc_resolution::Diagnostic),

    /// An import binds a name that is already taken in the module.
    NameCollision(ImportNameCollision),

    /// An import of a keyword path, such as `super`, without an alias.
    Unnamed(UnnamedImport),
}

impl Report for Diagnostic {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::Resolution(diagnostic) => diagnostic.report(engine).await,
            Self::NameCollision(diagnostic) => diagnostic.report(engine).await,
            Self::Unnamed(diagnostic) => diagnostic.report(engine).await,
        }
    }
}

/// What already owns the name that an import binds.
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
pub enum ImportNameOwner {
    /// A member declared in the module.
    Member(GlobalSymbolID),

    /// An earlier import, whose name is written at the given span.
    Import(RelativeSpan),
}

impl ImportNameOwner {
    /// Returns where the owner of the name is written, if anywhere.
    async fn highlight(&self, engine: &TrackedEngine) -> Option<Highlight<ByteIndex>> {
        match self {
            Self::Member(symbol_id) => {
                let span = engine.get_span(*symbol_id).await?;
                Some(Highlight::new(
                    engine.to_absolute_span(&span).await,
                    Some("the name is declared here".to_owned()),
                ))
            }
            Self::Import(span) => Some(Highlight::new(
                engine.to_absolute_span(span).await,
                Some("the name is imported here".to_owned()),
            )),
        }
    }
}

/// An import binding a name that a member of the module or an earlier import
/// already has.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct ImportNameCollision {
    name: Interned<str>,

    /// The span of the name that the import binds.
    span: RelativeSpan,

    owner: ImportNameOwner,

    /// The module the import is written in.
    module_id: GlobalSymbolID,
}

impl Report for ImportNameCollision {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        let module_name = engine.get_qualified_name(self.module_id).await;
        let related = self.owner.highlight(engine).await.map(|highlight| vec![highlight]);

        Rendered::builder()
            .message(format!(
                "the name `{}` is already defined in the scope `{module_name}`",
                self.name.as_ref()
            ))
            .primary_highlight(Highlight::new(
                engine.to_absolute_span(&self.span).await,
                Some("imported again here".to_owned()),
            ))
            .maybe_related(related)
            .help_message("rename the import with `as`")
            .build()
    }
}

/// An import of a path made of only a keyword, such as `super`, that has no
/// alias to name it.
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
pub struct UnnamedImport {
    span: RelativeSpan,
}

impl Report for UnnamedImport {
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        Rendered::builder()
            .message("the imported path has no name to bind")
            .primary_highlight(Highlight::new(engine.to_absolute_span(&self.span).await, None))
            .help_message("name the import with `as`, as in `import super as parent`")
            .build()
    }
}

/// Resolves the `from` clauses of the imports of a module, each only once:
/// the items of `from path import (a, b)` share their clause.
#[derive(Debug, Default)]
struct FromClauses {
    parents_by_path: FxHashMap<Path, Option<GlobalSymbolID>>,
}

impl FromClauses {
    /// Returns the symbol that `from` names, or `None` if it names nothing.
    async fn resolve(
        &mut self,
        resolver: ImportResolver<'_>,
        from: &Path,
    ) -> Option<GlobalSymbolID> {
        if let Some(parent) = self.parents_by_path.get(from) {
            return *parent;
        }

        let parent = resolver.resolve_from(from).await;
        self.parents_by_path.insert(from.clone(), parent);
        parent
    }
}

impl Build for Key {
    type Diagnostic = Diagnostic;

    async fn execute(engine: &TrackedEngine, &Self { module_id }: &Self) -> Output<Self> {
        let syntax = engine.get_imports_syntax(module_id).await;
        let members = engine.get_members(module_id).await;
        let resolution_diagnostics = Storage::new();
        let resolver = ImportResolver::new(engine, module_id, &resolution_diagnostics);

        let mut diagnostics = Vec::new();
        let mut imports = Imports::new();
        let mut spans_by_name = FxHashMap::default();
        let mut from_clauses = FromClauses::default();

        for import in syntax.iter() {
            let from = import.from().and_then(|from| from.path());
            let Some(list) = import.list() else { continue };

            for item in list.items() {
                let Some(path) = item.path() else { continue };

                // find the name the item binds
                let identifier = match item.bound_name() {
                    ImportItemName::Identifier(identifier) => identifier,
                    ImportItemName::Keyword(span) => {
                        diagnostics.push(Diagnostic::Unnamed(UnnamedImport { span }));
                        continue;
                    }
                    ImportItemName::Missing => continue,
                };
                let name = identifier.kind.0.clone();

                // the name must not be taken by a member, even one declared
                // after the imports, nor by an earlier import
                let owner = members
                    .get_by_name(&name)
                    .map(|id| ImportNameOwner::Member(module_id.target_id.make_global(id)))
                    .or_else(|| spans_by_name.get(&name).copied().map(ImportNameOwner::Import));
                if let Some(owner) = owner {
                    diagnostics.push(Diagnostic::NameCollision(ImportNameCollision {
                        name,
                        span: identifier.span,
                        owner,
                        module_id,
                    }));
                    continue;
                }
                spans_by_name.insert(name.clone(), identifier.span);

                // resolve the path, relative to the symbol of the `from`
                // clause; an item of a clause that names nothing binds nothing
                let parent = match &from {
                    Some(from) => match from_clauses.resolve(resolver, from).await {
                        Some(parent) => Some(parent),
                        None => continue,
                    },
                    None => None,
                };
                if let Some(symbol_id) = resolver.resolve_import(parent, &path).await {
                    imports.insert(name, symbol_id);
                }
            }
        }

        // the resolution diagnostics are reported along with the others
        diagnostics
            .extend(resolution_diagnostics.into_vec().into_iter().map(Diagnostic::Resolution));

        Output::new_with(engine.intern(imports), diagnostics, [], engine)
    }
}

register_build!(Key);

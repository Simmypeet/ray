//! Completes the segment of a path at the cursor.
//!
//! A path is a `Path` node wherever the syntax has one: in signatures, and in
//! the calls and struct initializations of a body. While a call is typed in a
//! body, before its `(`, the path is still a chain of field accesses on an
//! identifier that is not a local.
//!
//! The segments before the cursor are looked up as the resolver does, from
//! the site the path is written at, and what can follow them is filtered by
//! the position of the path, such as a type or a call.

use rayc_lexical::tree::Tree;
use rayc_parser::concrete_tree::Node;
use rayc_qbice::TrackedEngine;
use rayc_resolution::prefix::{
    PathPrefix, PathStart, PrefixRoot, accessible_members, find_this_trait, path_starts,
};
use rayc_source_file::{ByteIndex, SourceElement};
use rayc_symbol::{
    GlobalSymbolID,
    name::get_name,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_syntax::{
    effect_row::{ConcreteEffectRow, EffectRow, EffectRowTail},
    expression::{
        DirectCall, DotMember, HandleOperationClause, Leaf, Postfix, PostfixOperator, RunWith,
        StructInitialization,
    },
    given::{GivenArgument, GivenParameter},
    instance::Instance,
    marker::MarkerImplementation,
    path::Path,
    r#type::Type,
    where_clause::MarkerPredicate,
};
use rayc_type::{poly_var::get_poly_var_map, ty::TyKind};
use rayc_typed_ast::typed_function::TypedFunctionMap;
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

use crate::segment::SegmentAt;

/// The segment of a path at the cursor, and where the path is written.
pub(super) struct PathCursor {
    segment: Segment,
    position: Position,
}

/// Which segment of a path the cursor is on.
enum Segment {
    /// The first segment, or the keyword the path starts with.
    First,

    /// A segment after `0`, the beginning of the path.
    After(PathPrefix),
}

impl PathCursor {
    /// Finds the `Path` node at `offset` in `syntax`, whose spans are relative
    /// to `token_tree`.
    pub(super) fn at(token_tree: &Tree, syntax: &Node, offset: ByteIndex) -> Option<Self> {
        // the cursor is on the root, or on a continuation right after its
        // `.` or in its name
        let before = offset.checked_sub(1)?;
        let segment_at =
            SegmentAt::find(token_tree, syntax, before, |span| span.contains_cursor(offset))?;

        let position = Position::of(token_tree, syntax, segment_at.path(), before);
        let segment = if segment_at.is_on_root() {
            Segment::First
        } else {
            Segment::After(segment_at.into_prefix_before()?)
        };

        Some(Self { segment, position })
    }

    /// Finds the path being typed at `offset` in `body`, before the `(` or
    /// `{` that makes the parser read it as one: an identifier followed by
    /// field accesses, where the identifier is not a local in `functions`.
    pub(super) async fn in_postfix(
        engine: &TrackedEngine,
        token_tree: &Tree,
        body: &Node,
        functions: &TypedFunctionMap,
        offset: ByteIndex,
    ) -> Option<Self> {
        let before = offset.checked_sub(1)?;
        let postfix = body.get_deepest_ast::<Postfix>(token_tree, before)?;
        let Some(Leaf::Identifier(root)) = postfix.leaf() else {
            return None;
        };

        // the field names before the `.` holding the cursor are the segments
        // of the path
        let mut names = Vec::new();
        let mut operators = postfix.postfixes();
        loop {
            let PostfixOperator::Dot(dot) = operators.next()? else {
                return None;
            };
            if token_tree.span_contains_cursor(&dot.span(), offset) {
                break;
            }
            let Some(DotMember::FieldAccess(field_access)) = dot.member() else {
                return None;
            };
            names.push(field_access.name()?.kind.0);
        }

        // a local of the same name makes the chain a member access, as the
        // binder looks up a bare identifier among the locals first
        let root_end = token_tree.absolute_span_of(&root.span).end;
        let is_local = functions
            .visible_name_bindings(engine, root_end)
            .await
            .any(|(_, name_binding)| *name_binding.name() == root.kind.0);
        if is_local {
            return None;
        }

        Some(Self {
            segment: Segment::After(PathPrefix::new(PrefixRoot::Name(root.kind.0), names)),
            position: Position::Expression,
        })
    }

    /// Returns whether the cursor is on a first segment that may name a
    /// local, such as the callee of a call.
    pub(super) const fn may_name_local(&self) -> bool {
        match self.segment {
            Segment::First => self.position.offers_locals(),
            Segment::After(_) => false,
        }
    }

    /// Lists what the segment at the cursor can name in a path written at
    /// `site`.
    pub(super) async fn complete(
        &self,
        engine: &TrackedEngine,
        site: GlobalSymbolID,
    ) -> Vec<CompletionItem> {
        match &self.segment {
            Segment::First => {
                let mut items = first_segments(engine, site, self.position).await;
                items.extend(self.keywords(engine, site).await);
                items
            }
            Segment::After(prefix) => self.complete_after(engine, site, prefix).await,
        }
    }

    /// Lists the members of what `prefix` names that fit the position,
    /// sorted by name.
    async fn complete_after(
        &self,
        engine: &TrackedEngine,
        site: GlobalSymbolID,
        prefix: &PathPrefix,
    ) -> Vec<CompletionItem> {
        let Some(parent) = prefix.find_parent(engine, site).await else {
            return Vec::new();
        };

        let mut items = Vec::new();
        for member_id in accessible_members(engine, site, parent).await {
            if self.position.offers(engine.get_symbol_kind(member_id).await) {
                items.push(symbol_item(engine, member_id, None).await);
            }
        }

        // the members are found in no particular order
        items.sort_by(|left, right| left.label.cmp(&right.label));
        items
    }

    /// Lists the keywords a path can start with at `site`: `target`, `super`
    /// outside of the root module, and `this` in a trait.
    async fn keywords(&self, engine: &TrackedEngine, site: GlobalSymbolID) -> Vec<CompletionItem> {
        let mut keywords = vec!["target"];
        if PathPrefix::new(PrefixRoot::Super, Vec::new()).find_parent(engine, site).await.is_some()
        {
            keywords.push("super");
        }
        if self.position.offers_this() && find_this_trait(engine, site).await.is_some() {
            keywords.push("this");
        }

        keywords
            .into_iter()
            .map(|keyword| CompletionItem {
                label: keyword.to_owned(),
                kind: Some(CompletionItemKind::KEYWORD),
                sort_text: Some(format!("{}{keyword}", Rank::Keyword.prefix())),
                ..CompletionItem::default()
            })
            .collect()
    }
}

/// Lists the symbols and type parameters that the first segment of a path
/// written at `site` in `position` can name.
pub(super) async fn first_segments(
    engine: &TrackedEngine,
    site: GlobalSymbolID,
    position: Position,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    for start in path_starts(engine, site).await {
        match start {
            PathStart::PolyVar(poly_var_id) => {
                let poly_var_map = engine.get_poly_var_map(poly_var_id.parent_id()).await;
                let kind = poly_var_map.kind_of(poly_var_id.id());
                if position.offers_poly_var(kind) {
                    let label = poly_var_map.name_of(poly_var_id.id()).to_owned();
                    items.push(CompletionItem {
                        sort_text: Some(format!("{}{label}", Rank::Start.prefix())),
                        label,
                        kind: Some(CompletionItemKind::TYPE_PARAMETER),
                        detail: Some(poly_var_detail(kind).to_owned()),
                        ..CompletionItem::default()
                    });
                }
            }
            PathStart::Symbol(symbol_id) => {
                if position.offers(engine.get_symbol_kind(symbol_id).await) {
                    items.push(symbol_item(engine, symbol_id, Some(Rank::Start)).await);
                }
            }
        }
    }

    items
}

/// The groups of items that a first segment lists, and the keywords that
/// follow them, in the order they are offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Rank {
    /// A local variable, which a bare identifier in a body names first.
    Local,

    /// A type parameter or a symbol.
    Start,

    /// A keyword, such as one a path starts with.
    Keyword,
}

impl Rank {
    /// Returns the prefix of the `sortText` of the items of this group.
    pub(super) const fn prefix(self) -> char {
        match self {
            Self::Local => '0',
            Self::Start => '1',
            Self::Keyword => '2',
        }
    }
}

/// Where a path is written, which decides what it may name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Position {
    /// A type, such as the type of a parameter.
    Type,

    /// An effect row, such as `\ {State}`.
    EffectRow,

    /// The trait of an instance or of a given parameter.
    Trait,

    /// The marker of an implementation or of a where-clause predicate.
    Marker,

    /// The effect handled by `run ... with`.
    Effect,

    /// The operation of a `handle` clause.
    Operation,

    /// The callee of a call.
    Call,

    /// The struct of a struct initialization.
    StructInitialization,

    /// An expression being typed, which may become a call or a struct
    /// initialization.
    Expression,

    /// A given argument, which names a dictionary.
    Dictionary,

    /// A position not recognized, where anything is offered.
    Unknown,
}

impl Position {
    /// Finds where `path`, which contains `byte` in `syntax`, is written.
    fn of(token_tree: &Tree, syntax: &Node, path: &Path, byte: ByteIndex) -> Self {
        let span = path.span();
        let is_path = |candidate: Option<Path>| candidate.is_some_and(|other| other.span() == span);

        // the deepest node of each kind holding a path that contains the
        // byte holds this path if its path is this one
        if syntax.get_deepest_ast::<DirectCall>(token_tree, byte).is_some_and(|x| is_path(x.path()))
        {
            return Self::Call;
        }
        if syntax
            .get_deepest_ast::<StructInitialization>(token_tree, byte)
            .is_some_and(|x| is_path(x.path()))
        {
            return Self::StructInitialization;
        }
        if syntax.get_deepest_ast::<RunWith>(token_tree, byte).is_some_and(|x| is_path(x.effect()))
        {
            return Self::Effect;
        }
        if syntax
            .get_deepest_ast::<HandleOperationClause>(token_tree, byte)
            .is_some_and(|x| is_path(x.operation()))
        {
            return Self::Operation;
        }
        if syntax.get_deepest_ast::<Type>(token_tree, byte).is_some_and(|x| is_path(x.into_path()))
        {
            return Self::Type;
        }
        if syntax
            .get_deepest_ast::<EffectRow>(token_tree, byte)
            .is_some_and(|x| is_path(x.into_path()))
            || syntax
                .get_deepest_ast::<ConcreteEffectRow>(token_tree, byte)
                .is_some_and(|x| x.effects().any(|effect| is_path(Some(effect))))
            || syntax
                .get_deepest_ast::<EffectRowTail>(token_tree, byte)
                .is_some_and(|x| is_path(x.variable()))
        {
            return Self::EffectRow;
        }
        if syntax
            .get_deepest_ast::<Instance>(token_tree, byte)
            .is_some_and(|x| is_path(x.trait_reference()))
            || syntax
                .get_deepest_ast::<GivenParameter>(token_tree, byte)
                .is_some_and(|x| is_path(x.trait_reference()))
        {
            return Self::Trait;
        }
        if syntax
            .get_deepest_ast::<MarkerImplementation>(token_tree, byte)
            .is_some_and(|x| is_path(x.marker()))
            || syntax
                .get_deepest_ast::<MarkerPredicate>(token_tree, byte)
                .is_some_and(|x| is_path(x.marker()))
        {
            return Self::Marker;
        }
        if syntax
            .get_deepest_ast::<GivenArgument>(token_tree, byte)
            .is_some_and(|x| is_path(x.dictionary()))
        {
            return Self::Dictionary;
        }

        Self::Unknown
    }

    /// Returns whether a symbol of `kind` fits this position, or can lead to
    /// one that does.
    const fn offers(self, kind: SymbolKind) -> bool {
        match kind {
            // a module can lead to anything
            SymbolKind::Module => true,
            SymbolKind::Struct => matches!(
                self,
                Self::Type | Self::StructInitialization | Self::Expression | Self::Unknown
            ),
            // an enum names a type and leads to its variants
            SymbolKind::Enum => {
                matches!(self, Self::Type | Self::Call | Self::Expression | Self::Unknown)
            }
            SymbolKind::Variant => matches!(self, Self::Call | Self::Expression | Self::Unknown),
            SymbolKind::Def
            | SymbolKind::ExternDef
            | SymbolKind::TraitDef
            | SymbolKind::InstanceDef => {
                matches!(self, Self::Call | Self::Expression | Self::Unknown)
            }
            SymbolKind::EffectOperation => {
                matches!(self, Self::Call | Self::Operation | Self::Expression | Self::Unknown)
            }
            SymbolKind::Effect => matches!(
                self,
                Self::EffectRow
                    | Self::Effect
                    | Self::Operation
                    | Self::Call
                    | Self::Expression
                    | Self::Unknown
            ),
            SymbolKind::Trait => {
                matches!(self, Self::Trait | Self::Call | Self::Expression | Self::Unknown)
            }
            SymbolKind::Marker => matches!(self, Self::Marker | Self::Unknown),
            SymbolKind::TraitType | SymbolKind::InstanceType => {
                matches!(self, Self::Type | Self::Unknown)
            }
            SymbolKind::Instance => matches!(
                self,
                Self::Type | Self::Call | Self::Expression | Self::Dictionary | Self::Unknown
            ),
            // an implementation has no name to write
            SymbolKind::MarkerImplementation => false,
        }
    }

    /// Returns whether a type parameter of `kind` fits this position, or can
    /// lead to what does, as an instance parameter leads to the members of
    /// its trait.
    const fn offers_poly_var(self, kind: TyKind) -> bool {
        match kind {
            TyKind::Star => matches!(self, Self::Type | Self::Unknown),
            TyKind::EffectRow => matches!(self, Self::EffectRow | Self::Unknown),
            TyKind::Instance => matches!(
                self,
                Self::Type | Self::Call | Self::Expression | Self::Dictionary | Self::Unknown
            ),
            // a lifetime is written with a quote, not as a path
            TyKind::Lifetime => false,
        }
    }

    /// Returns whether `this`, the dictionary of the enclosing trait, fits
    /// this position.
    const fn offers_this(self) -> bool { self.offers_poly_var(TyKind::Instance) }

    /// Returns whether a local may be named here: a call may call a local
    /// closure, and an expression may be a local.
    pub(super) const fn offers_locals(self) -> bool {
        matches!(self, Self::Call | Self::Expression)
    }
}

/// Creates the item of the symbol `symbol_id`, in the group `rank` of a first
/// segment if any.
async fn symbol_item(
    engine: &TrackedEngine,
    symbol_id: GlobalSymbolID,
    rank: Option<Rank>,
) -> CompletionItem {
    let kind = engine.get_symbol_kind(symbol_id).await;
    let label = engine.get_name(symbol_id).await.to_string();

    CompletionItem {
        sort_text: rank.map(|rank| format!("{}{label}", rank.prefix())),
        label,
        kind: Some(item_kind(kind)),
        detail: Some(kind.str().to_owned()),
        ..CompletionItem::default()
    }
}

/// Returns the kind of item that shows a symbol of `kind`.
const fn item_kind(kind: SymbolKind) -> CompletionItemKind {
    match kind {
        SymbolKind::Module => CompletionItemKind::MODULE,
        SymbolKind::Def
        | SymbolKind::ExternDef
        | SymbolKind::EffectOperation
        | SymbolKind::TraitDef
        | SymbolKind::InstanceDef => CompletionItemKind::FUNCTION,
        SymbolKind::Struct => CompletionItemKind::STRUCT,
        SymbolKind::Enum => CompletionItemKind::ENUM,
        SymbolKind::Variant => CompletionItemKind::ENUM_MEMBER,
        SymbolKind::Effect | SymbolKind::Trait | SymbolKind::Marker => {
            CompletionItemKind::INTERFACE
        }
        SymbolKind::TraitType | SymbolKind::InstanceType => CompletionItemKind::TYPE_PARAMETER,
        SymbolKind::Instance | SymbolKind::MarkerImplementation => CompletionItemKind::CLASS,
    }
}

/// Returns what a type parameter of `kind` is called in an item.
const fn poly_var_detail(kind: TyKind) -> &'static str {
    match kind {
        TyKind::Star => "type parameter",
        TyKind::EffectRow => "effect parameter",
        TyKind::Instance => "instance parameter",
        TyKind::Lifetime => "lifetime parameter",
    }
}

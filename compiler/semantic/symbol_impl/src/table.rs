use std::{collections::hash_map::Entry, path::Path, sync::Arc};

use bon::Builder;
use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_extend::extend;
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_source_file::{LocalSourceID, SOURCE_FILE_EXTENSION, get_stable_path_id};
use rayc_symbol::{
    GlobalSymbolID, SymbolID, calculate_implements_id, calculate_qualified_name_id,
    get_target_root_module_id,
    member::{Insertion, Member},
    symbol_kind::SymbolKind,
};
use rayc_syntax::{
    def::{ParameterList, ReturnType},
    effect::TypeParameterList,
    effect_row::EffectRowAnnotation,
    given::GivenParameterList,
    kind::KindAscription,
    module::ModuleContent,
    path::Path as SyntaxPath,
    statement::Block,
    r#struct::StructBody,
    r#type::Type,
    where_clause::WhereClause,
};
use rayc_target::{TargetID, get_invocation_arguments};

use crate::diagnostic::{Diagnostic, ItemRedefinition, SourceFileLoadFail};

type Map<V> = FxHashMap<SymbolID, V>;

#[derive(Debug, Builder)]
#[allow(clippy::option_option)]
pub struct Infos {
    name: Interned<str>,
    span: Option<RelativeSpan>,
    symbol_kind: SymbolKind,
    parameter_list: Option<Option<ParameterList>>,
    return_type: Option<Option<ReturnType>>,
    effect_row: Option<Option<EffectRowAnnotation>>,
    member: Option<MemberBuilder>,
    def_body: Option<Option<Block>>,
    struct_body: Option<Option<StructBody>>,
    linear_struct: Option<bool>,
    variadic: Option<bool>,
    type_parameters: Option<Option<TypeParameterList>>,
    given_parameter_list: Option<Option<GivenParameterList>>,
    where_clause: Option<Option<WhereClause>>,
    instance_trait: Option<Option<SyntaxPath>>,
    negative_marker_implementation: Option<bool>,
    marker_implementation_marker: Option<Option<SyntaxPath>>,
    marker_implementation_type: Option<Option<Type>>,
    type_definition: Option<Option<Type>>,
    kind_ascription: Option<Option<KindAscription>>,
}

#[derive(Debug, Default, StableHash, Encode, Decode)]
struct SyntaxTable {
    parameter_lists: Map<Option<ParameterList>>,
    return_types: Map<Option<ReturnType>>,
    effect_rows: Map<Option<EffectRowAnnotation>>,
    def_bodies: Map<Option<Block>>,
    struct_bodies: Map<Option<StructBody>>,
    linear_structs: Map<bool>,
    variadic_defs: Map<bool>,
    type_parameters: Map<Option<TypeParameterList>>,
    given_parameter_lists: Map<Option<GivenParameterList>>,
    where_clauses: Map<Option<WhereClause>>,
    instance_traits: Map<Option<SyntaxPath>>,
    negative_marker_implementations: Map<bool>,
    marker_implementation_markers: Map<Option<SyntaxPath>>,
    marker_implementation_types: Map<Option<Type>>,
    type_definitions: Map<Option<Type>>,
    kind_ascriptions: Map<Option<KindAscription>>,
}

/// Stores the information of the symbols declared in a single source file. It
/// maps the symbol ID to its related information.
///
/// A file module is declared in one file and defines its members in another:
/// its declaration is stored in the table of the declaring file, which links
/// to the table of the module's own file, where its members are stored.
#[derive(Debug, Default, StableHash, Encode, Decode)]
pub struct Table {
    symbol_kinds: Map<SymbolKind>,
    members: Map<Interned<Member>>,
    parents: Map<Option<SymbolID>>,
    spans: Map<Option<RelativeSpan>>,
    names: Map<Interned<str>>,

    syntaxes: SyntaxTable,

    /// The ID of the source file of this table, or `None` if the file could
    /// not be loaded.
    source_id: Option<LocalSourceID>,

    /// The tables of the file modules declared in this file, in declaration
    /// order.
    next_tables: Vec<TableKey>,

    diagnostics: Vec<Diagnostic>,
}

impl Table {
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] { &self.diagnostics }

    pub(crate) fn push_diagnostic(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    #[must_use]
    pub fn member(&self, id: SymbolID) -> Option<&Interned<Member>> { self.members.get(&id) }
}

#[derive(Debug, Default)]
pub struct MemberBuilder {
    current_id: GlobalSymbolID,
    current_qualified_name: Vec<Interned<str>>,

    member: Member,
    occurrences: FxHashMap<Interned<str>, usize>,

    redef_errors: Vec<ItemRedefinition>,
}

impl MemberBuilder {
    #[must_use]
    pub fn new(current_id: GlobalSymbolID, current_qualified_name: Vec<Interned<str>>) -> Self {
        Self {
            current_id,
            current_qualified_name,
            member: Member::default(),
            occurrences: FxHashMap::default(),
            redef_errors: Vec::new(),
        }
    }

    /// Checks whether a member with the given name has already been
    /// registered.
    #[must_use]
    pub(crate) fn has_member(&self, name: &str) -> bool { self.member.get_by_name(name).is_some() }

    /// Returns the qualified name of the symbol whose members are being built.
    #[must_use]
    pub(crate) fn qualified_name(&self) -> &[Interned<str>] { &self.current_qualified_name }

    #[must_use]
    pub(crate) fn child(&self, current_id: GlobalSymbolID, name: Interned<str>) -> Self {
        let mut qualified_name = self.current_qualified_name.clone();
        qualified_name.push(name);
        Self::new(current_id, qualified_name)
    }
}

impl Table {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    #[must_use]
    pub fn get_symbol_kind(&self, symbol_id: SymbolID) -> SymbolKind {
        self.symbol_kinds.get(&symbol_id).copied().unwrap()
    }

    #[must_use]
    pub fn get_span(&self, symbol_id: SymbolID) -> Option<RelativeSpan> {
        self.spans.get(&symbol_id).copied().unwrap()
    }

    #[must_use]
    pub fn get_name(&self, symbol_id: SymbolID) -> Interned<str> {
        self.names.get(&symbol_id).cloned().unwrap()
    }

    #[must_use]
    pub fn get_parent(&self, symbol_id: SymbolID) -> Option<SymbolID> {
        self.parents.get(&symbol_id).copied().unwrap()
    }

    #[must_use]
    pub fn get_parameter_list_syntax(&self, symbol_id: SymbolID) -> Option<ParameterList> {
        self.syntaxes.parameter_lists.get(&symbol_id).cloned().unwrap()
    }

    #[must_use]
    pub fn get_return_type_syntax(&self, symbol_id: SymbolID) -> Option<ReturnType> {
        self.syntaxes.return_types.get(&symbol_id).cloned().unwrap()
    }

    #[must_use]
    pub fn get_effect_row_syntax(&self, symbol_id: SymbolID) -> Option<EffectRowAnnotation> {
        self.syntaxes.effect_rows.get(&symbol_id).cloned().unwrap()
    }

    #[must_use]
    pub fn get_def_body_syntax(&self, symbol_id: SymbolID) -> Option<Block> {
        self.syntaxes.def_bodies.get(&symbol_id).cloned().unwrap()
    }

    #[must_use]
    pub fn get_struct_body_syntax(&self, symbol_id: SymbolID) -> Option<StructBody> {
        self.syntaxes.struct_bodies.get(&symbol_id).cloned().unwrap()
    }

    #[must_use]
    pub fn is_linear_struct(&self, symbol_id: SymbolID) -> bool {
        self.syntaxes.linear_structs.get(&symbol_id).copied().unwrap()
    }

    #[must_use]
    pub fn is_variadic_def(&self, symbol_id: SymbolID) -> bool {
        self.syntaxes.variadic_defs.get(&symbol_id).copied().unwrap()
    }

    #[must_use]
    pub fn get_type_parameter_list_syntax(&self, symbol_id: SymbolID) -> Option<TypeParameterList> {
        self.syntaxes.type_parameters.get(&symbol_id).cloned().unwrap()
    }

    #[must_use]
    pub fn get_given_parameter_list_syntax(
        &self,
        symbol_id: SymbolID,
    ) -> Option<GivenParameterList> {
        self.syntaxes.given_parameter_lists.get(&symbol_id).cloned().unwrap()
    }

    #[must_use]
    pub fn get_where_clause_syntax(&self, symbol_id: SymbolID) -> Option<WhereClause> {
        self.syntaxes.where_clauses.get(&symbol_id).cloned().unwrap()
    }

    #[must_use]
    pub fn get_instance_trait_syntax(&self, symbol_id: SymbolID) -> Option<SyntaxPath> {
        self.syntaxes.instance_traits.get(&symbol_id).cloned().unwrap()
    }

    #[must_use]
    pub fn get_marker_implementation_marker_syntax(
        &self,
        symbol_id: SymbolID,
    ) -> Option<SyntaxPath> {
        self.syntaxes.marker_implementation_markers.get(&symbol_id).cloned().unwrap()
    }

    #[must_use]
    pub fn is_negative_marker_implementation(&self, symbol_id: SymbolID) -> bool {
        self.syntaxes.negative_marker_implementations.get(&symbol_id).copied().unwrap()
    }

    #[must_use]
    pub fn get_marker_implementation_type_syntax(&self, symbol_id: SymbolID) -> Option<Type> {
        self.syntaxes.marker_implementation_types.get(&symbol_id).cloned().unwrap()
    }

    #[must_use]
    pub fn get_type_definition_syntax(&self, symbol_id: SymbolID) -> Option<Type> {
        self.syntaxes.type_definitions.get(&symbol_id).cloned().unwrap()
    }

    #[must_use]
    pub fn get_kind_ascription_syntax(&self, symbol_id: SymbolID) -> Option<KindAscription> {
        self.syntaxes.kind_ascriptions.get(&symbol_id).cloned().unwrap()
    }

    /// Returns the ID of the source file of this table, or `None` if the file
    /// could not be loaded.
    #[must_use]
    pub const fn source_id(&self) -> Option<LocalSourceID> { self.source_id }

    /// Returns the keys of the tables of the file modules declared in this
    /// file.
    #[must_use]
    pub fn next_tables(&self) -> impl DoubleEndedIterator<Item = &TableKey> {
        self.next_tables.iter()
    }

    /// Returns the key of the table storing the members of the given file
    /// module declared in this file.
    #[must_use]
    pub fn file_module_table(&self, symbol_id: SymbolID) -> Option<&TableKey> {
        self.next_tables.iter().find(|table_key| table_key.module_id == symbol_id)
    }

    /// Records the table of a file module declared in this file, which stores
    /// the members of the module.
    pub(crate) fn push_next_table(&mut self, table_key: TableKey) {
        self.next_tables.push(table_key);
    }

    /// Loads the module content of the source file of `key`.
    ///
    /// Loading failures are reported as diagnostics, pointing at the
    /// declaration of the file module when the file belongs to one.
    async fn load_source_file(
        &mut self,
        key: &TableKey,
        engine: &TrackedEngine,
    ) -> Option<ModuleContent> {
        let report_failure = |table: &mut Self, error_message: String| {
            table.diagnostics.push(Diagnostic::SourceFileLoadFail(SourceFileLoadFail {
                error_message,
                path: key.path.clone(),
                file_module_id: (!key.is_target_root())
                    .then(|| key.target_id.make_global(key.module_id)),
            }));
        };

        // read the file first so that a missing or unreadable file is reported
        // with its IO error
        if let Err(error) = engine
            .query(&rayc_source_file::Key { path: key.path.clone(), target_id: key.target_id })
            .await
        {
            report_failure(self, error.to_string());
            return None;
        }

        // the token tree requires the file to have a stable ID, which fails
        // when the file lies outside of the target directory
        match engine.get_stable_path_id(key.path.clone(), key.target_id).await {
            Ok(source_id) => self.source_id = Some(source_id),
            Err(error) => {
                report_failure(self, error.to_string());
                return None;
            }
        }

        engine
            .query(&rayc_syntax::Key { path: key.path.clone(), target_id: key.target_id })
            .await
            .expect("the source file has been loaded successfully")
            .0
    }

    fn insert_member_as_root_module(&mut self, member: MemberBuilder, engine: &TrackedEngine) {
        self.insert_info(
            member.current_id.id,
            None,
            Infos::builder()
                .symbol_kind(SymbolKind::Module)
                .name(member.current_qualified_name[0].clone())
                .member(member)
                .build(),
            engine,
        );
    }

    fn insert_info(
        &mut self,
        symbol_id: SymbolID,
        parent: Option<SymbolID>,
        info: Infos,
        engine: &TrackedEngine,
    ) {
        self.spans.insert(symbol_id, info.span);
        self.names.insert(symbol_id, info.name);
        self.parents.insert(symbol_id, parent);
        self.symbol_kinds.insert(symbol_id, info.symbol_kind);

        if let Some(parameter_list) = info.parameter_list {
            self.syntaxes.parameter_lists.insert(symbol_id, parameter_list);
        }

        if let Some(return_type) = info.return_type {
            self.syntaxes.return_types.insert(symbol_id, return_type);
        }

        if let Some(effect_row) = info.effect_row {
            self.syntaxes.effect_rows.insert(symbol_id, effect_row);
        }

        if let Some(def_body) = info.def_body {
            self.syntaxes.def_bodies.insert(symbol_id, def_body);
        }

        if let Some(struct_body) = info.struct_body {
            self.syntaxes.struct_bodies.insert(symbol_id, struct_body);
        }

        if let Some(linear) = info.linear_struct {
            self.syntaxes.linear_structs.insert(symbol_id, linear);
        }

        if let Some(variadic) = info.variadic {
            self.syntaxes.variadic_defs.insert(symbol_id, variadic);
        }

        if let Some(type_parameters) = info.type_parameters {
            self.syntaxes.type_parameters.insert(symbol_id, type_parameters);
        }

        if let Some(given_parameter_list) = info.given_parameter_list {
            self.syntaxes.given_parameter_lists.insert(symbol_id, given_parameter_list);
        }

        if let Some(where_clause) = info.where_clause {
            self.syntaxes.where_clauses.insert(symbol_id, where_clause);
        }

        if let Some(instance_trait) = info.instance_trait {
            self.syntaxes.instance_traits.insert(symbol_id, instance_trait);
        }

        if let Some(negative) = info.negative_marker_implementation {
            self.syntaxes.negative_marker_implementations.insert(symbol_id, negative);
        }

        if let Some(marker) = info.marker_implementation_marker {
            self.syntaxes.marker_implementation_markers.insert(symbol_id, marker);
        }

        if let Some(implementor) = info.marker_implementation_type {
            self.syntaxes.marker_implementation_types.insert(symbol_id, implementor);
        }

        if let Some(type_definition) = info.type_definition {
            self.syntaxes.type_definitions.insert(symbol_id, type_definition);
        }

        if let Some(kind_ascription) = info.kind_ascription {
            self.syntaxes.kind_ascriptions.insert(symbol_id, kind_ascription);
        }

        if let Some(member) = info.member {
            self.members.insert(symbol_id, engine.intern(member.member));
            self.diagnostics
                .extend(member.redef_errors.into_iter().map(Diagnostic::ItemRedefinition));
        }
    }

    pub async fn insert_symbol(
        &mut self,
        member_builder: &mut MemberBuilder,
        info: Infos,
        engine: &TrackedEngine,
    ) -> GlobalSymbolID {
        // retrieves the occurrence count of the member name. normally,
        // this `count` should be 0 if no redefinition has been encountered.
        let count = match member_builder.occurrences.entry(info.name.clone()) {
            Entry::Occupied(mut occupied_entry) => {
                let result = *occupied_entry.get();
                *occupied_entry.get_mut() += 1;
                result + 1
            }
            Entry::Vacant(vacant_entry) => {
                vacant_entry.insert(0);
                0
            }
        };

        // generating the symbol ID for the member
        let id = engine
            .calculate_qualified_name_id(
                member_builder
                    .current_qualified_name
                    .iter()
                    .map(|x| &**x)
                    .chain(std::iter::once(&*info.name)),
                member_builder.current_id.target_id,
                Some(member_builder.current_id.id),
                count,
            )
            .await;

        match member_builder.member.insert(info.name.clone(), id) {
            // cool
            Insertion::Inserted => {}

            // a redefinition has been encountered
            Insertion::Conflicted(symbol_id) => {
                member_builder.redef_errors.push(
                    ItemRedefinition::builder()
                        .existing_id(member_builder.current_id.target_id.make_global(symbol_id))
                        .redefinition_span(info.span.expect("should have a span"))
                        .in_id(member_builder.current_id)
                        .build(),
                );
            }
        }

        // finally, insert the symbol information into the table
        self.insert_info(id, Some(member_builder.current_id.id), info, engine);

        member_builder.current_id.target_id.make_global(id)
    }

    pub async fn insert_unnamed_symbol(
        &mut self,
        member_builder: &mut MemberBuilder,
        info: Infos,
        engine: &TrackedEngine,
    ) -> GlobalSymbolID {
        let span = info.span.expect("an unnamed symbol should have a span");
        let id = engine.calculate_implements_id(&span, member_builder.current_id.target_id).await;

        member_builder.member.insert_unnamed(id);
        self.insert_info(id, Some(member_builder.current_id.id), info, engine);

        member_builder.current_id.target_id.make_global(id)
    }

    pub fn insert_symbol_members(
        &mut self,
        symbol_id: SymbolID,
        member: MemberBuilder,
        engine: &TrackedEngine,
    ) {
        self.members.insert(symbol_id, engine.intern(member.member));
        self.diagnostics.extend(member.redef_errors.into_iter().map(Diagnostic::ItemRedefinition));
    }
}

impl Table {
    pub fn all_symbol_ids(&self) -> impl Iterator<Item = SymbolID> + '_ {
        self.symbol_kinds.keys().copied()
    }

    /// Returns the IDs of the symbols of the given kind declared in this
    /// file.
    pub fn symbol_ids_of_kind(&self, kind: SymbolKind) -> impl Iterator<Item = SymbolID> + '_ {
        self.symbol_kinds
            .iter()
            .filter_map(move |(id, symbol_kind)| (*symbol_kind == kind).then_some(*id))
    }

    pub fn all_def_with_body_ids(&self) -> impl Iterator<Item = SymbolID> + '_ {
        self.symbol_kinds.iter().filter_map(|(id, kind)| kind.has_def_body().then_some(*id))
    }

    pub fn all_callable_def_ids(&self) -> impl Iterator<Item = SymbolID> + '_ {
        self.syntaxes.parameter_lists.keys().copied()
    }
}

/// Identifies the table of a single source file: the root file of a target or
/// the file of a file module.
///
/// The key carries everything needed to build the table from its file alone,
/// so that the table of a file module does not depend on the table of the
/// file declaring the module, and is reused when only the declaring file
/// changes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query)]
#[value(Arc<Table>)]
pub struct TableKey {
    target_id: TargetID,

    /// The path of the source file.
    path: Interned<Path>,

    /// The directory owned by the module, where its file modules are looked
    /// up.
    directory: Interned<Path>,

    /// The module whose members the file defines.
    module_id: SymbolID,

    /// The qualified name of the module, starting with the target name.
    qualified_name: Interned<[Interned<str>]>,
}

impl TableKey {
    /// Creates the key of the table of the root file of the given target.
    ///
    /// Like in Rust, the root module owns the directory containing the root
    /// file.
    pub(crate) async fn new_target_root(target_id: TargetID, engine: &TrackedEngine) -> Self {
        let arg = engine.get_invocation_arguments(target_id).await;
        let path: Interned<Path> = engine.intern_unsized(arg.file_path().to_path_buf());
        let directory =
            engine.intern_unsized(path.parent().unwrap_or_else(|| Path::new("")).to_path_buf());

        Self {
            target_id,
            path,
            directory,
            module_id: engine.get_target_root_module_id(target_id).await,
            qualified_name: engine.intern_unsized(vec![engine.intern_unsized(arg.target_name())]),
        }
    }

    /// Creates the key of the table of the file module `module_id`, named
    /// `qualified_name`, declared in a module owning the directory `directory`.
    ///
    /// Like in Rust, the file module `name` declared in a module owning the
    /// directory `dir` is loaded from `dir/name.ray` and owns the directory
    /// `dir/name`.
    pub(crate) fn new_file_module(
        target_id: TargetID,
        directory: &Path,
        module_id: SymbolID,
        qualified_name: Vec<Interned<str>>,
        engine: &TrackedEngine,
    ) -> Self {
        let name = qualified_name.last().expect("a file module has a name").clone();

        Self {
            target_id,
            path: engine.intern_unsized(
                directory.join(format!("{}.{SOURCE_FILE_EXTENSION}", name.as_ref())),
            ),
            directory: engine.intern_unsized(directory.join(name.as_ref())),
            module_id,
            qualified_name: engine.intern_unsized(qualified_name),
        }
    }

    /// Returns the target the file belongs to.
    #[must_use]
    pub const fn target_id(&self) -> TargetID { self.target_id }

    /// Returns the path of the source file.
    #[must_use]
    pub const fn path(&self) -> &Interned<Path> { &self.path }

    /// Returns whether the file is the root file of its target, which defines
    /// the root module. Any other file defines a file module declared in
    /// another file.
    #[must_use]
    pub fn is_target_root(&self) -> bool { self.qualified_name.len() == 1 }
}

/// Returns the table of the source file identified by `table_key`.
#[extend]
pub async fn get_table(self: &TrackedEngine, table_key: &TableKey) -> Arc<Table> {
    self.query(table_key).await
}

#[executor(config = Config, style = qbice::ExecutionStyle::Firewall)]
pub async fn table_executor(key: &TableKey, engine: &TrackedEngine) -> Arc<Table> {
    let mut table = Table::default();
    let mut member =
        MemberBuilder::new(key.target_id.make_global(key.module_id), key.qualified_name.to_vec());

    // register the members the file defines for its module
    if let Some(module_content) = table.load_source_file(key, engine).await {
        table
            .register_module_members(&mut member, module_content.members(), &key.directory, engine)
            .await;
    }

    // the root file declares the root module itself, whereas a file module is
    // declared in another file and only gets its members from this one
    if key.is_target_root() {
        table.insert_member_as_root_module(member, engine);
    } else {
        table.insert_symbol_members(key.module_id, member, engine);
    }

    Arc::new(table)
}

#[distributed_slice(RAY_PROGRAM)]
static TABLE_EXECUTOR: Registration<Config> = Registration::new::<TableKey, TableExecutor>();

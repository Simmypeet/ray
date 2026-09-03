use std::{collections::hash_map::Entry, path::Path, sync::Arc};

use bon::Builder;
use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_source_file::{LocalSourceID, get_stable_path_id};
use rayc_symbol::{
    GlobalSymbolID, SymbolID, calculate_qualified_name_id, get_target_root_module_id,
    member::{Insertion, Member},
    symbol_kind::SymbolKind,
};
use rayc_syntax::{
    def::{ParameterList, ReturnType},
    effect::TypeParameterList,
    effect_row::EffectRowAnnotation,
    path::Path as SyntaxPath,
    statement::Block,
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
    variadic: Option<bool>,
    type_parameters: Option<Option<TypeParameterList>>,
    instance_trait: Option<Option<SyntaxPath>>,
}

#[derive(Debug, Default, StableHash, Encode, Decode)]
struct SyntaxTable {
    parameter_lists: Map<Option<ParameterList>>,
    return_types: Map<Option<ReturnType>>,
    effect_rows: Map<Option<EffectRowAnnotation>>,
    def_bodies: Map<Option<Block>>,
    variadic_defs: Map<bool>,
    type_parameters: Map<Option<TypeParameterList>>,
    instance_traits: Map<Option<SyntaxPath>>,
}

/// Stores the symbol information. It maps the symbol ID to its related
/// information.
#[derive(Debug, Default, StableHash, Encode, Decode)]
pub struct Table {
    symbol_kinds: Map<SymbolKind>,
    members: Map<Interned<Member>>,
    parents: Map<Option<SymbolID>>,
    spans: Map<Option<RelativeSpan>>,
    names: Map<Interned<str>>,

    syntaxes: SyntaxTable,
    source_id: Option<LocalSourceID>,

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
    pub async fn new_root_module_id(
        target_id: TargetID,
        target_name: Interned<str>,
        engine: &TrackedEngine,
    ) -> Self {
        Self {
            current_id: target_id.make_global(engine.get_target_root_module_id(target_id).await),

            current_qualified_name: vec![target_name],
            member: Member::default(),
            occurrences: FxHashMap::default(),
            redef_errors: Vec::new(),
        }
    }
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
    pub fn is_variadic_def(&self, symbol_id: SymbolID) -> bool {
        self.syntaxes.variadic_defs.get(&symbol_id).copied().unwrap()
    }

    #[must_use]
    pub fn get_type_parameter_list_syntax(&self, symbol_id: SymbolID) -> Option<TypeParameterList> {
        self.syntaxes.type_parameters.get(&symbol_id).cloned().unwrap()
    }

    #[must_use]
    pub fn get_instance_trait_syntax(&self, symbol_id: SymbolID) -> Option<SyntaxPath> {
        self.syntaxes.instance_traits.get(&symbol_id).cloned().unwrap()
    }

    #[must_use]
    pub const fn source_id(&self) -> Option<LocalSourceID> { self.source_id }

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

        if let Some(variadic) = info.variadic {
            self.syntaxes.variadic_defs.insert(symbol_id, variadic);
        }

        if let Some(type_parameters) = info.type_parameters {
            self.syntaxes.type_parameters.insert(symbol_id, type_parameters);
        }

        if let Some(instance_trait) = info.instance_trait {
            self.syntaxes.instance_traits.insert(symbol_id, instance_trait);
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

    pub fn all_def_ids(&self) -> impl Iterator<Item = SymbolID> + '_ {
        self.symbol_kinds.iter().filter_map(|(id, kind)| (*kind == SymbolKind::Def).then_some(*id))
    }

    pub fn all_callable_def_ids(&self) -> impl Iterator<Item = SymbolID> + '_ {
        self.syntaxes.parameter_lists.keys().copied()
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Arc<Table>)]
#[extend(name = get_table, by_val)]
pub struct Key {
    pub target_id: TargetID,
}

#[executor(config = Config, style = qbice::ExecutionStyle::Firewall)]
pub async fn table_executor(&Key { target_id }: &Key, engine: &TrackedEngine) -> Arc<Table> {
    let mut table = Table::default();

    let arg = engine.get_invocation_arguments(target_id).await;
    let internred_path: Interned<Path> = engine.intern_unsized(arg.file_path().to_path_buf());

    let target_name = arg.target_name();

    let syntax_key =
        engine.query(&rayc_syntax::Key { path: internred_path.clone(), target_id }).await;

    let stable_path_id = engine.get_stable_path_id(internred_path.clone(), target_id).await.ok();

    let mut member =
        MemberBuilder::new_root_module_id(target_id, engine.intern_unsized(target_name), engine)
            .await;

    match syntax_key {
        Ok((Some(syntax), _)) => {
            table.register_module_members(&mut member, &syntax, engine).await;
        }

        Ok((None, _)) => {}

        Err(err) => {
            table.diagnostics.push(Diagnostic::SourceFileLoadFail(SourceFileLoadFail {
                error_message: err.to_string(),
                path: internred_path,
                submodule_span: None,
            }));
        }
    }

    table.source_id = stable_path_id;
    table.insert_member_as_root_module(member, engine);

    Arc::new(table)
}

#[distributed_slice(RAY_PROGRAM)]
static TABLE_EXECUTOR: Registration<Config> = Registration::new::<Key, TableExecutor>();

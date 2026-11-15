//! Contains the [`TableIndex`], which locates the table storing the
//! information of each symbol of a target.

use std::{path::Path, sync::Arc};

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_extend::extend;
use rayc_hash::FxHashMap;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_source_file::{GlobalSourceID, LocalSourceID};
use rayc_symbol::{GlobalSymbolID, SymbolID};
use rayc_target::TargetID;
use rayc_tokio::join_set::JoinSet;

use crate::table::{Table, TableKey, TableSummary, get_table};

/// Locates the information of the symbols of a target, which is spread over
/// the tables of its source files.
#[derive(Debug, Default, StableHash, Encode, Decode)]
pub struct TableIndex {
    /// The table storing the information of each symbol.
    symbol_tables: FxHashMap<SymbolID, Interned<TableKey>>,

    /// The tables of the target, sorted by their keys.
    table_keys: Vec<Interned<TableKey>>,

    /// The tables of the source files loaded into the target.
    source_files: FxHashMap<LocalSourceID, Interned<TableKey>>,
}

impl TableIndex {
    /// Returns the key of the table storing the information of the given
    /// symbol.
    #[must_use]
    pub fn symbol_table(&self, symbol_id: SymbolID) -> Option<&Interned<TableKey>> {
        self.symbol_tables.get(&symbol_id)
    }

    /// Returns the keys of the tables of every source file of the target.
    #[must_use]
    pub fn table_keys(&self) -> impl ExactSizeIterator<Item = &Interned<TableKey>> {
        self.table_keys.iter()
    }

    /// Returns the path of the loaded source file with the given ID, if the
    /// file belongs to the target.
    #[must_use]
    pub fn source_file_path(&self, source_id: LocalSourceID) -> Option<&Interned<Path>> {
        self.source_files.get(&source_id).map(|table_key| table_key.path())
    }

    /// Returns the module whose members the loaded source file with the given
    /// ID defines, if the file belongs to the target.
    #[must_use]
    pub fn source_file_module(&self, source_id: LocalSourceID) -> Option<SymbolID> {
        self.source_files.get(&source_id).map(|table_key| table_key.module_id())
    }

    /// Returns the IDs and paths of every source file loaded into the target.
    pub fn source_files(&self) -> impl Iterator<Item = (LocalSourceID, &Interned<Path>)> {
        self.source_files.iter().map(|(id, table_key)| (*id, table_key.path()))
    }

    /// Records the symbols and the source file of the given table.
    fn insert_table(&mut self, table_key: Interned<TableKey>, table: &TableSummary) {
        for symbol_id in table.all_symbol_ids() {
            self.symbol_tables.insert(symbol_id, table_key.clone());
        }

        if let Some(source_id) = table.source_id() {
            self.source_files.insert(source_id, table_key.clone());
        }

        self.table_keys.push(table_key);
    }

    /// Sorts the tables by their keys.
    ///
    /// The tables are recorded in the order their tasks finish, which varies
    /// from one run to another, whereas the index of a target must not.
    fn sort_tables(&mut self) { self.table_keys.sort_unstable(); }
}

/// A query for the [`TableIndex`] of a target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Arc<TableIndex>)]
#[extend(name = get_table_index, by_val)]
pub struct TableIndexKey {
    pub target_id: TargetID,
}

/// A projection of the table of a source file for its [`TableSummary`].
///
/// The table changes with any edit to its file, whereas the symbols it
/// stores rarely do: the index of the target reads the summaries, so that it
/// recomputes only when a symbol is added or removed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query)]
#[value(Arc<TableSummary>)]
struct TableSummaryKey {
    table_key: Interned<TableKey>,
}

#[executor(config = Config, style = qbice::ExecutionStyle::Projection)]
async fn table_summary_executor(
    TableSummaryKey { table_key }: &TableSummaryKey,
    engine: &TrackedEngine,
) -> Arc<TableSummary> {
    Arc::new(engine.get_table(table_key).await.summary())
}

#[distributed_slice(RAY_PROGRAM)]
static TABLE_SUMMARY_EXECUTOR: Registration<Config> =
    Registration::new::<TableSummaryKey, TableSummaryExecutor>();

/// Retrieves the summary of the table of the given key, handing the key back
/// so that the summary can be told apart from the ones retrieved by the other
/// tasks.
async fn load_table_summary(
    engine: TrackedEngine,
    table_key: Interned<TableKey>,
) -> (Interned<TableKey>, Arc<TableSummary>) {
    let summary = engine.query(&TableSummaryKey { table_key: table_key.clone() }).await;

    (table_key, summary)
}

#[executor(config = Config, style = qbice::ExecutionStyle::Firewall)]
async fn table_index_executor(
    &TableIndexKey { target_id }: &TableIndexKey,
    engine: &TrackedEngine,
) -> Arc<TableIndex> {
    let root_key = engine.intern(TableKey::new_target_root(target_id, engine).await);

    // retrieves the summaries of the tables concurrently, one task per source
    // file: the tables of the file modules are requested as soon as the table
    // of the file declaring them is available
    let mut index = TableIndex::default();
    let mut tasks = JoinSet::new();
    tasks.spawn(load_table_summary(engine.clone(), root_key));

    while let Some((table_key, summary)) = tasks.next().await {
        for next_table_key in summary.next_tables() {
            tasks.spawn(load_table_summary(engine.clone(), next_table_key.clone()));
        }

        index.insert_table(table_key, &summary);
    }

    index.sort_tables();

    Arc::new(index)
}

#[distributed_slice(RAY_PROGRAM)]
static TABLE_INDEX_EXECUTOR: Registration<Config> =
    Registration::new::<TableIndexKey, TableIndexExecutor>();

/// A projection of the [`TableIndex`] for the key of the table storing the
/// information of a single symbol, or `None` if no table stores it.
///
/// The index changes whenever a symbol is added or removed anywhere in the
/// target, whereas the table of a particular symbol rarely changes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<Interned<TableKey>>)]
struct SymbolTableKey {
    symbol_id: GlobalSymbolID,
}

#[executor(config = Config, style = qbice::ExecutionStyle::Projection)]
async fn symbol_table_executor(
    &SymbolTableKey { symbol_id }: &SymbolTableKey,
    engine: &TrackedEngine,
) -> Option<Interned<TableKey>> {
    engine.get_table_index(symbol_id.target_id).await.symbol_table(symbol_id.id).cloned()
}

#[distributed_slice(RAY_PROGRAM)]
static SYMBOL_TABLE_EXECUTOR: Registration<Config> =
    Registration::new::<SymbolTableKey, SymbolTableExecutor>();

/// Returns the table storing the information of the given symbol, or `None`
/// if no table stores it.
///
/// The projections of a symbol's information read the table through this,
/// so it must not panic for a symbol that has been removed.
#[extend]
pub async fn try_get_symbol_table(
    self: &TrackedEngine,
    symbol_id: GlobalSymbolID,
) -> Option<Arc<Table>> {
    let table_key = self.query(&SymbolTableKey { symbol_id }).await?;

    Some(self.get_table(&table_key).await)
}

/// Returns the module whose members the source file `source_id` defines, or
/// `None` if the file is not loaded into its target.
#[extend]
pub async fn get_source_file_module(
    self: &TrackedEngine,
    source_id: GlobalSourceID,
) -> Option<GlobalSymbolID> {
    let index = self.get_table_index(source_id.target_id).await;

    index
        .source_file_module(source_id.id)
        .map(|module_id| source_id.target_id.make_global(module_id))
}

/// Returns the summaries of the tables of every source file of the given
/// target.
#[extend]
pub async fn get_target_table_summaries(
    self: &TrackedEngine,
    target_id: TargetID,
) -> Vec<Arc<TableSummary>> {
    let index = self.get_table_index(target_id).await;

    let mut summaries = Vec::new();
    for table_key in index.table_keys() {
        summaries.push(self.query(&TableSummaryKey { table_key: table_key.clone() }).await);
    }

    summaries
}

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
use rayc_source_file::LocalSourceID;
use rayc_symbol::{GlobalSymbolID, SymbolID};
use rayc_target::TargetID;

use crate::table::{Table, TableKey, get_table};

/// Locates the information of the symbols of a target, which is spread over
/// the tables of its source files.
#[derive(Debug, Default, StableHash, Encode, Decode)]
pub struct TableIndex {
    /// The table storing the information of each symbol.
    symbol_tables: FxHashMap<SymbolID, Interned<TableKey>>,

    /// The tables of the target: the root file's first, followed by the
    /// tables of the file modules in declaration order, depth-first.
    table_keys: Vec<Interned<TableKey>>,

    /// The paths of the source files loaded into the target.
    source_files: FxHashMap<LocalSourceID, Interned<Path>>,
}

impl TableIndex {
    /// Returns the key of the table storing the information of the given
    /// symbol.
    #[must_use]
    pub fn symbol_table(&self, symbol_id: SymbolID) -> Option<&Interned<TableKey>> {
        self.symbol_tables.get(&symbol_id)
    }

    /// Returns the IDs of every symbol of the target.
    pub fn all_symbol_ids(&self) -> impl Iterator<Item = SymbolID> + '_ {
        self.symbol_tables.keys().copied()
    }

    /// Returns the keys of the tables of every source file of the target.
    pub fn table_keys(&self) -> impl Iterator<Item = &Interned<TableKey>> { self.table_keys.iter() }

    /// Returns the path of the loaded source file with the given ID, if the
    /// file belongs to the target.
    #[must_use]
    pub fn source_file_path(&self, source_id: LocalSourceID) -> Option<&Interned<Path>> {
        self.source_files.get(&source_id)
    }

    /// Returns the IDs and paths of every source file loaded into the target.
    pub fn source_files(&self) -> impl Iterator<Item = (LocalSourceID, &Interned<Path>)> {
        self.source_files.iter().map(|(id, path)| (*id, path))
    }

    /// Records the symbols and the source file of the given table.
    fn insert_table(&mut self, table_key: Interned<TableKey>, table: &Table) {
        for symbol_id in table.all_symbol_ids() {
            self.symbol_tables.insert(symbol_id, table_key.clone());
        }

        if let Some(source_id) = table.source_id() {
            self.source_files.insert(source_id, table_key.path().clone());
        }

        self.table_keys.push(table_key);
    }
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

#[executor(config = Config, style = qbice::ExecutionStyle::Firewall)]
async fn table_index_executor(
    &TableIndexKey { target_id }: &TableIndexKey,
    engine: &TrackedEngine,
) -> Arc<TableIndex> {
    let mut index = TableIndex::default();

    // follows the tables of the file modules from the root file, depth-first
    // and in declaration order
    let mut pending = vec![engine.intern(TableKey::new_target_root(target_id, engine).await)];
    while let Some(table_key) = pending.pop() {
        let table = engine.get_table(&table_key).await;

        pending.extend(table.next_tables().rev().cloned());
        index.insert_table(table_key, &table);
    }

    Arc::new(index)
}

#[distributed_slice(RAY_PROGRAM)]
static TABLE_INDEX_EXECUTOR: Registration<Config> =
    Registration::new::<TableIndexKey, TableIndexExecutor>();

/// A projection of the [`TableIndex`] for the key of the table storing the
/// information of a single symbol.
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

/// Returns the table storing the information of the given symbol.
#[extend]
pub async fn get_symbol_table(self: &TrackedEngine, symbol_id: GlobalSymbolID) -> Arc<Table> {
    let table_key = self
        .query(&SymbolTableKey { symbol_id })
        .await
        .unwrap_or_else(|| panic!("the symbol {symbol_id:?} is not in any table"));

    self.get_table(&table_key).await
}

/// Returns the tables of every source file of the given target.
#[extend]
pub async fn get_target_tables(self: &TrackedEngine, target_id: TargetID) -> Vec<Arc<Table>> {
    let index = self.get_table_index(target_id).await;

    let mut tables = Vec::new();
    for table_key in index.table_keys() {
        tables.push(self.get_table(table_key).await);
    }

    tables
}

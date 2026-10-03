//! Contains the definition of the [`SourceMap`] type, which is a collection
//! of source files that can be used when reporting diagnostics.
use std::{collections::HashMap, path::Path};

use linkme::distributed_slice;
use qbice::{executor, program::Registration, storage::intern::Interned};
use rayc_extend::extend;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_source_file::{FilePathKey, GlobalSourceID, SourceFile};
use rayc_target::TargetID;

use crate::index::get_table_index;

#[executor(config = Config)]
async fn file_path_executor(
    &FilePathKey { id }: &FilePathKey,
    engine: &TrackedEngine,
) -> Interned<Path> {
    engine
        .get_table_index(id.target_id)
        .await
        .source_file_path(id.id)
        .cloned()
        .expect("a source ID is only created for a source file loaded into its target")
}

#[distributed_slice(RAY_PROGRAM)]
static FILE_PATH_EXECUTOR: Registration<Config> =
    Registration::new::<FilePathKey, FilePathExecutor>();

/// A collection of source files that can be accessed by their global IDs.
///
/// This is used when reporting diagnostics to retrieve source file content.
#[derive(Debug, Clone)]
pub struct SourceMap(pub HashMap<GlobalSourceID, SourceFile>);

/// Creates a new [`SourceMap`] that will allow diagnostics to
/// retrieve source files by their IDs.
#[extend]
pub async fn create_source_map(self: &TrackedEngine, target_id: TargetID) -> SourceMap {
    let mut map = HashMap::new();
    for target_id in [target_id, TargetID::CORE] {
        let index = self.get_table_index(target_id).await;

        // every file loaded into the target has been read successfully
        for (id, path) in index.source_files() {
            if let Ok(file) =
                self.query(&rayc_source_file::Key { path: path.clone(), target_id }).await
            {
                map.insert(target_id.make_global(id), file);
            }
        }
    }
    SourceMap(map)
}

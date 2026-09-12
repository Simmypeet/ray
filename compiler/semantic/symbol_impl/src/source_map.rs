//! Contains the definition of the [`SourceMap`] type, which is a collection
//! of source files that can be used when reporting diagnostics.
use std::{collections::HashMap, path::Path};

use linkme::distributed_slice;
use qbice::{executor, program::Registration, storage::intern::Interned};
use rayc_extend::extend;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_source_file::{FilePathKey, GlobalSourceID, SourceFile, get_stable_path_id};
use rayc_target::{TargetID, get_invocation_arguments};

use crate::table;

#[executor(config = Config)]
async fn file_path_executor(
    &FilePathKey { id }: &FilePathKey,
    engine: &TrackedEngine,
) -> Interned<Path> {
    let table = engine.query(&table::Key { target_id: id.target_id }).await;

    if table.source_id() == Some(id.id) {
        let args = engine.get_invocation_arguments(id.target_id).await;

        engine.intern_unsized(args.file_path().to_path_buf())
    } else {
        todo!()
    }
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
        let args = self.get_invocation_arguments(target_id).await;
        let path: Interned<Path> = self.intern_unsized(args.file_path().to_path_buf());
        if let (Ok(file), Ok(id)) = (
            self.query(&rayc_source_file::Key { path: path.clone(), target_id }).await,
            self.get_stable_path_id(path, target_id).await,
        ) {
            map.insert(target_id.make_global(id), file);
        }
    }
    SourceMap(map)
}

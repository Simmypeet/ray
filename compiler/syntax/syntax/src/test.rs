use std::path::PathBuf;

use qbice::storage::intern::Interned;
use rayc_lexical::tree::Tree;
use rayc_qbice::DuplicatingInterner;
use rayc_source_file::{SourceFile, simple_source_map::SimpleSourceMap};
use rayc_target::TargetID;

pub fn parse_token_tree(source_code: &str) -> Tree {
    let source_map = SimpleSourceMap::new();
    let source_id = source_map.register(
        TargetID::TEST,
        SourceFile::from_str(source_code, Interned::new_duplicating_unsized(PathBuf::from("test"))),
    );
    let source = source_map.get(TargetID::TEST.make_global(source_id)).unwrap();

    Tree::from_source(
        &source,
        TargetID::TEST.make_global(source_id),
        &DuplicatingInterner,
        &rayc_handler::Panic,
    )
}

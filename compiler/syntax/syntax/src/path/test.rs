use std::path::PathBuf;

use qbice::storage::intern::Interned;
use rayc_lexical::tree::Tree;
use rayc_parser::abstract_tree::AbstractTree;
use rayc_qbice::DuplicatingInterner;
use rayc_source_file::{GlobalSourceID, LocalSourceID, SourceFile};
use rayc_target::TargetID;

use super::Path;

#[test]
fn qualified_path_with_type_arguments() {
    let source = SourceFile::from_str(
        "my_module.inner.Effect[int32, bool]",
        Interned::new_duplicating_unsized(PathBuf::from("test")),
    );
    let interner = DuplicatingInterner;
    let tree = Tree::from_source(
        &source,
        GlobalSourceID::new(TargetID::TEST, LocalSourceID::new(0, 0)),
        &interner,
        &rayc_handler::Panic,
    );

    let (path, errors) = Path::parse(&tree, &interner);

    assert!(errors.is_empty(), "{errors:#?}");
    let segments = path.unwrap().segments().collect::<Vec<_>>();
    assert_eq!(segments.len(), 3);
    assert_eq!(segments[0].identifier().unwrap().kind.0.as_ref(), "my_module");
    assert_eq!(segments[1].identifier().unwrap().kind.0.as_ref(), "inner");
    assert_eq!(segments[2].identifier().unwrap().kind.0.as_ref(), "Effect");
    assert_eq!(segments[2].type_arguments().unwrap().arguments().count(), 2);
}

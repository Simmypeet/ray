use std::path::PathBuf;

use qbice::storage::intern::Interned;
use rayc_lexical::tree::Tree;
use rayc_parser::abstract_tree::AbstractTree;
use rayc_qbice::DuplicatingInterner;
use rayc_source_file::{GlobalSourceID, LocalSourceID, SourceFile};
use rayc_target::TargetID;

use super::EffectRow;

fn parse(source: &str) -> EffectRow {
    let source =
        SourceFile::from_str(source, Interned::new_duplicating_unsized(PathBuf::from("test")));
    let interner = DuplicatingInterner;
    let tree = Tree::from_source(
        &source,
        GlobalSourceID::new(TargetID::TEST, LocalSourceID::new(0, 0)),
        &interner,
        &rayc_handler::Panic,
    );
    let (row, errors) = EffectRow::parse(&tree, &interner);

    assert!(errors.is_empty(), "{errors:#?}");
    row.unwrap()
}

#[test]
fn effect_rows() {
    let empty = parse("{}");
    assert_eq!(empty.effects().count(), 0);
    assert!(empty.tail().is_none());

    let closed = parse("{Console, State[int32], Console}");
    assert_eq!(closed.effects().count(), 3);
    assert!(closed.tail().is_none());

    let open = parse("{Console, State[int32] | e}");
    assert_eq!(open.effects().count(), 2);
    assert_eq!(open.tail().unwrap().variable().unwrap().kind.0.as_ref(), "e");

    let variable = parse("{| e}");
    assert_eq!(variable.effects().count(), 0);
    assert_eq!(variable.tail().unwrap().variable().unwrap().kind.0.as_ref(), "e");
}

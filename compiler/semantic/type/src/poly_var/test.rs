use qbice::storage::intern::Interned;
use rayc_arena::ID;
use rayc_lexical::tree::{Branch, OffsetMode, RelativeLocation, RelativeSpan};
use rayc_source_file::LocalSourceID;
use rayc_target::TargetID;

use super::{PolyVar, PolyVarMap};
use crate::ty::TyKind;

fn span() -> RelativeSpan {
    let location =
        RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ID::<Branch>::new(0) };

    RelativeSpan::new(location, location, TargetID::TEST.make_global(LocalSourceID::new(0, 0)))
}

// input: two declarations named `a`
// premise: polymorphic variable names are unique within one function
// output: one map entry and the same ID for both insertions
#[test]
fn duplicate_name_reuses_existing_id() {
    let mut map = PolyVarMap::new();
    let first =
        map.insert(PolyVar::new(Interned::new_duplicating_unsized("a"), TyKind::Star, span()));
    let second =
        map.insert(PolyVar::new(Interned::new_duplicating_unsized("a"), TyKind::Star, span()));

    assert_eq!(first, second);
    assert_eq!(map.len(), 1);
    assert_eq!(map.find_by_name("a"), Some(first));
}

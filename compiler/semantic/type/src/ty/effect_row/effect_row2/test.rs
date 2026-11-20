use std::{collections::HashMap, sync::Arc};

use qbice::storage::intern::Interned;
use rayc_lexical::tree::{OffsetMode, RelativeLocation, RelativeSpan};
use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor, TrackedEngine};
use rayc_symbol::{GlobalSymbolID, SymbolID};
use rayc_target::TargetID;

use super::{EffectAtom, EffectRow2, Entry, LabelOrType};
use crate::{
    poly_var::{GlobalPolyVarID, PolyVar, PolyVarMap},
    subst::{Subst, Substitutable},
    ty::{
        Integer, Mutability, Primitive, Ty, TyKind, args::Args, effect_row::EffectLabel,
        inference::Inference, lifetime::Lifetime,
    },
};

/// The polymorphic variables every test can refer to.
struct Fixture {
    /// A type variable `a`.
    a: GlobalPolyVarID,
    /// An effect row variable `r`.
    r: GlobalPolyVarID,
    /// An effect row variable `s`.
    s: GlobalPolyVarID,
}

/// Creates an engine whose single owner declares `a`, `r`, and `s`, so that
/// their kinds can be queried.
async fn fixture() -> (TrackedEngine, Fixture) {
    let mut engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    let owner = TargetID::TEST.make_global(SymbolID::from_u128(0));
    let location = RelativeLocation {
        offset: 0,
        mode: OffsetMode::Start,
        relative_to: rayc_arena::ID::new(0),
    };
    let span = RelativeSpan {
        start: location,
        end: location,
        source_id: TargetID::TEST.make_global(rayc_source_file::LocalSourceID::new(0, 0)),
    };
    let mut poly_vars = PolyVarMap::new();
    let a = poly_vars.insert(PolyVar::new_type(engine.intern_unsized("a"), span)).unwrap();
    let r = poly_vars.insert(PolyVar::new_effect2(engine.intern_unsized("r"), span)).unwrap();
    let s = poly_vars.insert(PolyVar::new_effect2(engine.intern_unsized("s"), span)).unwrap();
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        crate::poly_var::Key { symbol_id: owner },
        engine.intern(poly_vars),
    )]))));

    (Arc::new(engine).tracked().await, Fixture {
        a: GlobalPolyVarID::new(owner, a),
        r: GlobalPolyVarID::new(owner, r),
        s: GlobalPolyVarID::new(owner, s),
    })
}

fn effect_id(id: u128) -> GlobalSymbolID { TargetID::TEST.make_global(SymbolID::from_u128(id)) }

/// The label of effect `id` applied to `args`.
fn label(
    id: u128,
    args: impl IntoIterator<Item = Interned<Ty>>,
    engine: &TrackedEngine,
) -> EffectLabel {
    EffectLabel::new(effect_id(id), Args::new(args, engine))
}

fn int32(engine: &TrackedEngine) -> Interned<Ty> {
    Ty::new_primitive(Primitive::Integer(Integer::Int32), engine)
}

fn int32_ref(lifetime: Lifetime, engine: &TrackedEngine) -> Interned<Ty> {
    Ty::new_reference(
        Ty::new_lifetime(lifetime, engine),
        int32(engine),
        Mutability::Immutable,
        engine,
    )
}

fn poly(id: GlobalPolyVarID, engine: &TrackedEngine) -> Interned<Ty> {
    Ty::new_poly_var(id, engine)
}

/// Builds a row through the smart constructor.
async fn row(
    entries: impl IntoIterator<Item = (LabelOrType, Vec<EffectLabel>)>,
    engine: &TrackedEngine,
) -> Interned<Ty> {
    EffectRow2::new_effect_row(entries, engine).await
}

/// Normalizes `(source, excluded)` pairs into a multiset representation, so
/// comparisons ignore entry and exclusion order but still see duplicates.
fn normalized(
    entries: impl IntoIterator<Item = (Entry, Vec<EffectLabel>)>,
) -> Vec<(Entry, Vec<EffectLabel>)> {
    let mut entries = entries
        .into_iter()
        .map(|(source, mut excluded)| {
            excluded.sort();
            (source, excluded)
        })
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

/// The entries of a constructed row as `(source, excluded)` pairs,
/// independent of the row's internal order.
fn entries_of(ty: &Interned<Ty>) -> Vec<(Entry, Vec<EffectLabel>)> {
    let Ty::EffectRow2(row) = &**ty else {
        panic!("expected an effect row, found {ty:?}");
    };
    normalized(
        row.guarded_row_entries().map(|entry| (entry.source.clone(), entry.excluded.to_vec())),
    )
}

/// Asserts that `ty` is a row holding exactly `expected`, in any order.
#[track_caller]
fn assert_entries(
    ty: &Interned<Ty>,
    expected: impl IntoIterator<Item = (Entry, Vec<EffectLabel>)>,
) {
    assert_eq!(entries_of(ty), normalized(expected));
}

fn atom(ty: Interned<Ty>) -> Entry { Entry::Atom(EffectAtom(ty)) }

// input: {}
// premise: {}
// output: the empty row {}
#[tokio::test]
async fn empty_input_constructs_the_empty_row() {
    let (engine, _) = fixture().await;

    let result = row([], &engine).await;

    assert_entries(&result, []);
}

// input: {r}
// premise: r is an effect row variable
// output: r itself, not a row wrapping it
#[tokio::test]
async fn lone_unguarded_atom_collapses_to_the_atom() {
    let (engine, f) = fixture().await;
    let r = poly(f.r, &engine);

    let result = row([(LabelOrType::Type(r.clone()), vec![])], &engine).await;

    assert_eq!(result, r);
}

// input: {?0}
// premise: ?0 is an effect row inference variable
// output: ?0 itself
#[tokio::test]
async fn lone_unguarded_inference_atom_collapses_to_the_inference() {
    let (engine, _) = fixture().await;
    let inference = engine.intern(Ty::Inference(Inference::new(TyKind::EffectRow2, 0)));

    let result = row([(LabelOrType::Type(inference.clone()), vec![])], &engine).await;

    assert_eq!(result, inference);
}

// input: {r \ {F, E, F}}
// premise: r is an effect row variable
// output: {r \ {E, F}}; a guarded atom stays a row with deduplicated
//         exclusions
#[tokio::test]
async fn guarded_atom_keeps_deduplicated_exclusions() {
    let (engine, f) = fixture().await;
    let r = poly(f.r, &engine);
    let e = label(1, [], &engine);
    let f_ = label(2, [], &engine);

    let result =
        row([(LabelOrType::Type(r.clone()), vec![f_.clone(), e.clone(), f_.clone()])], &engine)
            .await;

    assert_entries(&result, [(atom(r), vec![e, f_])]);
}

// input: {r \ {E[&'static int32]}}
// premise: {}
// output: {r \ {E[&'_ int32]}}; exclusions are stored lifetime-erased
#[tokio::test]
async fn atom_exclusions_are_lifetime_erased() {
    let (engine, f) = fixture().await;
    let r = poly(f.r, &engine);

    let result = row(
        [(LabelOrType::Type(r.clone()), vec![label(
            1,
            [int32_ref(Lifetime::Static, &engine)],
            &engine,
        )])],
        &engine,
    )
    .await;

    assert_entries(&result, [(atom(r), vec![label(
        1,
        [int32_ref(Lifetime::Erased, &engine)],
        &engine,
    )])]);
}

// input: {E \ {F}}
// premise: E and F are distinct effects
// output: {E}; an exclusion apart from the label is dropped
#[tokio::test]
async fn label_drops_exclusion_apart_from_it() {
    let (engine, _) = fixture().await;
    let e = label(1, [], &engine);

    let result = row([(LabelOrType::Label(e.clone()), vec![label(2, [], &engine)])], &engine).await;

    assert_entries(&result, [(Entry::Label(e), vec![])]);
}

// input: {E[&'static int32] \ {E[&'_ int32]}, F}
// premise: the labels are the same up to lifetimes
// output: {F}; a label excluded by itself is removed from the row
#[tokio::test]
async fn label_excluded_by_the_same_label_up_to_lifetimes_is_removed() {
    let (engine, _) = fixture().await;
    let f_ = label(2, [], &engine);

    let result = row(
        [
            (LabelOrType::Label(label(1, [int32_ref(Lifetime::Static, &engine)], &engine)), vec![
                label(1, [int32_ref(Lifetime::Erased, &engine)], &engine),
            ]),
            (LabelOrType::Label(f_.clone()), vec![]),
        ],
        &engine,
    )
    .await;

    assert_entries(&result, [(Entry::Label(f_), vec![])]);
}

// input: {E[a] \ {E[int32]}}
// premise: a is a type variable, so E[a] and E[int32] may or may not coincide
// output: {E[a] \ {E[int32]}}; the blocked exclusion is kept
#[tokio::test]
async fn label_keeps_exclusion_blocked_on_a_poly_var() {
    let (engine, f) = fixture().await;
    let e_a = label(1, [poly(f.a, &engine)], &engine);
    let e_int = label(1, [int32(&engine)], &engine);

    let result = row([(LabelOrType::Label(e_a.clone()), vec![e_int.clone()])], &engine).await;

    assert_entries(&result, [(Entry::Label(e_a), vec![e_int])]);
}

// input: {E[?0] \ {E[int32]}}
// premise: ?0 is an unresolved type inference variable
// output: {E[?0] \ {E[int32]}}; the exclusion waits for inference
#[tokio::test]
async fn label_keeps_exclusion_waiting_for_inference() {
    let (engine, _) = fixture().await;
    let inference = engine.intern(Ty::Inference(Inference::new(TyKind::Star, 0)));
    let e_infer = label(1, [inference], &engine);
    let e_int = label(1, [int32(&engine)], &engine);

    let result = row([(LabelOrType::Label(e_infer.clone()), vec![e_int.clone()])], &engine).await;

    assert_entries(&result, [(Entry::Label(e_infer), vec![e_int])]);
}

// input: {F, r, E, F} and {E, r, F}
// premise: {}
// output: the same interned row; construction is order-insensitive and
//         deduplicates entries
#[tokio::test]
async fn construction_is_canonical_across_order_and_duplicates() {
    let (engine, f) = fixture().await;
    let r = poly(f.r, &engine);
    let e = LabelOrType::Label(label(1, [], &engine));
    let f_ = LabelOrType::Label(label(2, [], &engine));

    let left = row(
        [
            (f_.clone(), vec![]),
            (LabelOrType::Type(r.clone()), vec![]),
            (e.clone(), vec![]),
            (f_.clone(), vec![]),
        ],
        &engine,
    )
    .await;
    let right = row([(e, vec![]), (LabelOrType::Type(r), vec![]), (f_, vec![])], &engine).await;

    assert_eq!(left, right);
    assert_eq!(entries_of(&left).len(), 3);
}

// input: {E[&'static int32], E[&'_ int32]}
// premise: the labels differ only in lifetimes
// output: a single E[&_ int32] entry, up to its lifetime
#[tokio::test]
async fn labels_equal_up_to_lifetimes_are_deduplicated() {
    let (engine, _) = fixture().await;

    let result = row(
        [
            (LabelOrType::Label(label(1, [int32_ref(Lifetime::Static, &engine)], &engine)), vec![]),
            (LabelOrType::Label(label(1, [int32_ref(Lifetime::Erased, &engine)], &engine)), vec![]),
        ],
        &engine,
    )
    .await;

    let entries = entries_of(&result);
    assert_eq!(entries.len(), 1);
    let (Entry::Label(kept), excluded) = &entries[0] else {
        panic!("expected a label entry, found {:?}", entries[0]);
    };
    assert!(excluded.is_empty());
    assert_eq!(
        crate::ty::inspect::LifetimeErasable::erase_lifetime(kept, &engine).await,
        label(1, [int32_ref(Lifetime::Erased, &engine)], &engine)
    );
}

// input: {r \ {E}, r, E}
// premise: r subsumes r \ {E}, since it excludes fewer labels
// output: {E, r}; the subsumed entry is removed
#[tokio::test]
async fn subsumed_guarded_entry_is_removed() {
    let (engine, f) = fixture().await;
    let r = poly(f.r, &engine);
    let e = label(1, [], &engine);

    let result = row(
        [
            (LabelOrType::Type(r.clone()), vec![e.clone()]),
            (LabelOrType::Type(r.clone()), vec![]),
            (LabelOrType::Label(e.clone()), vec![]),
        ],
        &engine,
    )
    .await;

    assert_entries(&result, vec![(Entry::Label(e), vec![]), (atom(r), vec![])]);
}

// input: {r \ {E}, r}
// premise: r subsumes r \ {E}
// output: r itself, after the subsumed entry is removed
#[tokio::test]
async fn subsumption_can_collapse_the_row_to_an_atom() {
    let (engine, f) = fixture().await;
    let r = poly(f.r, &engine);

    let result = row(
        [
            (LabelOrType::Type(r.clone()), vec![label(1, [], &engine)]),
            (LabelOrType::Type(r.clone()), vec![]),
        ],
        &engine,
    )
    .await;

    assert_eq!(result, r);
}

// input: {{E, F, r \ {G}} \ {E}}
// premise: the outer exclusion applies to every inner entry
// output: {F, r \ {E, G}}; the nested row is flattened and E is removed
#[tokio::test]
async fn nested_row_is_flattened_under_outer_exclusions() {
    let (engine, f) = fixture().await;
    let r = poly(f.r, &engine);
    let e = label(1, [], &engine);
    let f_ = label(2, [], &engine);
    let g = label(3, [], &engine);
    let inner = row(
        [
            (LabelOrType::Label(e.clone()), vec![]),
            (LabelOrType::Label(f_.clone()), vec![]),
            (LabelOrType::Type(r.clone()), vec![g.clone()]),
        ],
        &engine,
    )
    .await;

    let result = row([(LabelOrType::Type(inner), vec![e.clone()])], &engine).await;
    let expected =
        row([(LabelOrType::Label(f_), vec![]), (LabelOrType::Type(r), vec![e, g])], &engine).await;

    assert_eq!(result, expected);
}

// input: {E[a] \ {E[int32]}, F}
// premise: a := int32
// output: {F}; the instantiated label is excluded by itself
#[tokio::test]
async fn substitution_removes_label_that_becomes_excluded() {
    let (engine, f) = fixture().await;
    let f_ = label(2, [], &engine);
    let generic = row(
        [
            (LabelOrType::Label(label(1, [poly(f.a, &engine)], &engine)), vec![label(
                1,
                [int32(&engine)],
                &engine,
            )]),
            (LabelOrType::Label(f_.clone()), vec![]),
        ],
        &engine,
    )
    .await;

    let result =
        generic.apply_subst_or_clone(&Subst::new_singleton(f.a, int32(&engine)), &engine).await;

    assert_eq!(result, row([(LabelOrType::Label(f_), vec![])], &engine).await);
}

// input: {E[a] \ {E[int32]}}
// premise: a := bool
// output: {E[bool]}; the exclusion becomes apart and is dropped
#[tokio::test]
async fn substitution_drops_exclusion_that_becomes_apart() {
    let (engine, f) = fixture().await;
    let bool_ty = Ty::new_primitive(Primitive::Bool, &engine);
    let generic = row(
        [(LabelOrType::Label(label(1, [poly(f.a, &engine)], &engine)), vec![label(
            1,
            [int32(&engine)],
            &engine,
        )])],
        &engine,
    )
    .await;

    let result =
        generic.apply_subst_or_clone(&Subst::new_singleton(f.a, bool_ty.clone()), &engine).await;

    assert_entries(&result, [(Entry::Label(label(1, [bool_ty], &engine)), vec![])]);
}

// input: {G, r \ {E}}
// premise: r := {E, F, s}
// output: {F, G, s \ {E}}; the substituted row is flattened under r's
//         exclusions
#[tokio::test]
async fn substitution_flattens_row_into_atom_position() {
    let (engine, f) = fixture().await;
    let e = label(1, [], &engine);
    let f_ = label(2, [], &engine);
    let g = label(3, [], &engine);
    let s = poly(f.s, &engine);
    let generic = row(
        [
            (LabelOrType::Label(g.clone()), vec![]),
            (LabelOrType::Type(poly(f.r, &engine)), vec![e.clone()]),
        ],
        &engine,
    )
    .await;
    let replacement = row(
        [
            (LabelOrType::Label(e.clone()), vec![]),
            (LabelOrType::Label(f_.clone()), vec![]),
            (LabelOrType::Type(s.clone()), vec![]),
        ],
        &engine,
    )
    .await;

    let result =
        generic.apply_subst_or_clone(&Subst::new_singleton(f.r, replacement), &engine).await;
    let expected = row(
        [
            (LabelOrType::Label(f_), vec![]),
            (LabelOrType::Label(g), vec![]),
            (LabelOrType::Type(s), vec![e]),
        ],
        &engine,
    )
    .await;

    assert_eq!(result, expected);
}

// input: {r, s}
// premise: r := s
// output: s itself; the duplicate atoms merge and the row collapses
#[tokio::test]
async fn substitution_merging_atoms_collapses_to_the_atom() {
    let (engine, f) = fixture().await;
    let s = poly(f.s, &engine);
    let generic = row(
        [(LabelOrType::Type(poly(f.r, &engine)), vec![]), (LabelOrType::Type(s.clone()), vec![])],
        &engine,
    )
    .await;

    let result = generic.apply_subst_or_clone(&Subst::new_singleton(f.r, s.clone()), &engine).await;

    assert_eq!(result, s);
}

// input: {E, r}
// premise: r := {}
// output: {E}; substituting the empty row removes the atom
#[tokio::test]
async fn substitution_with_empty_row_removes_the_atom() {
    let (engine, f) = fixture().await;
    let e = label(1, [], &engine);
    let generic = row(
        [(LabelOrType::Label(e.clone()), vec![]), (LabelOrType::Type(poly(f.r, &engine)), vec![])],
        &engine,
    )
    .await;

    let result = generic
        .apply_subst_or_clone(&Subst::new_singleton(f.r, row([], &engine).await), &engine)
        .await;

    assert_entries(&result, [(Entry::Label(e), vec![])]);
}

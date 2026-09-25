use std::{collections::HashMap, sync::Arc};

use qbice::storage::intern::Interned;
use rayc_lexical::tree::{OffsetMode, RelativeLocation, RelativeSpan};
use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor, TrackedEngine};
use rayc_symbol::{GlobalSymbolID, SymbolID};
use rayc_target::TargetID;
use rayc_type::{
    poly_var::{GlobalPolyVarID, PolyVar, PolyVarMap},
    trait_ref::TraitRef,
    ty::{Mutability, Primitive, Ty, args::Args, lifetime::Lifetime},
    where_clause::{OutlivesPredicate, PredicateKind},
};

use super::ImpliedBoundsKey;
use crate::Solver;

/// A test site with: lifetimes `'a`, `'b`, `'c`, a
/// type `t`, and a dictionary `d: Project[t]`.
struct Fixture {
    engine: TrackedEngine,
    symbol_id: GlobalSymbolID,
    lt_a: Interned<Ty>,
    lt_b: Interned<Ty>,
    lt_c: Interned<Ty>,
    ty_t: Interned<Ty>,
    dict_d: Interned<Ty>,
}

impl Fixture {
    async fn new() -> Self {
        let mut engine = Engine::new_with(
            qbice::serialize::Plugin::default(),
            InMemoryFactory,
            qbice::stable_hash::SeededStableHasherBuilder::new(0),
        )
        .await
        .unwrap();
        let symbol_id = TargetID::TEST.make_global(SymbolID::from_u128(0));
        let project = TargetID::TEST.make_global(SymbolID::from_u128(1));
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

        // Declare the site's variables.
        let mut poly_vars = PolyVarMap::new();
        let mut insert = |poly_var| {
            let id = poly_vars.insert(poly_var).unwrap();
            engine.intern(Ty::PolyVar(GlobalPolyVarID::new(symbol_id, id)))
        };
        let lt_a = insert(PolyVar::new_lifetime(engine.intern_unsized("'a"), span));
        let lt_b = insert(PolyVar::new_lifetime(engine.intern_unsized("'b"), span));
        let lt_c = insert(PolyVar::new_lifetime(engine.intern_unsized("'c"), span));
        let ty_t = insert(PolyVar::new_type(engine.intern_unsized("t"), span));
        let trait_ref =
            TraitRef::new(project, Args::new_with_args(engine.intern_unsized([ty_t.clone()])));
        let dict_d = insert(PolyVar::new_instance(engine.intern_unsized("d"), trait_ref, span));

        // The site implies no bounds, so only the givens are facts.
        engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
            rayc_type::poly_var::Key { symbol_id },
            engine.intern(poly_vars),
        )]))));
        engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
            ImpliedBoundsKey { site: symbol_id },
            engine.intern_unsized(Vec::<OutlivesPredicate>::new()),
        )]))));

        Self { engine: Arc::new(engine).tracked().await, symbol_id, lt_a, lt_b, lt_c, ty_t, dict_d }
    }

    fn solver(&self, facts: impl IntoIterator<Item = OutlivesPredicate>) -> Solver {
        Solver::with_givens(
            self.engine.clone(),
            self.symbol_id,
            facts.into_iter().map(PredicateKind::Outlives),
        )
    }

    fn static_lifetime(&self) -> Interned<Ty> { Ty::new_lifetime(Lifetime::Static, &self.engine) }

    /// `Project.Out` projected from the dictionary `d`.
    fn projection(&self) -> Interned<Ty> {
        let out = TargetID::TEST.make_global(SymbolID::from_u128(2));
        Ty::new_instance_associated(out, self.dict_d.clone(), [], &self.engine)
    }
}

fn region(longer: &Interned<Ty>, shorter: &Interned<Ty>) -> OutlivesPredicate {
    OutlivesPredicate::Region { longer: longer.clone(), shorter: shorter.clone() }
}

fn type_outlives(ty: &Interned<Ty>, bound: &Interned<Ty>) -> OutlivesPredicate {
    OutlivesPredicate::Type { ty: ty.clone(), bound: bound.clone() }
}

// input: 'a: 'c
// premise: 'a: 'b, 'b: 'c
// output: true
#[tokio::test]
async fn region_outlives_is_transitive() {
    let fixture = Fixture::new().await;
    let mut solver = fixture
        .solver([region(&fixture.lt_a, &fixture.lt_b), region(&fixture.lt_b, &fixture.lt_c)]);

    assert!(solver.entails_outlives(&region(&fixture.lt_a, &fixture.lt_c)).await);
}

// input: 'c: 'a
// premise: 'a: 'b, 'b: 'c
// output: false
#[tokio::test]
async fn region_outlives_does_not_hold_in_reverse() {
    let fixture = Fixture::new().await;
    let mut solver = fixture
        .solver([region(&fixture.lt_a, &fixture.lt_b), region(&fixture.lt_b, &fixture.lt_c)]);

    assert!(!solver.entails_outlives(&region(&fixture.lt_c, &fixture.lt_a)).await);
}

// input: 'static: 'a
// premise: {}
// output: true
#[tokio::test]
async fn static_outlives_every_lifetime() {
    let fixture = Fixture::new().await;
    let mut solver = fixture.solver([]);

    assert!(solver.entails_outlives(&region(&fixture.static_lifetime(), &fixture.lt_a)).await);
}

// input: 'a: 'b
// premise: 'a: 'static
// output: true
#[tokio::test]
async fn lifetime_outliving_static_outlives_every_lifetime() {
    let fixture = Fixture::new().await;
    let mut solver = fixture.solver([region(&fixture.lt_a, &fixture.static_lifetime())]);

    assert!(solver.entails_outlives(&region(&fixture.lt_a, &fixture.lt_b)).await);
}

// input: t: 'a
// premise: t: 'b, 'b: 'a
// output: true
#[tokio::test]
async fn type_parameter_outlives_through_region_chain() {
    let fixture = Fixture::new().await;
    let mut solver = fixture.solver([
        type_outlives(&fixture.ty_t, &fixture.lt_b),
        region(&fixture.lt_b, &fixture.lt_a),
    ]);

    assert!(solver.entails_outlives(&type_outlives(&fixture.ty_t, &fixture.lt_a)).await);
}

// input: t: 'a
// premise: 'b: 'a
// output: false
#[tokio::test]
async fn type_parameter_needs_a_fact() {
    let fixture = Fixture::new().await;
    let mut solver = fixture.solver([region(&fixture.lt_b, &fixture.lt_a)]);

    assert!(!solver.entails_outlives(&type_outlives(&fixture.ty_t, &fixture.lt_a)).await);
}

// input: (&'b t, int32): 'a
// premise: 'b: 'a
// output: false, since t: 'a is unknown
#[tokio::test]
async fn type_outlives_requires_every_component() {
    let fixture = Fixture::new().await;
    let reference = Ty::new_reference(
        fixture.lt_b.clone(),
        fixture.ty_t.clone(),
        Mutability::Immutable,
        &fixture.engine,
    );
    let int32 = Ty::new_primitive(Primitive::Int32, &fixture.engine);
    let tuple = Ty::new_tuple(fixture.engine.intern_unsized([reference, int32]), &fixture.engine);
    let mut solver = fixture.solver([region(&fixture.lt_b, &fixture.lt_a)]);

    assert!(!solver.entails_outlives(&type_outlives(&tuple, &fixture.lt_a)).await);
}

// input: 'b: 'c
// premise: &'a &'b int32: 'c
// output: true, since the fact decomposes to 'a: 'c and 'b: 'c
#[tokio::test]
async fn type_fact_decomposes_into_its_components() {
    let fixture = Fixture::new().await;
    let int32 = Ty::new_primitive(Primitive::Int32, &fixture.engine);
    let inner =
        Ty::new_reference(fixture.lt_b.clone(), int32, Mutability::Immutable, &fixture.engine);
    let outer =
        Ty::new_reference(fixture.lt_a.clone(), inner, Mutability::Immutable, &fixture.engine);
    let mut solver = fixture.solver([type_outlives(&outer, &fixture.lt_c)]);

    assert!(solver.entails_outlives(&region(&fixture.lt_b, &fixture.lt_c)).await);
}

// input: d.Out: 'a
// premise: d: Project[t], t: 'a
// output: true, since everything the projection comes from outlives 'a
#[tokio::test]
async fn projection_outlives_when_its_trait_arguments_do() {
    let fixture = Fixture::new().await;
    let mut solver = fixture.solver([type_outlives(&fixture.ty_t, &fixture.lt_a)]);

    assert!(solver.entails_outlives(&type_outlives(&fixture.projection(), &fixture.lt_a)).await);
}

// input: d.Out: 'a
// premise: d: Project[t], d.Out: 'b, 'b: 'a
// output: true, from the projection fact
#[tokio::test]
async fn projection_outlives_from_a_fact() {
    let fixture = Fixture::new().await;
    let mut solver = fixture.solver([
        type_outlives(&fixture.projection(), &fixture.lt_b),
        region(&fixture.lt_b, &fixture.lt_a),
    ]);

    assert!(solver.entails_outlives(&type_outlives(&fixture.projection(), &fixture.lt_a)).await);
}

// input: t: '_
// premise: {}
// output: true, since erased lifetimes are checked on the IR
#[tokio::test]
async fn erased_bound_is_always_satisfied() {
    let fixture = Fixture::new().await;
    let erased = Ty::new_lifetime(Lifetime::Erased, &fixture.engine);
    let mut solver = fixture.solver([]);

    assert!(solver.entails_outlives(&type_outlives(&fixture.ty_t, &erased)).await);
}

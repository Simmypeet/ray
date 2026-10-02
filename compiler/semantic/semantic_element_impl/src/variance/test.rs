use std::{collections::HashMap, sync::Arc};

use Variance::{Bivariant, Contravariant, Covariant, Invariant};
use qbice::storage::intern::Interned;
use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor, TrackedEngine};
use rayc_semantic_element::{
    parameter::{Key as ParameterKey, Parameter, ParameterMap},
    return_type::Key as ReturnTypeKey,
    struct_body::{Field, Key as StructBodyKey, StructBody},
};
use rayc_source_file::GlobalSourceID;
use rayc_symbol::{
    GlobalSymbolID, SymbolID,
    member::{Key as MemberKey, Member},
    symbol_kind::{AllEffectIDs, AllNominalTypeIDs},
};
use rayc_target::TargetID;
use rayc_type::{
    poly_var::{GlobalPolyVarID, Key as PolyVarKey, PolyVar, PolyVarMap},
    ty::{Integer, Mutability, Primitive, Ty, args::Args},
    variance::{Variance, VarianceMap, get_variance},
};

use super::{VarianceExecutor, VariancesExecutor};

/// Returns the variances of a map in the order of its poly var map.
fn variances(map: &VarianceMap) -> Vec<Variance> { map.iter().collect() }

fn span() -> RelativeSpan {
    let location =
        RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ROOT_BRANCH_ID };
    RelativeSpan { start: location, end: location, source_id: GlobalSourceID::default() }
}

/// The declarations of one test target, registered as precomputed queries.
struct Declarations {
    engine: Arc<Engine>,
    tracked: TrackedEngine,
    next_id: u128,
    poly_vars: HashMap<PolyVarKey, Interned<PolyVarMap>>,
    struct_bodies: HashMap<StructBodyKey, Interned<StructBody>>,
    members: HashMap<MemberKey, Interned<Member>>,
    parameters: HashMap<ParameterKey, Interned<ParameterMap>>,
    return_types: HashMap<ReturnTypeKey, Interned<Ty>>,
    structs: Vec<SymbolID>,
    effects: Vec<SymbolID>,
}

impl Declarations {
    async fn new() -> Self {
        let engine = Engine::new_with(
            qbice::serialize::Plugin::default(),
            InMemoryFactory,
            qbice::stable_hash::SeededStableHasherBuilder::new(0),
        )
        .await
        .unwrap();
        let engine = Arc::new(engine);
        let tracked = engine.clone().tracked().await;
        Self {
            engine,
            tracked,
            next_id: 1,
            poly_vars: HashMap::new(),
            struct_bodies: HashMap::new(),
            members: HashMap::new(),
            parameters: HashMap::new(),
            return_types: HashMap::new(),
            structs: Vec::new(),
            effects: Vec::new(),
        }
    }

    const fn engine(&self) -> &TrackedEngine { &self.tracked }

    fn fresh_id(&mut self) -> GlobalSymbolID {
        let id = TargetID::TEST.make_global(SymbolID::from_u128(self.next_id));
        self.next_id += 1;
        id
    }

    /// Declares a symbol whose parameters are named `names`, where a name
    /// starting with `'` is a lifetime, and a leading `+`, `-` or `=`
    /// declares the variance. Returns the symbol and its parameters.
    fn declare<const N: usize>(
        &mut self,
        names: [&'static str; N],
    ) -> (GlobalSymbolID, [Interned<Ty>; N]) {
        let symbol_id = self.fresh_id();
        let mut poly_vars = PolyVarMap::new();
        let ids = names.map(|name| {
            let (declared, name) = match name.split_at(1) {
                ("+", rest) => (Some(Covariant), rest),
                ("-", rest) => (Some(Contravariant), rest),
                ("=", rest) => (Some(Invariant), rest),
                _ => (None, name),
            };
            let name_str = self.tracked.intern_unsized(name);
            let mut poly_var = if name.starts_with('\'') {
                PolyVar::new_lifetime(name_str, span())
            } else {
                PolyVar::new_type(name_str, span())
            };
            if let Some(declared) = declared {
                poly_var = poly_var.with_declared_variance(declared);
            }
            poly_vars.insert(poly_var).unwrap()
        });
        self.poly_vars.insert(PolyVarKey { symbol_id }, self.tracked.intern(poly_vars));
        let params =
            ids.map(|id| self.tracked.intern(Ty::PolyVar(GlobalPolyVarID::new(symbol_id, id))));
        (symbol_id, params)
    }

    fn define_struct(
        &mut self,
        symbol_id: GlobalSymbolID,
        fields: impl IntoIterator<Item = Interned<Ty>>,
    ) {
        let mut body = StructBody::new();
        for (index, ty) in fields.into_iter().enumerate() {
            let name = self.tracked.intern_unsized(format!("field{index}"));
            body.insert(Field::builder().name(name).span(span()).ty(ty).build()).unwrap();
        }
        self.struct_bodies.insert(StructBodyKey { symbol_id }, self.tracked.intern(body));
        self.structs.push(symbol_id.id);
    }

    /// Defines an effect from its operations, each given as its parameter
    /// types and its return type.
    fn define_effect(
        &mut self,
        symbol_id: GlobalSymbolID,
        operations: impl IntoIterator<Item = (Vec<Interned<Ty>>, Interned<Ty>)>,
    ) {
        let mut members = Member::default();
        for (index, (parameter_types, return_type)) in operations.into_iter().enumerate() {
            let operation_id = self.fresh_id();
            let name = self.tracked.intern_unsized(format!("operation{index}"));
            let _ = members.insert(name, operation_id.id);

            let mut parameters = ParameterMap::new();
            for ty in parameter_types {
                parameters.push(Parameter::builder().ty(ty).build());
            }
            self.parameters
                .insert(ParameterKey { symbol_id: operation_id }, self.tracked.intern(parameters));
            self.return_types.insert(ReturnTypeKey { symbol_id: operation_id }, return_type);
        }
        self.members.insert(MemberKey { symbol_id }, self.tracked.intern(members));
        self.effects.push(symbol_id.id);
    }

    fn reference(
        &self,
        lifetime: &Interned<Ty>,
        pointee: &Interned<Ty>,
        mutability: Mutability,
    ) -> Interned<Ty> {
        Ty::new_reference(lifetime.clone(), pointee.clone(), mutability, &self.tracked)
    }

    fn pointer(&self, pointee: &Interned<Ty>, mutability: Mutability) -> Interned<Ty> {
        Ty::new_pointer(pointee.clone(), mutability, &self.tracked)
    }

    fn int32(&self) -> Interned<Ty> {
        Ty::new_primitive(Primitive::Integer(Integer::Int32), &self.tracked)
    }

    fn structure<const N: usize>(
        &self,
        symbol_id: GlobalSymbolID,
        args: [&Interned<Ty>; N],
    ) -> Interned<Ty> {
        Ty::new_struct(symbol_id, Args::new(args.map(Clone::clone), &self.tracked), &self.tracked)
    }

    /// Registers every declaration and the executors under test.
    async fn finish(self) -> TrackedEngine {
        let Self {
            mut engine,
            tracked,
            poly_vars,
            struct_bodies,
            members,
            parameters,
            return_types,
            structs,
            effects,
            ..
        } = self;
        drop(tracked);

        let target = TargetID::TEST;
        let engine_mut = Arc::get_mut(&mut engine).unwrap();
        engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
            AllNominalTypeIDs { target },
            structs.into(),
        )]))));
        engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
            AllEffectIDs { target },
            effects.into(),
        )]))));
        engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(poly_vars)));
        engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(struct_bodies)));
        engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(members)));
        engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(parameters)));
        engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(return_types)));
        engine_mut.register_executor(Arc::new(VariancesExecutor));
        engine_mut.register_executor(Arc::new(VarianceExecutor));

        engine.tracked().await
    }
}

// input: struct S['a, 'b, t, u]: (&'a t, &'b mut u)
// premise: {}
// output: ['a Covariant, 'b Covariant, t Covariant, u Invariant]
#[tokio::test]
async fn reference_positions_give_struct_variances() {
    let mut declarations = Declarations::new().await;
    let (shared, [a, b, t, u]) = declarations.declare(["'a", "'b", "t", "u"]);
    let fields = [
        declarations.reference(&a, &t, Mutability::Immutable),
        declarations.reference(&b, &u, Mutability::Mutable),
    ];
    declarations.define_struct(shared, fields);

    let engine = declarations.finish().await;
    assert_eq!(variances(&*engine.get_variance(shared).await), [
        Covariant, Covariant, Covariant, Invariant
    ]);
}

// input: struct Outer['a, t]: Inner['a, t], and
//        struct Inner['a, t]: &'a mut t
// premise: {}
// output: Outer is ['a Covariant, t Invariant], as Inner is
#[tokio::test]
async fn mutable_reference_inside_struct_is_invariant() {
    let mut declarations = Declarations::new().await;
    let (inner, [inner_a, inner_t]) = declarations.declare(["'a", "t"]);
    let (outer, [outer_a, outer_t]) = declarations.declare(["'a", "t"]);
    let inner_field = declarations.reference(&inner_a, &inner_t, Mutability::Mutable);
    declarations.define_struct(inner, [inner_field]);
    let outer_field = declarations.structure(inner, [&outer_a, &outer_t]);
    declarations.define_struct(outer, [outer_field]);

    let engine = declarations.finish().await;
    assert_eq!(variances(&*engine.get_variance(outer).await), [Covariant, Invariant]);
}

// input: struct List['a, 'n, t]: (&'n t, &'n List['a, 'n, t])
// premise: {}
// output: ['a Bivariant, 'n Covariant, t Covariant]; the recursion alone does
//         not use 'a
#[tokio::test]
async fn recursion_alone_does_not_use_a_parameter() {
    let mut declarations = Declarations::new().await;
    let (list, [a, n, t]) = declarations.declare(["'a", "'n", "t"]);
    let list_ty = declarations.structure(list, [&a, &n, &t]);
    let fields = [
        declarations.reference(&n, &t, Mutability::Immutable),
        declarations.reference(&n, &list_ty, Mutability::Immutable),
    ];
    declarations.define_struct(list, fields);

    let engine = declarations.finish().await;
    assert_eq!(variances(&*engine.get_variance(list).await), [Bivariant, Covariant, Covariant]);
}

// input: struct Even['a, 'b, 'n]: (&'a int32, &'n Odd['a, 'b, 'n]), and
//        struct Odd['a, 'b, 'n]: (&'n mut Even['a, 'b, 'n], &'b int32)
// premise: {}
// output: both are ['a Invariant, 'b Invariant, 'n Invariant]
#[tokio::test]
async fn invariance_propagates_through_mutual_recursion() {
    let mut declarations = Declarations::new().await;
    let (even, [even_a, even_b, even_n]) = declarations.declare(["'a", "'b", "'n"]);
    let (odd, [odd_a, odd_b, odd_n]) = declarations.declare(["'a", "'b", "'n"]);
    let int32 = declarations.int32();

    let odd_ty = declarations.structure(odd, [&even_a, &even_b, &even_n]);
    let even_fields = [
        declarations.reference(&even_a, &int32, Mutability::Immutable),
        declarations.reference(&even_n, &odd_ty, Mutability::Immutable),
    ];
    declarations.define_struct(even, even_fields);

    let even_ty = declarations.structure(even, [&odd_a, &odd_b, &odd_n]);
    let odd_fields = [
        declarations.reference(&odd_n, &even_ty, Mutability::Mutable),
        declarations.reference(&odd_b, &int32, Mutability::Immutable),
    ];
    declarations.define_struct(odd, odd_fields);

    let engine = declarations.finish().await;
    assert_eq!(variances(&*engine.get_variance(even).await), [Invariant; 3]);
    assert_eq!(variances(&*engine.get_variance(odd).await), [Invariant; 3]);
}

// input: struct Raw['a, t]: (*t, *mut &'a int32)
// premise: {}
// output: ['a Bivariant, t Invariant]; a raw pointer's pointee is not a use
#[tokio::test]
async fn raw_pointer_pointee_is_not_a_use() {
    let mut declarations = Declarations::new().await;
    let (raw, [a, t]) = declarations.declare(["'a", "t"]);
    let int32 = declarations.int32();
    let reference = declarations.reference(&a, &int32, Mutability::Immutable);
    let fields = [
        declarations.pointer(&t, Mutability::Immutable),
        declarations.pointer(&reference, Mutability::Mutable),
    ];
    declarations.define_struct(raw, fields);

    let engine = declarations.finish().await;
    assert_eq!(variances(&*engine.get_variance(raw).await), [Bivariant, Invariant]);
}

// input: struct Token[-'a, +t, =u]: int32
// premise: {}
// output: ['a Contravariant, t Covariant, u Invariant]
#[tokio::test]
async fn unused_parameter_has_its_declared_variance() {
    let mut declarations = Declarations::new().await;
    let (token, _) = declarations.declare(["-'a", "+t", "=u"]);
    let int32 = declarations.int32();
    declarations.define_struct(token, [int32]);

    let engine = declarations.finish().await;
    assert_eq!(variances(&*engine.get_variance(token).await), [
        Contravariant,
        Covariant,
        Invariant
    ]);
}

// input: struct Cell['a, =t]: &'a t, and struct User['b, u]: Cell['b, u]
// premise: {}
// output: Cell is ['a Covariant, t Invariant], and so User is
//         ['b Covariant, u Invariant]
#[tokio::test]
async fn declared_variance_replaces_inferred_variance_at_uses() {
    let mut declarations = Declarations::new().await;
    let (cell, [a, t]) = declarations.declare(["'a", "=t"]);
    let (user, [b, u]) = declarations.declare(["'b", "u"]);
    let cell_field = declarations.reference(&a, &t, Mutability::Immutable);
    declarations.define_struct(cell, [cell_field]);
    let user_field = declarations.structure(cell, [&b, &u]);
    declarations.define_struct(user, [user_field]);

    let engine = declarations.finish().await;
    assert_eq!(variances(&*engine.get_variance(cell).await), [Covariant, Invariant]);
    assert_eq!(variances(&*engine.get_variance(user).await), [Covariant, Invariant]);
}

// input: struct Phantom[t]: int32, and struct User[u]: Phantom[u]
// premise: {}
// output: Phantom is [t Invariant], and so User is [u Invariant]
#[tokio::test]
async fn unused_type_parameter_is_invariant_at_uses() {
    let mut declarations = Declarations::new().await;
    let (phantom, [_]) = declarations.declare(["t"]);
    let (user, [u]) = declarations.declare(["u"]);
    let int32 = declarations.int32();
    declarations.define_struct(phantom, [int32]);
    let user_field = declarations.structure(phantom, [&u]);
    declarations.define_struct(user, [user_field]);

    let engine = declarations.finish().await;
    assert_eq!(variances(&*engine.get_variance(phantom).await), [Invariant]);
    assert_eq!(variances(&*engine.get_variance(user).await), [Invariant]);
}

// input: eff E['p, 'r, 'b, 'u]:
//            def op(x: &'p int32, y: &'b int32) -> (&'r int32, &'b int32)
// premise: {}
// output: ['p Covariant, 'r Contravariant, 'b Invariant, 'u Bivariant]
#[tokio::test]
async fn operation_positions_give_effect_variances() {
    let mut declarations = Declarations::new().await;
    let (effect, [p, r, b, _]) = declarations.declare(["'p", "'r", "'b", "'u"]);
    let int32 = declarations.int32();
    let parameters = vec![
        declarations.reference(&p, &int32, Mutability::Immutable),
        declarations.reference(&b, &int32, Mutability::Immutable),
    ];
    let return_type = Ty::new_tuple(
        declarations.engine().intern_unsized([
            declarations.reference(&r, &int32, Mutability::Immutable),
            declarations.reference(&b, &int32, Mutability::Immutable),
        ]),
        declarations.engine(),
    );
    declarations.define_effect(effect, [(parameters, return_type)]);

    let engine = declarations.finish().await;
    assert_eq!(variances(&*engine.get_variance(effect).await), [
        Covariant,
        Contravariant,
        Invariant,
        Bivariant
    ]);
}

// input: eff E['s, t]: def get() -> MutRef['s, t], and
//        struct MutRef['a, u]: &'a mut u
// premise: {}
// output: ['s Contravariant, t Invariant]
#[tokio::test]
async fn effect_variance_composes_with_struct_variance() {
    let mut declarations = Declarations::new().await;
    let (mut_ref, [a, u]) = declarations.declare(["'a", "u"]);
    let (effect, [s, t]) = declarations.declare(["'s", "t"]);
    let field = declarations.reference(&a, &u, Mutability::Mutable);
    declarations.define_struct(mut_ref, [field]);
    let return_type = declarations.structure(mut_ref, [&s, &t]);
    declarations.define_effect(effect, [(Vec::new(), return_type)]);

    let engine = declarations.finish().await;
    assert_eq!(variances(&*engine.get_variance(effect).await), [Contravariant, Invariant]);
}

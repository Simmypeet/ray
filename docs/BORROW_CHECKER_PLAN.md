This document is the implementation plan for Ray's borrow checker. The baseline
is **exactly Rust's rules**: the same things Rust accepts should be accepted, and
the same things Rust rejects should be rejected. Where this plan deliberately
deviates, or where Ray has no Rust counterpart (effect handlers, dictionaries),
the text says so explicitly. The checking algorithm is the location-sensitive
**Polonius** model described in
[Polonius, part 1](https://smallcultfollowing.com/babysteps/blog/2023/09/22/polonius-part-1/)
and
[part 2](https://smallcultfollowing.com/babysteps/blog/2023/09/29/polonius-part-2/).

The plan is split into phases. Each phase compiles and passes the test suite on
its own, so the work can land incrementally. The first half of the document
describes the language changes; the second half describes the compiler
changes, phase by phase. Design decisions made while reviewing the plan are
collected at the end.

# Summary of the Design

1. Add a new kind, `TyKind::Lifetime`. Lifetime parameters are ordinary
   polymorphic variables of that kind, so `Args`, `Subst` and `PolyVarMap`
   carry them without new machinery.
2. Add a safe reference type `&'a t` / `&'a mut t`. `.&` and `.&mut`, by-reference
   closure captures and handler captures all produce references. The existing
   `*t` / `*mut t` stay as unchecked raw pointers for FFI and low-level code.
3. Add outlives predicates (`'a: 'b`, `t: 'a`), well-formedness rules, and
   implied bounds.
4. Compute the **variance** of every struct parameter with a whole-target
   fixed-point, and of every `eff` parameter from its operation signatures.
   A parameter may instead declare its variance, as in `+t`, `-t` or `=t`.
   Effect rows use subtyping on the lifetimes in their labels, while the set
   of labels is still unified exactly.
5. Make the type relation variance-aware. Type *structure* is still unified
   exactly. Relating two lifetimes does not bind anything: it emits an
   **outlives constraint** as a side output. Instance resolution and marker
   entailment emit outlives constraints the same way.
6. **Type inference in the typed AST ignores lifetimes** (every lifetime
   relation succeeds). Lifetimes are re-inferred later on the IR, where each
   constraint can be tagged with the CFG point where it holds. This follows
   rustc's split between `typeck` and MIR type checking. It refines the original
   idea of collecting constraints during typed-AST inference (see
   [Why Lifetimes Are Re-Inferred on the IR](#why-lifetimes-are-re-inferred-on-the-ir)).
7. After memory analysis (moves and drop elaboration), an **IR type check**
   gives each lifetime in the function its own region variable and produces
   location-tagged constraints.
8. **Polonius** builds the location-sensitive constraint graph, computes which
   loans are live at each point, and reports every access that invalidates a
   live loan. A separate check verifies the relations between universal regions
   (the lifetime parameters of the function, and `'static`).
9. Monomorphization erases every lifetime. Lifetimes never affect code
   generation or instance selection.

# Language Changes

## Lifetimes

A lifetime is written `'name`. `'static` is built in. Lifetimes appear in the
same bracketed argument list as types, and by convention come first:

```
struct Ref['a, t]:
    value: &'a t
```

In `def` signatures, a lifetime that is mentioned but not declared is
introduced implicitly, the same way lowercase type variables such as `a` are
today:

```
def first(pair: &'a Pair[t, t]) -> &'a t:
    return pair.*.first.&
```

Structs, traits, effects and instances must declare their lifetimes
explicitly, as in Rust. The only difference from Rust is the implicit
introduction in `def` signatures, and it only affects syntax.

## References

| Type          | Meaning                             | `Copy` | Variance in `'a` | Variance in `t` |
|---------------|-------------------------------------|--------|------------------|-----------------|
| `&'a t`       | shared reference, checked           | yes    | covariant        | covariant       |
| `&'a mut t`   | unique reference, checked           | no     | covariant        | invariant       |
| `*t`          | raw pointer, unchecked (existing)   | yes    | —                | bivariant       |
| `*mut t`      | raw pointer, unchecked (existing)   | yes    | —                | bivariant       |

- `place.&` has type `&'r t` and `place.&mut` has type `&'r mut t`, where `'r`
  is a fresh region. Taking either one creates a **loan** of `place`.
- `r.*` is a place. Writing through it, or taking `.&mut` of it, requires `r`
  to be `&mut` (or `*mut`). This extends the existing lvalue requirement
  check.
- A non-`Copy` value cannot be moved out of `r.*` when `r` is a reference.
  Memory analysis reports this as "cannot move out of a borrow" (Rust E0507).
- **Coercions** at typed-AST coercion sites, as in Rust:
  - `&'a mut t` to `&'a t`;
  - `&'a t` to `*t` and `&'a mut t` to `*mut t`. This keeps FFI such as
    `scanf("%d", x.&mut)` working;
  - an implicit **reborrow** when a `&mut` value is passed where a `&mut` is
    expected: `f(r)` becomes `f(r.*.&mut)`, so `r` is not moved. Without this,
    `&mut` is not practical to use.

Raw pointers stay unchecked. Creating a raw pointer is always allowed, but
**dereferencing one requires `unsafe`**, as in Rust. Ray does not have
`unsafe` yet, so it is part of this work.

A dereference of a raw pointer is a separate projection, `RawDeref`, distinct
from the `Deref` of a reference (see
[Deref as a Projection](#deref-as-a-projection)). A place behind a `RawDeref`
is plain memory that the compiler does not track, and its rules deliberately
differ from Rust's:

- **Assigning** through a `RawDeref` is a plain write. The previous value is
  not dropped. In Rust, `*raw = value` drops the old value, and only
  `ptr::write` does not.
- **Loading** through a `RawDeref` is a plain bitwise copy, even for a
  non-`Copy` type. It is not checked as a move, and the source is not marked
  uninitialized, like Rust's `ptr::read`. Keeping ownership consistent, for
  example not dropping the same value twice, is the programmer's
  responsibility inside `unsafe`.

## Lifetime Elision in `def` Signatures

Rust's rules apply:

1. Each elided lifetime in a parameter becomes a fresh lifetime parameter of
   the function.
2. If the parameters mention exactly one lifetime, elided or not, it is used
   for every elided lifetime in the return type.
3. Otherwise, an elided lifetime in the return type is an error.

Rust's `&self` rule has no direct counterpart, because Ray has no receiver
syntax, and it is not added for now.

Elided lifetimes are not allowed in struct fields.

## Outlives Predicates

`where` clauses accept:

- `'a: 'b`: `'a` outlives `'b`;
- `t: 'a`: every lifetime in `t` outlives `'a`.

**Implied bounds** come only from references: `&'a t` implies `t: 'a`. The
references in a function's parameter and return types are assumed
well-formed, so `def f(x: &'a &'b int32)` implies `'b: 'a` without writing it.

**Inferred outlives requirements** for structs follow Rust RFC 2093, with the
same restriction: `struct Ref['a, t]: value: &'a t` infers `t: 'a` without a
`where` clause, and a struct holding a `Ref['a, t]` infers it in turn.
Computing this is a fixed point over struct definitions, like variance.

This deviates from Rust: an outlives predicate declared in a `where` clause is
never implied or inferred. `struct B['a, t] where (t: 'a)` requires `t: 'a` of
every use, and a struct holding a `B['a, t]` or a function taking one must
write `t: 'a` itself.

**Marker implementations** are the one exception. An implementation assumes
every requirement of naming its head, such as `t: 'a` for
`impl['a, t] Send for &'a t` or the declared where clause of `S` for
`impl[t] Send for S[t]`, including marker predicates. An implementation only
applies to a goal type matching its head, and that type is well-formed where it
is named, so the requirements hold at every use. A valid head is one
constructor applied to distinct variables, so the assumptions never claim
anything about a concrete type. Marker implementation where clauses therefore
reject outlives predicates and accept only marker predicates. This also lets a
negative implementation, which cannot have a where clause, name a head with
requirements.

## Well-Formedness

- `&'a t` is well-formed only if `t: 'a`.
- `S['a, t]` is well-formed only if the struct's inferred and declared outlives
  predicates hold for those arguments.

Signatures and struct fields are checked for well-formedness at declaration
time. Types inside bodies are checked during the IR type check, where the
resulting outlives obligations become constraints.

## Variance

Variance is the usual four-point lattice: bivariant (unused), covariant,
contravariant, invariant.

| Constructor                     | Variance of its arguments                                       |
|---------------------------------|-----------------------------------------------------------------|
| primitives                      | none                                                            |
| tuple                           | covariant                                                       |
| `&'a t`                         | `'a` covariant, `t` covariant                                   |
| `&'a mut t`                     | `'a` covariant, `t` invariant                                   |
| `*t` / `*mut t`                 | bivariant (raw pointers are unchecked)                          |
| struct                          | declared or computed (see below)                                |
| closure type                    | invariant in all arguments (conservative; see below)            |
| instance / dictionary types     | invariant                                                       |
| associated type projection      | invariant (as in Rust)                                          |
| effect label                    | declared or computed per `eff` (see [Effect Variance](#effect-variance)) |
| effect row                      | by label; see [Effects and Lifetimes](#effects-and-lifetimes)   |

The variance of a struct parameter is the join, over every occurrence of the
parameter in the struct's fields, of the variance at that position. Positions
compose with the usual "transform" operation (for example, covariant inside
invariant is invariant). Recursive and mutually recursive structs need a
fixed point: start every parameter at bivariant and iterate until nothing
changes. As in rustc, this is one query over the whole target, not one query
per struct, because the dependency graph is cyclic.

A parameter that stays bivariant is unused. Rust rejects unused lifetime
parameters (and unused type parameters, which need `PhantomData`). This plan
allows both. An unused **lifetime** parameter stays bivariant, so its
arguments are never related. Every other unused parameter (of kind type,
effect row or dictionary) is made invariant. That can make a use of it in
another struct invariant, so the fixed point runs again after defaulting.

Raw pointers are unchecked, so their pointee is bivariant: a parameter used
only behind `*t` or `*mut t` is unused.

### Declared Variance

A struct or `eff` parameter may declare its variance with a marker: `+` for
covariant, `-` for contravariant, and `=` for invariant. Bivariance cannot be
declared.

```
struct Iter[+'a, +t]:        # 'a is unused, t is only behind a raw pointer
    ptr: *mut t

struct Cell['a, =t]:         # more restrictive than the inferred covariance
    value: &'a t
```

- The declared variance **replaces** the inferred one. Every use of the
  struct or `eff` elsewhere, and every relation, sees the declared variance.
  It is a constant in the fixed point, so it also cuts recursion, and editing
  the fields of a struct cannot silently change its declared variance.
- Every use of a declared parameter must be **within** the declared variance:
  the join of its uses in each field, operation parameter or return type must
  be at most the declared variance in the lattice. Anything else is an error
  at that use site. Declaring a variance more restrictive than needed is the
  point of the feature, so it is never a lint.
- An unused parameter may declare any variance. This replaces Rust's
  `PhantomData` for giving an unused lifetime a variance.
- A declared variance only affects variance. It does not add outlives
  requirements: a struct that needs `t: 'a` writes it in its where clause.
- Markers are only allowed on the parameters of a `struct` or an `eff`.

Closure types are invariant in all their arguments. This matches rustc, which
relates the generic arguments of two closure types invariantly. A closure
type is nominal and identical at every use, so invariance only affects code
that relates two different instantiations of the same closure. This is about
the closure **type**, not the `Def` trait: trait arguments are always related
invariantly, as in Rust, where traits have no variance.

Ray has no function-pointer types today, so no built-in type constructor is
contravariant. Contravariance does arise from effect parameters used in
operation return types (see [Effect Variance](#effect-variance)).

## Instance Selection Ignores Lifetimes

As in Rust, lifetimes never decide **which** instance is chosen:

- Candidate matching, ranking, and the overlap check treat every lifetime as
  equal.
- After an instance is chosen, matching its head against the goal, and its
  `where` clause, produce outlives constraints. For example,
  `inst for Show[&'static cstr]` selected for `Show[&'r cstr]` yields
  `'r: 'static`, and that constraint may fail later in the borrow checker.

This is what makes it sound to run type inference and monomorphization
without lifetimes.

Marker (`Copy`) entailment follows the same rule. Rust allows a marker
implementation to depend on lifetimes, for example `impl Copy for S<'static>`.
That is a known source of trouble in rustc, so Ray deliberately rejects such
marker implementations at declaration time, at least for now. This is a
deviation from Rust.

## Effects and Lifetimes

An effect may take lifetime parameters: `eff Reader['a]: def get() -> &'a int32`.
A `perform` instantiates the operation signature from the label's arguments,
exactly as for type arguments today.

### Effect Row Subtyping

Effect rows use **subtyping on the lifetimes inside their labels**, not
invariance. The structure of a row is still unified exactly:

- **Which labels a row contains** is decided by the existing Koka-style row
  unification. Labels match by effect symbol only, the label sets must agree,
  and tails are rewritten to a shared tail. There is no subset subtyping on
  label sets. Open tails already express "a computation with fewer effects":
  `\ {A | e}` unifies with `{A, B}`. Adding subset subtyping would make a
  constraint such as `?E <: {A, B}` fail to determine `?E`, and inference
  would lose principal types.
- **The lifetimes in matched labels** are related by the variance of each
  effect parameter. `row₁ <: row₂` relates each pair of matched labels
  `L[args₁] <: L[args₂]` by `L`'s parameter variances, and relates the tails
  covariantly.

The typed-AST inference is unaffected. It ignores lifetimes, so for the calls
`a(); b()` in a function with effect `B`, binding `?Ea := B` and `?Eb := B`
by unification is still correct. The subtyping only matters in the IR type
check, which relates each concrete, renumbered row to the enclosing one at
the point where the effect flows. See the effect items in
[Phase 5](#phase-5-ir-type-check-region-renumbering).

Subtyping applies only where a sub-computation's effects flow into the
enclosing computation:

- a call: callee row `<:` the current function's row;
- a `perform`: the operation's label `<:` the current function's row;
- a `run body with Eff[...]`: the body's row `<:` the handled label plus the
  row of the code around the `run`.

A row stored in an invariant position (a closure type's arguments, `Def`'s
`Effect` associated type, or a dictionary type) is still related by equality,
just as `T` in `&mut T` stays invariant even though `T` can have subtyping.

### Effect Variance

A `perform` is a call **to** the handler, so an effect label behaves like a
function pointer held by the computation. A computation with effect `E` is
roughly a function taking a handler for `E`. That puts the handler interface
in a contravariant position, which flips the usual function variance:

- a parameter used in an operation's **parameter** types is **covariant**;
- a parameter used in an operation's **return** type is **contravariant**;
- a parameter used in both is **invariant**.

Two examples:

```
eff Log['a]:
    def log(message: &'a cstr)       # 'a is covariant

eff Reader['a]:
    def get() -> &'a int32           # 'a is contravariant
```

- `Log['long] <: Log['short]` when `'long: 'short`. A callee that logs
  `&'long` messages can run under a handler that accepts `&'short` messages,
  because the reference coerces.
- `Reader['x] <: Reader['static]` when `'static: 'x`, which always holds. A
  callee that only needs `&'x` references can run under a handler that
  returns `&'static` references. The reverse is rejected: a callee that asks
  for `Reader['static]` may store the reference somewhere long-lived, so it
  cannot run under a handler that returns references to a local.

Positions nested inside a parameter or return type compose with the usual
transform operation, using struct variances where structs appear. Positions
inside invariant constructors (closure types, `&mut`) stay invariant. An
effect parameter that stays bivariant is unused, and is handled as for
structs; see [Declared Variance](#declared-variance) for declaring it.

Operation signatures can mention structs, and structs can mention effect rows
inside closure types, so struct and effect variances depend on each other.
They are computed together as one fixed point in the same query as structs,
and the arguments of an effect label are related by that effect's variances.

This relies on handlers being tail-resumptive: the handler finishes the
operation before the computation continues, so the handler really is just an
argument the computation calls. See the note about continuations below.

### Outlives for Rows

The outlives relation needs a rule for rows: `row: 'a` holds when every label
argument outlives `'a` and the tail outlives `'a`. For a tail that is a row
polymorphic variable `e`, `e: 'a` must come from a `where` clause or from
implied bounds, like a type variable.

### Handler Captures

Besides row subtyping, handlers interact with borrowing through their
captures. Ray's handlers are tail-resumptive: an operation handler returns a
value and the body resumes. The borrow checker can therefore treat them as
follows:

- `run body with Eff: handlers` creates the handler closures, with their
  by-reference captures borrowed, when the `run` starts, and keeps them alive
  until it ends.
- A `perform` inside the body is a call into a handler that is already alive.
  Its loans are those of the handler's captures, and they are live for the
  whole `run`.
- The body thunk's captures are borrowed by the same rules.

So a handler that captures `a` by `&mut` conflicts with a body that reads `a`
directly, exactly as two Rust closures would conflict. That is the correct
Rust-equivalent rule. The existing fixture `effect_handler_shared_capture`
stays valid only because its body does not touch `a` directly.

If Ray ever adds non-tail-resumptive handlers (a first-class `resume`, or
multi-shot continuations), a captured continuation holds the body's stack
loans. That would need a new rule, most likely "a continuation is a value
whose type mentions every region live at the `perform`", and is out of scope
here. Such handlers would also invalidate the reasoning behind
[Effect Variance](#effect-variance), since the handler would no longer be
just an argument the computation calls, so both rules must be revisited
together.

## Drops Are Uses

Memory analysis already inserts explicit `Drop.drop` calls into the IR before
borrow checking runs. The borrow checker therefore sees drops as ordinary
uses, and Rust's "drop check" mostly falls out. The part that needs care is
precision. Rust does not treat dropping a `&t`, or a struct with no `Drop`
implementation that transitively holds only references, as a use of those
lifetimes. Ray matches this by computing which regions a drop **needs**:

- A no-op drop (primitives, pointers, references, `NoDrop`) needs no region.
- A tuple, closure, or generated nominal drop needs the union of what its
  elements, captures, or fields need.
- A user-written `Drop` instance, or a `given Drop[a]` dictionary for an
  opaque type, needs every region in the dropped type.

This is rustc's `dropck_outlives` computation, without `#[may_dangle]`.

# Why Lifetimes Are Re-Inferred on the IR

The original sketch accumulated lifetime constraints during typed-AST type
inference. This plan makes inference ignore lifetimes and re-infers them on
the IR, for four reasons:

1. **Polonius needs locations.** Every subset constraint must be tagged with
   the CFG point where it holds. The typed AST has no CFG. Mapping spans to
   points afterwards is fragile, because one expression becomes several
   instructions and some instructions have no source expression at all.
2. **Constraints the typed AST never sees.** Drops, stores into temporaries,
   reborrows, and the forced moves inserted by memory analysis exist only in
   the IR.
3. **Known types.** When the IR is built, every type is fully known up to
   lifetimes. The relation still implements rustc's generalization (see
   [Phase 4](#phase-4-variance-aware-relation-and-outlives-side-output)), but
   the constraints the borrow checker keeps never depend on it: on the IR
   there are no type inference variables to generalize.
4. **Caching.** Typed-AST query results stay lifetime-erased and stable, and
   only the borrow-check query depends on region details.

The solver still emits outlives constraints as a side output, as originally
proposed. During typed-AST inference the caller discards them. During the IR
type check the caller keeps them and tags them with a point.

# Compiler Changes

## Phase 0: IR Prerequisites

These refactors change no behavior and can land first.

- **Deref as a projection only.** Remove `AddressRoot::Deref(IRExprID)` and
  add dereference projections, so that a place is always a root plus a list
  of projections, as in Rust's MIR. See
  [Deref as a Projection](#deref-as-a-projection).
- **Storage end.** `Instruction::ScopePop` already marks where locals die.
  Document that it is the "storage dead" point, because a live loan of a local
  at its `ScopePop` is the "does not live long enough" error (Rust E0597), and
  temporaries (E0716) are covered the same way by temporary scopes.
- **Variable liveness.** Add a backward dataflow problem in `rayc_ir` over the
  existing `dataflow` framework. It records which locals are **use-live** (read
  later) and which are **drop-live** (only dropped later). The drop-live
  distinction feeds the drop precision rules above.

### Deref as a Projection

Today `AddressRoot::Deref(IRExprID)` roots a place at an rvalue, the loaded
pointer, so `r.*.field` becomes `Deref(Load(r))` followed by
`[Field(field)]`. This causes two problems:

- **The base place is lost.** The borrow checker needs to know that
  `r.*.field` is based on `r`. Writing to `r` kills loans through `r.*`, the
  [reborrow walk](#reborrow-constraints) follows the dereferences in the path,
  and place overlap compares paths. None of this works when the base is
  hidden behind an expression.
- **A hidden move.** `Load::Implicit` of a non-`Copy` value is a move. Raw
  pointers are `Copy`, so this is harmless today, but once `&mut` exists,
  `r.*.x = 1` would move `r` out. By-reference captures are lowered the same
  way (`function_build_state.rs` loads the capture, then dereferences it), so
  they have the same problem.

After the change, `r.*.field` is `Variable(r)` followed by
`[Deref, Field(field)]`, and a by-reference capture `x` is `Capture(x)`
followed by `[Deref]`. Nothing is loaded to reach the place.

There are two dereference projections:

- `Deref` dereferences a **reference**. It is checked: moves out through it
  are rejected, and the borrow checker follows it in the
  [reborrow walk](#reborrow-constraints).
- `RawDeref` dereferences a **raw pointer**. It requires `unsafe`, and the
  place behind it is untracked memory: assignment is a plain write that does
  not drop the old value, and a load is a plain bitwise copy (see
  [References](#references)).

Today every dereference is of a raw pointer, and memory analysis already
leaves deref roots untracked. So Phase 0 only introduces `RawDeref`, and
behavior does not change. `Deref` arrives in Phase 1 together with
references. The table below writes `Deref` for whichever projection
applies.

Changes:

| Where                                                   | Today                                   | After                                                                     |
|---------------------------------------------------------|-----------------------------------------|---------------------------------------------------------------------------|
| `rayc_ir` `address.rs`                                  | `AddressRoot::Deref(IRExprID)`          | removed; `Projection::RawDeref` added (and `Projection::Deref` in Phase 1) |
| `ir_builder` `expression/deref.rs`                      | `Deref(rvalue)`                         | the base's place plus `Deref`; a computed base goes through `lower_to_address_or_temporary` first |
| `ir_builder` `function_build_state.rs` (captures)        | load the capture, then `Deref`          | `Capture(id)` plus `Deref`                                                |
| `memory` `stack_state.rs`                               | a deref root is untracked               | the place is tracked only up to its first `Deref`                        |
| `mono_ir_builder` `lower/cfg.rs`                        | `expression_place(e).dereference()`     | lower the root place, then apply each projection, including `Deref`      |

Rules that need care:

- **Memory analysis.** What lies beyond a `Deref` is not owned by the stack
  frame. Initialization tracking stops at the prefix before the first `Deref`,
  and drop elaboration never drops through one. Two rules the implicit load
  of the base used to enforce must now be stated directly:
  - using `r.*...` requires `r` to be initialized, as a read of `r` that does
    not move it;
  - writing `r.*.x = 1` does not initialize `r`.
- **Moves through a dereference.** Moving out through a `Deref` (a reference)
  is "cannot move out of a borrow" (Phase 1). A load through a `RawDeref` is a
  bitwise copy and never counts as a move, and an assignment through a
  `RawDeref` never drops the old value.
- **Types of prefixes.** `Address` carries no types. The borrow checker and
  memory analysis need to know whether each dereferenced pointer is `&`,
  `&mut` or raw. Add a helper that computes the type of each prefix from the
  root's type and the field substitutions, so `Projection::Deref` stores
  nothing.
- **Temporaries.** Dereferencing a computed value, such as `f().*` or
  `(move pointer).*`, now creates a temporary. Pointers and references have a
  no-op drop, so the only cost is one more IR variable. The narrower
  `ScopePop` rule in [Phase 6](#phase-6-polonius-location-sensitive-loan-liveness)
  makes sure the temporary's end does not invalidate loans of the data behind
  it.

This refactor should land **before references exist**, while every pointer is
still `Copy`. Then the existing e2e suite checks it as a pure refactor: no
fixture output should change. Landing it later would mix it with removing the
hidden move.

## Phase 1: Lifetime Kind and Reference Types (no checking yet)

Goal: the language has references and lifetimes, and programs type-check, but
nothing is borrow-checked yet.

`rayc_syntax` / `rayc_parser` / `rayc_lexical`:
- A lifetime token `'ident`, lifetime arguments in bracket lists, `&'a t` and
  `&'a mut t` type syntax, and outlives predicates in `where` clauses.
- `unsafe`, which is required to dereference a raw pointer. Existing fixtures
  that dereference raw pointers directly (rather than through `.&`, which now
  produces references) need to be wrapped in `unsafe`.

`rayc_ir`:
- `Projection::Deref` for references, next to the `RawDeref` from Phase 0.

`rayc_type`:
- `TyKind::Lifetime`.
- `PolyVar::new_lifetime`: lifetime parameters are `Ty::PolyVar` of kind
  `Lifetime`, so substitution works unchanged.
- A new `Ty::Lifetime(Lifetime)` variant for lifetimes that are not
  parameters:

  ```rust
  pub enum Lifetime {
      Static,
      /// Lifetimes inside function bodies before borrow checking.
      Erased,
      /// A region variable, only created by the IR type check.
      Region(RegionID),
      Error,
  }
  ```

  Region variables are deliberately **not** `Ty::Inference`. Type inference
  binds inference variables through substitution, while region variables are
  never bound. They only collect outlives constraints. Keeping them separate
  makes it impossible for unification to bind one by accident.
- `Constant::Reference(Mutability)`, with arguments `[lifetime, pointee]`.
- `Ty::recursive_iter`, `contains_error`, `Substitutable` and `Reduce` all
  handle the new variants. A lifetime never reduces.

`rayc_semantic_element_impl`:
- Resolve lifetime names, implicit lifetime introduction in `def` signatures,
  and elision.

`rayc_typed_ast_builder`:
- `build_ref_of` produces `Constant::Reference` with an `Erased` lifetime
  instead of `Constant::Pointer`.
- Mutability of `r.*` places depends on the reference's mutability.
- The coercions listed under [References](#references).
- By-reference captures in the `CapturePlan` store references instead of raw
  pointers.

`rayc_solver`:
- `ty_relate`: two lifetime-kind types always relate successfully and produce
  no substitution. Kind checks in `bind_infer_var` / `bind_poly_var` already
  stop a lifetime being bound to a type.
- `Copy` for `&'a t` is built in. `&'a mut t` is never `Copy`.
- A no-op `Drop` for references, next to the one for pointers.

`rayc_memory`:
- "Cannot move out of a borrow" for non-`Copy` loads through a reference
  deref.

`rayc_mono_ir_builder` / codegen:
- References lower to the same representation as raw pointers.
  Monomorphization erases every lifetime before it interns an instantiated
  type, so `f['a]` and `f['b]` share one instance.

Tests: e2e `run` fixtures for references (read, write through `&mut`,
reborrow, FFI coercion) and `check` fixtures for mutability errors and moves
out of borrows. The existing fixtures that use `.&` keep their runtime
behavior.

## Phase 2: Outlives Predicates and Well-Formedness

- Add `PredicateKind::Outlives(OutlivesPredicate)`, a `subject: 'bound`
  requirement, to `where_clause`. The subject may be a lifetime (`'a: 'b`), a
  type or effect row (`t: 'a`), or, conservatively, a dictionary.
- Add inferred outlives requirements for structs (`get_inferred_outlives`).
  This is a target-wide fixed point, and can share its driver with the
  variance computation in Phase 3. Using a struct requires its inferred
  outlives predicates as well as its declared ones.
- Add implied bounds, for plain `def`s, structs, and marker implementations
  only: the bounds implied by the references in a `def`'s parameter and return
  types, a struct's inferred outlives, and every requirement of a marker
  implementation's head. Declared outlives predicates are never implied,
  except through a marker implementation's head. Every other declaration
  (trait and instance `def`s, instances, effects) must write its bounds in its
  where clause, which accepts outlives predicates everywhere except in marker
  implementations.
  This deviates from Rust, which also implies bounds from impl headers and
  trait method signatures. Implied bounds are
  part of the symbol's `WhereClause` (`get_where_clause`), next to its
  declared predicates (`get_declared_where_clause`), so every consumer of the
  where clause sees them. The outlives predicates among the givens form the
  `OutlivesEnvironment`, whose facts are the "known relations" between
  universal regions consumed in Phase 5.
- Add declaration-level WF checks. A written `&'a t` requires `t: 'a`, and a
  resolved symbol requires its instantiated outlives predicates. These checks
  only concern named lifetimes and need no region inference. They compare
  required outlives facts against declared and implied ones. Type inference
  drops outlives obligations; Phase 5 re-checks them on the IR.

Diagnostics: missing `t: 'a` bounds. Undeclared lifetimes in structs are
already reported by lifetime resolution. Unused lifetime parameters are
allowed and bivariant; see [Variance](#variance).

## Phase 3: Variance

- Add a `Variance` enum with `xform` and `join` in `rayc_type`.
- Add a query, `get_variances(target) -> map from struct to [Variance]`, that
  iterates to a fixed point over every struct in the target, following the
  table in [Variance](#variance). Add `get_variance(struct_id)` as a thin
  projection of it.
- Compute the variance of every `eff` parameter in the same fixed point as
  the structs, following [Effect Variance](#effect-variance): operation
  parameter types are walked starting from covariant, return types starting
  from contravariant, and effect labels in rows are walked by their effect's
  variances. The same `get_variance` query returns it for an `eff`.
- Variances are returned as a `VarianceMap`, which iterates them in poly var
  map order (the argument order) and looks one up by poly var ID.
- Unused lifetime parameters are allowed and stay bivariant. Other unused
  parameters are defaulted to invariant.
- Parse variance markers (`+`, `-`, `=`) on type parameters and store the
  declared variance on the poly var. A declared parameter starts the fixed
  point at its declared variance, which never changes. A separate query,
  `VarianceMismatchKey`, walks each field and operation signature again with
  the final variances and reports every use outside the declared variance.
  Markers on other declarations are reported when the poly var map is built.

Unit tests are justified for this phase: variance is a pure function of
declarations. The key cases are recursive structs, mutual recursion, `&mut`
inside another struct, raw pointers, declared variances, and effect
parameters used in operation parameters, in operation returns, in both, and
through a struct.

## Phase 4: Variance-Aware Relation and Outlives Side Output

`TyRelate` is currently equality under the names `lesser` and `greater`. Make
the variance explicit:

```rust
pub struct TyRelate {
    lesser: Interned<Ty>,
    greater: Interned<Ty>,
    variance: Variance, // Covariant: lesser <: greater; Invariant: equality
}
```

- When the relation decomposes an application, each derived constraint gets
  `parent.variance.xform(variance_of(constructor, index))`. For a struct, the
  variance comes from `get_variance`. Everything else uses the built-in table.
- The effect row relation (`entail_effect_row_subtype`) keeps its label
  matching and tail rewriting unchanged. What changes is the constraints it
  derives for label arguments
  (`DerivationRule::EffectLabelArgumentMatching`): they carry
  `parent.variance.xform(get_variance(effect)[index])` instead of
  plain equality. Tail constraints carry the parent variance. When a row sits
  in an invariant position, the parent variance is invariant, so everything
  under it is related by equality.
- Type structure that is not a lifetime is still unified **exactly**, whatever
  the variance, because Ray has no subtyping apart from lifetimes (and no
  higher-ranked types yet). Variance only changes what happens at lifetime
  leaves.
- At a lifetime leaf, `Step` gains a new outcome:

  ```rust
  pub enum Step {
      Subst(Subst),
      Derived(Vec<DerivedConstraint>),
      /// Lifetime relations produced by this step, as `longer: shorter`.
      Outlives(Vec<OutlivesConstraint>),
      NoProgress,
  }
  ```

  Covariant `'a <: 'b` becomes `'a: 'b`. Contravariant becomes `'b: 'a`.
  Invariant produces both. Bivariant produces nothing: relating two lifetimes
  in a bivariant position always succeeds. Relating two `Erased` lifetimes,
  or `Erased` with anything, produces nothing.
- `exhaustive_solve`, `head_match` and instance resolution return the outlives
  constraints they collect next to the substitution. Instance resolution
  appends the chosen instance's `Outlives` predicates, instantiated.
- `Solver` gains a mode switch, or a caller-supplied sink, that decides whether
  outlives constraints are kept (IR type check) or dropped (typed-AST
  inference). The typed-AST constraint solver uses the dropping mode, so its
  behavior does not change.
- `TopLevelMatching` may bind lifetime parameters of an instance head through
  substitution. That is ordinary instantiation, not inference.
- **There are no lifetime inference variables.** A lifetime is one of three
  kinds, and none of them is a `Ty::Inference`:

  | Kind                                        | Where it appears    | Bound by substitution?                                                   |
  |---------------------------------------------|---------------------|--------------------------------------------------------------------------|
  | lifetime parameter (`Ty::PolyVar` of kind `Lifetime`) | declarations | yes, by **instantiation** (calling a signature, matching an instance head) |
  | `Lifetime::Erased`                          | typed AST           | nothing to bind; every relation involving it succeeds                    |
  | `Lifetime::Region`                          | IR type check only  | **never**; it only gathers constraints                                   |

  A `Ty::Inference` of kind `Lifetime` must never be created. The unifier
  binds `Ty::Inference` variables, and no lifetime may go through that path.
- **Regions are never bound**, not even under invariance. An invariant
  relation between two regions emits two outlives constraints instead of a
  substitution, for two reasons:
  - **Precision.** An invariant relation at point P means "these two regions
    exchange loans at P". Binding `'r1 := 'r2` would make them the same region
    at every point, so loans that were only in `'r2` before P would also be in
    `'r1` before P. That is harmless for location-insensitive inference but
    loses exactly the precision Polonius adds.
  - **Simplicity.** The IR type check has no substitution to apply. Its output
    is a constraint set that Polonius consumes, and binding would add a
    union-find-and-rewrite step over every IR type just to express what two
    constraints already express.
- **Generalization.** When a type inference variable is bound in a
  non-invariant relation, the solver follows rustc and generalizes the other
  side first instead of binding to it directly. The generalizer walks the
  type with an ambient variance, composing it with each constructor's
  variance:
  - every type inference variable in a non-invariant position becomes a
    fresh type inference variable of the same kind **and the same
    `InferenceConstraint`**, since a numeric literal variable must stay
    numeric. Invariant positions keep the original variable;
  - every lifetime becomes `Erased`. Type inference variables exist only in
    the typed AST, where no region can be created, so there is never a fresh
    lifetime to generalize to. On the IR there are no type inference
    variables, so generalization never triggers there. The generalizer
    therefore has no region case, and nothing should create lifetime
    variables to fill one in;
  - the occurs check runs during generalization, as in rustc.

  In practice, generalization only introduces extra type variables. It is
  done anyway so that the relation is correct for every caller. The
  `RecordingInferenceGenerator` must record the variables it creates like any
  others, so that numeric defaulting and effect-row finalization still see
  them.
- **Given equalities match modulo lifetimes.** Associated-type reduction
  (`reduce_instance_associated`) is pure substitution, so any regions in its
  input are carried into its output and it needs no constraints. The only
  comparison in reduction is the given-equality rewrite in
  `Reduce for Interned<Ty>`, which today checks `equality.left() == self`.
  That exact check breaks once lifetimes exist. In the typed AST, a given
  `where d.Out['a] = &'a int32` would not match a body projection
  `d.Out['erased]`, which makes it a type-inference bug, not only a
  borrow-checker one. On the IR, a renumbered `d.Out['r7]` would never match
  anything. The rule becomes:
  - a given matches when the structure is equal, **ignoring lifetimes**.
    Lifetimes never decide whether a given applies, just as they never decide
    which instance is selected;
  - when it matches, each pair of corresponding lifetimes produces an
    **invariant** constraint (both directions), because projection arguments
    are invariant.
- **Reduction reports constraints.** `Reduce` takes a constraint sink, or
  returns its constraints next to the reduced value. This uses the same
  keep-or-drop split as the rest of the solver: typed-AST normalization drops
  them, and the IR type check records them at the current point.
- **Rigid projections relate structurally.** `entail_ty_relate` returns
  `NoProgress` for associated types. Today, two identical projections succeed
  only through the `lesser == greater` shortcut. On the IR, `d.Return` with
  renumbered regions on each side is no longer identical, so the relation
  would leave a residual and fail. Add a rule: two **irreducible** projections
  of the same member relate by relating their instances and arguments
  **invariantly**. A projection against any other type keeps today's
  behavior.

## Phase 5: IR Type Check (Region Renumbering)

A new crate, `rayc_borrowck`, runs per definition after `rayc_memory::analyze`.

1. **Universal regions.** Each lifetime parameter of the definition and
   `'static` gets a region variable marked *universal*. The known relations
   from Phase 2 (declared, implied, and "`'static` outlives everything") are
   recorded.
2. **Renumbering.** Every `Erased` lifetime in the type of every IR variable,
   expression, and capture gets a fresh *existential* region variable. The
   renumbered type stored on an IR expression or variable is only a **slot**:
   its regions are free, and they are constrained only by the flows into and
   out of it. It carries no information about where the value came from.
3. **Constraint generation.** Walk every instruction and emit
   `OutlivesConstraint { longer, shorter, point }` using the Phase 4 solver in
   keeping mode.

   Every endpoint that comes from a declaration is **re-instantiated** from
   the declaration with fresh regions, then **normalized eagerly**. Such
   endpoints are callee signatures, field types, operation signatures,
   instance associated types, and capture types. There are no type inference
   variables on the IR, so every reducible projection can be reduced
   immediately, and the constraints normalization reports are recorded at the
   current point. Whatever projections remain are rigid and relate
   structurally (Phase 4). Only then are the normalized endpoints related to
   the slots. The typed AST stays lazy, because inference variables there can
   block reduction.

   Normalization must happen **after** re-instantiation, never by reusing the
   typed AST's already normalized types. For example:

   ```
   def f(value: t) given (d: Get[t]) -> &'a int32 where d.Out = &'a int32:
       return d.get(value)
   ```

   The typed AST stores the call's result type already normalized, as
   `&'erased int32`. If the IR check only renumbered that to `&'r9 int32`,
   nothing would tie `'r9` to `'a`. The result would be unconstrained and
   could be accepted as `'static`, which is unsound. Re-instantiating
   `d.get`'s signature gives the result type `d.Out`. Normalizing it through
   the given gives `&'a int32`, and relating that to the slot ties `'r9` to
   `'a`.

   The relations for each construct:
   - `Store(address, expr)`: `type_of(expr) <: type_of(address)`;
   - `Call`: instantiate the callee signature with fresh regions for its
     lifetime parameters, relate each argument to its parameter type and the
     return type to the destination type, and prove the callee's `where`
     clause (outlives obligations become constraints). Also relate the
     callee's instantiated effect row `<:` the current function's effect row;
   - dictionary use (`d.call(...)`, drop dictionaries): re-run head matching
     for the already chosen instance to recover its outlives obligations;
   - `RefOf(place, mutability)` at point `p`: create loan `L` of `place`. The
     result type `&'r t` has `'r` containing `L` at `p`. If `place` goes
     through dereferences, add the **reborrow constraints** described in
     [Reborrow Constraints](#reborrow-constraints);
   - `Closure` / `Handle` construction: relate each capture operand to the
     capture type, and apply the nested function's propagated requirements
     (see step 5);
   - `Perform`: instantiate the operation signature from a label with fresh
     regions, relate arguments and result to it, and relate that label `<:`
     the current function's effect row;
   - `Handle`: relate the body thunk's effect row `<:` the handled label plus
     the current function's effect row, and relate each operation handler to
     the operation signature instantiated from the handled label;
   - well-formedness of every type that appears: `&'r t` requires `t: 'r`,
     and so on.
4. **Type-outlives.** Each `t: 'r` obligation is decomposed into region
   constraints following Rust's outlives components. Obligations about type
   parameters and projections are discharged against the where clause and
   implied bounds, or reported.
5. **Nested functions.** Lambdas, thunks and operation handlers are checked
   first, innermost first, and each passes requirements up to its parent.
   See [Nested Bodies](#nested-bodies).

### Reborrow Constraints

Borrowing a place that goes through a dereference is a **reborrow**. For a
borrow `P.&` or `P.&mut` with result region `'r`, walk the dereferences in
`P` from the innermost (closest to the borrow) outward:

- a dereference of `&'x mut _`: add `'x: 'r` and **continue** outward. A unique
  borrow is only unique if every `&mut` it was reached through stays
  borrowed for as long;
- a dereference of a shared `&'x _`: add `'x: 'r` and **stop**. The shared
  reference could have been copied out, so what it was reached through does
  not matter;
- a `RawDeref` (a raw pointer): **stop** with no constraint. Raw pointers are
  unchecked.

This is the rule from rustc's NLL, and it is what separates these two
getters:

```
struct Ref['a]:
    value: &'a int32

struct RefMut['a]:
    value: &'a mut int32

def getRef(self: &'s Ref['a]) -> &'a int32:
    return self.*.value            # accepted: copies the `&'a int32` out

def getRefMut(self: &'s mut RefMut['a]) -> &'a mut int32:
    return self.*.value            # error: lifetime may not live long enough
```

`getRef` copies a `Copy` reference out of the struct, so nothing is
reborrowed. In `getRefMut`, `self.*.value` is a `&mut` and cannot be moved
out of a borrow, so the implicit reborrow coercion turns it into
`self.*.value.*.&mut`. The walk dereferences `value` (`&'a mut`), which adds
`'a: 'r`, and then `self` (`&'s mut`), which adds `'s: 'r`. Returning the
result as `&'a mut` needs `'r: 'a`, so `'s: 'a` is required and cannot be
proven. Accepting it would allow two calls that each return a live
`&'a mut` to the same integer. The versions that are accepted either tie the
result to `'s`, or take `self` by value (`def into(self: RefMut['a]) -> &'a mut int32`).

The same rule decides how nested bodies may use their captures (see
[Nested Bodies](#nested-bodies)). An operation handler reaches its captures
through `&'env mut Env`, like `getRefMut`. A lambda or thunk owns its
environment, like `into`.

### Nested Bodies

The model is rustc's handling of closures, extended to `run` thunks and
operation handlers.

**Each body is checked on its own, innermost first.** Every lambda, thunk and
operation handler has its own CFG, so each is borrow-checked separately. Its
loans, liveness, and Polonius graph cover only that body. One borrow-check
query per definition processes the whole `IRFunctionMap` bottom-up. It builds
a nesting tree by scanning `Closure` and `Handle` expressions, recording each
nested function's parent and the point where the parent creates it.

**The interface between parent and child is a set of external regions.** A
nested function's external regions are every region in its interface type:
the owner arguments, parameter types, return type, effect row, and capture
types.

- For a lambda, these are exactly the arguments of its closure type.
- A thunk or an operation handler has no closure type, so the checker builds
  the same list itself. It contains the capture types, the effect row, and,
  for a handler, the operation's parameter and return types instantiated
  from the handled label.

Inside the child, external regions are **universal**: they are live at every
point and have no known relations to each other. When the child needs a
relation between external regions that it cannot prove, such as
`'ext1: 'ext2` or `t: 'ext1`, it does not report an error. It returns the
relation as a **requirement**, with each region identified by its position in
the interface. The parent maps those positions to its own regions at the
creation point and adds the requirements there. This is rustc's
`ClosureRegionRequirements`. With more than one level of nesting,
requirements pass up one level at a time.

A loan of one of the child's own locals that flows into an external region is
a real error in the child, for example returning a reference to a local.

**Capture loans belong to the parent.** A by-reference capture is a `RefOf`
evaluated in the parent before the `Closure` or `Handle` expression, so the
parent creates the loan. The closure value's type contains the capture region,
which stays live while the closure value is live. That is how
`closure_borrow_read` is rejected: the closure is still live when
`value = 42` runs.

**How a body reaches its captures depends on how it is invoked.** This is the
part that decides soundness:

| Body              | How it is invoked                                  | Rust analogue        | Access to captures             |
|-------------------|----------------------------------------------------|----------------------|--------------------------------|
| lambda            | `Def.call(fn: f, ...)` takes `f` **by value**      | `FnOnce` (or `Copy`) | environment owned by the call  |
| `run` thunk       | called exactly once by `Handle`                    | `FnOnce`             | environment owned by the call  |
| operation handler | called any number of times, one shared environment | `FnMut`              | through `&'env mut Env`        |

- **Lambdas and thunks.** A by-reference capture `x` is the place
  `Capture(x).*`, of type `&'h t`. Reborrows through it are constrained only
  by `'h`, so a lambda may return a reborrow of a `&mut` capture with lifetime
  `'h`. That is sound: a closure holding a `&mut` capture is not `Copy`, so it
  can be called only once.
- **Operation handlers.** Each handler body gets a fresh universal region
  `'env`. A capture root counts as one extra `&'env mut` dereference in the
  [reborrow walk](#reborrow-constraints), so reborrowing through a `&mut`
  capture adds `'env: 'r`. Copying a shared capture out needs no reborrow.
  The only known facts about
  `'env` are that each external region outlives it, which follows from the
  well-formedness of `&'env mut Env`. The existing `MoveOutOfHandlerCapture`
  diagnostic already applies the matching `FnMut` rule for moves.

Without `'env`, the following would be accepted:

```
let mut a = 1
run:
    let r = Reader.get()   # a reference into `a`
    Writer.update()        # writes `a` while `r` is live
    use(r.*)
with Reader, Writer:       # schematically: one shared environment
    def get(): return a.&
    def update(): a = 40
```

The loan in `get` and the write in `update` are in different bodies, so
neither body would see the conflict. With `'env`, `update` makes the shared
environment capture `a` as `&'h mut`. The reborrow `a.&` in `get` then needs
`'env: 'ret`, where `'ret` is an external region that `'env` cannot outlive,
and `get` is rejected with "captured variable cannot escape handler body".
This is the same error Rust reports for `FnMut` closures. If `a` were only
read, it would be captured as `&'h int32`, `get` could return a copy of that
reference with lifetime `'h`, and the loan of `a` would stay in the parent.

This rule is only sound if a handler environment never has two `&mut` borrows
live at once, which means a handler must never be re-entered while one of
its operations is running. Ray guarantees this by construction:

- **Typing.** An operation handler is typed with the effect row of the `run`
  expression, which is the row outside the handler and excludes the handled
  label (`compose_run_with_effect`). A handler that performs the effect it
  handles therefore reaches an **outer** handler for it, or fails to type.
- **Runtime.** Ray passes handlers as evidence. The handler record is built
  in the parent at the `Handle` point and is passed only to the body thunk
  (`lower_handle`). The evidence a handler uses for its own effects comes from
  the parent, so it cannot point back to the handler record that is running.
  A closure called inside a handler receives the handler's evidence at the
  call, so it cannot reach the running handler either.

Any future change that lets a handler reach its own record, for example
first-class handler values or handlers that capture evidence, must revisit
the `'env` rule.

**The `Handle` point in the parent.** The capture operands for the body thunk
and the handler environment are evaluated before the `Handle` expression, and
the `Handle` uses all of them. So when the body captures `a` by `&` and a
handler captures it by `&mut`, the parent reports E0502. At the same point
the parent adds the effect-row constraints from Phase 5, step 3 (the body's
row `<:` the handled label plus the outer row), and the requirements passed
up from the body and the handlers.

**Precision.** A requirement is a relation added at one point, the creation
point in the parent, so it is not location-sensitive inside the parent.
rustc accepts the same loss of precision. Lambda parameter and return regions
are external, so every call to the closure uses the same regions. This is the
missing higher-ranked support described in Phase 7.

## Phase 6: Polonius (Location-Sensitive Loan Liveness)

Input: the region constraints from Phase 5, loans, variable liveness from
Phase 0, and each region's variance within the type of each variable.

1. **Region liveness.** A region is live at point `p` if it appears in the type
   of a variable that is use-live at `p`, or in the "needs" set of a variable
   that is drop-live at `p`.
2. **Localized constraint graph.** Nodes are `(region, point)`. Edges:
   - an outlives constraint `'a: 'b` at `p` becomes an edge from `('b, p)` to
     `('a, p)`, so loans flow from the shorter region into the longer one. Its
     effect is visible at the successor points of `p`;
   - **liveness edges**: for each CFG edge `p -> q` and each region `'r` live
     at `q`, an edge from `('r, p)` to `('r, q)`. rustc's `-Zpolonius=next`
     orients these edges by the region's variance where it occurs: forward
     for covariant, backward for contravariant, both for invariant. This plan
     follows it.
3. **Loan reachability.** A loan `L` created at `p` into `'r` starts at
   `('r, p)` and is propagated along the graph. `L` is **live** at `q` if it
   reaches some `('x, q)` where `'x` is live at `q`. Propagation stops at a
   point that **kills** `L`: an assignment that overwrites a prefix of the
   loan's place, such as `p = ...` killing loans of `p.*`.
4. **Invalidations.** At each point, each access is checked against the live
   loans:
   - a write, or a `.&mut`, conflicts with any live loan of an overlapping
     place;
   - a read, or a `.&`, conflicts with any live mutable loan of an overlapping
     place;
   - a move conflicts with any live loan of an overlapping place;
   - a `ScopePop` conflicts with any live loan of a local in that scope whose
     path does not pass through a `Deref`. A loan of `tmp.*` borrows data
     behind the reference, which outlives `tmp`'s storage, so `f().*.&` is
     accepted. A loan of `tmp` itself is still E0716. This matches rustc;
   - a function return conflicts with any loan of a local that reaches a
     universal region.

   **Place overlap** compares roots, then projections pairwise. Different
   fields are disjoint, the same field overlaps, and a prefix overlaps its
   extensions. Past a `Deref` of a shared reference, a read cannot conflict.
5. **Universal region check** (not location-sensitive). Compute the transitive
   closure of the outlives constraints; `rayc_transitive_closure` exists and
   can be reused. For every pair of universal regions `'a: 'b` it implies,
   check that the pair is a known relation. Otherwise report "lifetime may not
   live long enough". A loan of a local reaching a universal region is the
   "returns a reference to a local" error (E0515) or E0597, depending on the
   case.

This is a scaled-down version of the design in the blog posts. The first
implementation can use plain worklist propagation over the graph. The posts
describe more efficient formulations, which are only needed if performance
becomes a problem.

### Diagnostics

Rust's error vocabulary, with Ray spans. Each error should point at the loan,
the conflicting access, and the later use that keeps the loan live, like
rustc's three-part message.

| Error                                        | Rust code  |
|----------------------------------------------|------------|
| cannot borrow as mutable more than once       | E0499      |
| cannot borrow as mutable because also borrowed as shared (and the reverse) | E0502 |
| cannot assign to a borrowed place            | E0506      |
| cannot move out of a borrowed place           | E0505      |
| borrowed value does not live long enough       | E0597      |
| temporary value dropped while borrowed         | E0716      |
| cannot return a reference to a local           | E0515      |
| lifetime may not live long enough              | —          |

## Phase 7: Rust Parity Extras

These can come after the core checker works, but Rust accepts code that needs
them, so they are part of the baseline:

- **Two-phase borrows** for `.&mut` auto-introduced by reborrow coercions at a
  call, so `f(v.&mut, len(v.&))` type-checks. How much Ray needs this depends
  on whether it gains autoref for method calls.
- **Higher-ranked callable parameters.** Rust reads `Fn(&T) -> &T` as
  `for<'a> Fn(&'a T) -> &'a T`. Ray's `fn: def(&int32) -> &int32` shorthand
  expands to a hidden callable type and a `Def[f]` dictionary. Without
  higher-ranked bounds, the elided lifetime has to become an ordinary lifetime
  parameter of the enclosing function, so the callback could not accept
  references to the callee's own locals. Parity needs placeholder regions and
  universes in the solver. Until that exists, **reject** elided lifetimes in
  callable parameter types instead of silently choosing a weaker meaning.
- **Late-bound lifetimes** on `def`s used as values. This only matters if
  named functions can be passed as `Def` values. Direct calls already
  instantiate fresh regions at every call.

# Impact on Existing Fixtures

Some current fixtures rely on unchecked pointers and become errors under
Rust's rules. For example, `run/closure/closure_borrow_read` captures `value`
by reference, assigns `value = 42` while the closure is still live, and then
calls it. That is E0506 in Rust. Following the project's rule, these fixtures
get fixed rather than weakening the checker:

- move the ones that demonstrate a genuine error into `check/borrow/...` as
  expected-error snapshots;
- rewrite the rest (for example with `move` captures) so they keep testing
  what they were meant to test.

Phase 1 should audit every fixture that uses `.&`, `.&mut`, by-reference
captures, or handler captures, and record which ones will turn into errors
once Phase 6 lands.

# Testing

Per the project's test strategy, the observable contract is e2e: `check`
fixtures with diagnostics snapshots, and `run` fixtures that must still
execute. Key borrow-check fixtures:

- Each error in the diagnostics table, with a minimal program.
- NLL basics: a loan that ends at its last use, so a later mutation is
  accepted.
- The flow-sensitive example from Polonius part 1, which NLL rejects and
  Polonius accepts:

  ```
  def main() -> ():
      let mut x = 22
      let mut y = 44
      let mut p = x.&
      let q = y.&mut
      if cond():
          p = q        # 'q flows into 'p only on this branch
          x = 23       # OK: the loan of `x` was killed by `p = q`
      else:
          y = 45       # OK: on this path `q` is dead and not in `p`
      read(p.*)
  ```

  The exact semantics should be checked against the blog post before the
  expected output is written.
- Returning a reference derived from a parameter (accepted) versus from a
  local (E0515).
- Closures: by-reference capture followed by mutation (error), `move` capture
  (accepted), closure requirement propagation, including through two levels
  of nesting, and a lambda returning a reborrow of its `&mut` capture
  (accepted).
- Handler environments: a handler returning a reborrow of a `&mut`-captured
  variable (error, "captured variable cannot escape handler body"), and one
  returning a copy of a shared capture (accepted).
- Handlers: a handler capturing by `&mut` while the body reads the same
  variable (error). The current `effect_handler_shared_capture` shape stays
  accepted.
- Effect variance: a callee needing `Reader['x]` for a local `'x` runs under a
  `Reader['static]` handler (accepted), while a callee needing
  `Reader['static]` under a handler returning a reference to a local
  (error). The mirror pair for a covariant `Log['a]`, and an invariant
  parameter used in both positions (error in both directions).
- Drops: a struct with a user `Drop` holding a reference that is dropped after
  the referent (error), and the same struct without a user `Drop` (accepted).
- Variance: passing a `&'long` where `&'short` is expected (accepted), and the
  same through `&mut &'long` (error).
- Reborrows: the `getRef` / `getRefMut` / `into` trio from
  [Reborrow Constraints](#reborrow-constraints), a reborrow through
  `& &mut` (the walk stops at the shared dereference), and a reborrow through
  a raw pointer (no constraint).

Unit tests are justified for variance computation, inferred outlives, and
place-overlap logic, because each has a small, precise contract that is hard
to exercise exhaustively end to end.

# Decisions

These were the plan's open questions. Each one is now decided, and the text
above has been updated to match.

1. **`unsafe`.** Dereferencing a raw pointer requires `unsafe`. Raw pointer
   dereferences use a separate `RawDeref` projection: assignment through it is
   a plain write without dropping the old value, and a load is a plain bitwise
   copy (see [References](#references) and
   [Deref as a Projection](#deref-as-a-projection)).
2. **Lifetime-dependent marker implementations** such as
   `impl Copy for S['static]` are rejected, a deliberate deviation from Rust,
   at least for now.
3. **Unused type parameters** are treated as invariant. There is no
   `PhantomData` equivalent for now.
4. **`&self` elision rule.** Not added for now.
5. **Enums and `Option`.** Acknowledged limitation: until Ray has sum types,
   the classic NLL/Polonius "problem case #3" (conditionally returning a borrow
   from a lookup) can only be approximated in tests.
6. **Higher-ranked bounds.** Elided lifetimes in callable parameter types are
   rejected. Higher-ranked lifetimes will be investigated later (Phase 7).
7. **Closure variance.** Closure types are invariant in all their arguments,
   which matches rustc. This concerns the closure type, not the `Def` trait,
   whose arguments are invariant like every trait's.
8. **Handler re-entry.** Not a problem: a handler cannot be re-entered while
   one of its operations is running, by construction of both typing and
   evidence passing (see [Nested Bodies](#nested-bodies)).

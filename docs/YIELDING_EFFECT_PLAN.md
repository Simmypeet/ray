# Yielding Effect: First Prototype Plan

Status: plan. Nothing here is implemented yet.

This is the implementation plan for the first prototype of
[YIELDING_EFFECT.md](YIELDING_EFFECT.md). That document is the design; this
one only says what to build, in which order, and how each step is checked.

## Scope

The prototype gets the **checks** right. A program that uses coroutines can
be type checked, memory checked and borrow checked, and gets the right
diagnostics. It cannot be built or run yet.

In scope:

- the surface: `coro`, `handle`, invoking a continuation;
- the coroutine types, the `Coroutine[c]` trait and `Continuation[r, c]`;
- the IR: suspension points, abandon edges, `coro` and `handle` lowering;
- memory analysis and the borrow checker on that IR;
- saved locals and marker entailment.

Out of scope:

- the state machine, the storage layout, and everything under "MonoIR
  update". MonoIR lowers a suspension point as the plain call it is today,
  and **panics** on a coroutine construction or a resume.
- `run ... with` inside a coroutine. Its IR shape and its checks are in
  scope; which bodies get reified is a MonoIR question.
- yielding local borrows. The design is still being researched, so a
  `perform` cannot pass a borrow of a local, as today.
- the open questions of the design: the bound syntax `coro d.m`, linear
  coroutines, per-effect compilation modes, moving and pinning.

So `check` accepts or rejects every program. `build` works for every
program that builds today, and panics on one that uses `coro` or `handle`.
`check` never reaches MonoIR, so the test suite does not see the panic.

## Phases at a glance

Each phase lands on its own and leaves every existing fixture green.

| Phase | What lands                                   | Visible change                    | Size |
| ----- | -------------------------------------------- | --------------------------------- | ---- |
| 0     | Borrow checking becomes its own query        | none                              | S    |
| 1     | `Suspend` terminator, resume edge only       | none                              | M    |
| 2     | Coroutine type, `Coroutine[c]`, `coro`       | `coro` type checks                | L    |
| 3     | Invoking a continuation, `handle`            | the design's examples type check  | L    |
| 4     | Abandon edges                                | new errors in effectful functions | M    |
| 5     | Closures and unresolved calls as coroutines  | `coro` on any call                | M    |
| 6     | Saved locals and markers                     | `c: Send` is answered             | L    |

Phases 0 and 1 are refactors. Phase 4 is the only one that can reject a
program that is accepted today.

## Phase 0: Borrow checking out of `BuildIR`

**Why.** `build_ir_executor` (`ir_builder/src/query.rs`) lowers, runs memory
analysis, borrow checks, erases lifetimes and verifies, all in one query.
The [staging](YIELDING_EFFECT.md#staging) rule needs two layers there. The
saved locals of `g` come from the IR of `g` after memory analysis. The
deferred marker goals of `f` are proved in the borrow checker phase of `f`,
and need the saved locals of `g`. If borrow checking stays inside `BuildIR`,
a function that asks a marker of its own coroutine is a cyclic query.

**Changes.**

- A new query, `BorrowCheck { def_id }`, takes the result of `BuildIR`, runs
  `borrow_check` on a copy and returns only diagnostics. It keeps today's
  gate: it runs only when nothing earlier reported an error.
- `BuildIR` keeps lowering, memory analysis and verification. Where
  `erase_lifetimes` goes has to be settled here, because the borrow checker
  needs the lifetimes that `BuildIR` erases today.
- The rendered-diagnostics query of `rayc_ir_builder` includes the new
  query.

**Done when** every `check` and `run` snapshot is unchanged.

## Phase 1: Suspension points

**Goal.** The IR ends a block at every suspension point. Nothing is checked
differently yet. See
[suspension points](YIELDING_EFFECT.md#suspension-points).

**Changes.**

- `rayc_ir` (`cfg.rs`): `Terminator::Suspend { expression, resume, abandon }`
  with `abandon: Option<BlockID>`, always `None` in this phase. The
  expression stays an `IRExprKind::Call`, `Perform` or `RunWith`.
- `rayc_ir_builder` (`expression/call.rs`, `expression/run_with.rs`): end
  the block with a `Suspend` for every `perform`, every call whose effect
  row is not empty, and every `run ... with` that has an effect of its own.
  One helper decides "has an effect": the row is not syntactically empty, so
  a row variable counts.
- `rayc_ir` (`liveness/local.rs`, `liveness/expr.rs`): the terminator uses
  the operands of its expression and defines the value on the resume edge.
- `rayc_memory`: the moves of the arguments happen at the terminator.
- `rayc_borrowck` (`constraint.rs`, `conflict.rs`, `region_liveness.rs`):
  collect and check at the terminator what is collected and checked at the
  expression instruction today.
- `rayc_ir_builder` (`verification.rs`): accept the new terminator.
- `rayc_mono_ir_builder` (`lower/cfg.rs`): lower `Suspend` as the expression
  followed by a jump to `resume`.

**Done when** every `check` and `run` snapshot is unchanged. This is the
regression gate for the whole IR change: `run` fixtures prove that the
direct form still executes.

**Risk.** Any code that assumes a `Call`, `Perform` or `RunWith` is evaluated
by an `Instruction::Expression`.

## Phase 2: The coroutine type and `coro`

**Goal.** `coro f(args)` type checks and is borrow checked. See
[representation](YIELDING_EFFECT.md#representation),
[dropping a coroutine](YIELDING_EFFECT.md#dropping-a-coroutine) and
[`coro`](YIELDING_EFFECT.md#coro).

**Changes.**

- `corelib/core.ray`, `symbol/src/core_item.rs`: `pub trait Coroutine[c]`
  with `Return` and `Effect`, and `pub struct Continuation[r, c]` with one
  private field of type `c`.
- `rayc_type` (`ty/application.rs`, `ty.rs`): `Constant::DefCoroutine`, keyed
  by a `def_id`, applied to `poly_args`. Every exhaustive match on
  `Constant` and `View` gets an arm: invariant in all arguments, decomposed
  into its arguments for outlives.
- `rayc_type` (`reduce.rs`), `rayc_solver` (`instance_resolution.rs`): a
  built-in `Coroutine[c]` instance for a coroutine type and for a `&mut` to
  one, next to the built-in `Def` instance of a closure. `Return` and
  `Effect` reduce by substituting `poly_args` into the signature.
- `rayc_solver` (`solver/marker_entailment.rs`): a coroutine type is never
  `Copy`. The check sits before the givens, next to the one for linear
  structs.
- `Drop`: `Constant::CoroutineDropInstance`, the coroutine type followed by
  one dictionary per upvar, resolved next to
  `resolve_closure_drop_instance`. For the borrow checker, this drop needs
  every region in `poly_args`.
- Syntax (`lexical/src/kind.rs`, `syntax/src/expression.rs`): the `coro`
  keyword and `coro <call>`.
- Typed AST: a `Coro` expression with the call target and the arguments. Its
  type is `Continuation[(), &mut <coroutine type>]`. Evaluating it performs
  only the effects of its arguments. In this phase the target must be a
  direct call of a `def`; anything else is a "not supported yet" error.
- `rayc_ir_builder`: the hidden variable in the enclosing block scope, a new
  `IRExprKind::Coroutine`, a `Store`, a `RefOf` and the `Continuation`.
- `rayc_memory`: a missing upvar dictionary is reported at the `coro`.
- `rayc_mono_ir_builder`: panic on `IRExprKind::Coroutine`.

**Fixtures** (`check/coroutine/...`):

- `coro_basic`: accepted.
- `continuation_escapes`: `return coro f()` is "does not live long enough".
- `coro_in_loop`: accepted, the storage is dropped every iteration.
- `continuation_not_copy`: using one twice is a use after move.
- `coro_linear_argument`, `coro_argument_missing_drop`: errors at the
  `coro`.
- `coroutine_drop_uses_borrow`: the `x = 2` example under
  [lifetimes](YIELDING_EFFECT.md#lifetimes).
- `coro_not_a_call`: error.
- `coroutine_trait_bounds`: a generic function over `c` with
  `where (co.Return = ..., co.Effect = ...)`, accepted and mismatched.

## Phase 3: Resuming

**Goal.** The examples of the design document type check. See
[handling a coroutine](YIELDING_EFFECT.md#handling-a-coroutine-handle) and
[`handle`](YIELDING_EFFECT.md#handle).

**Changes.**

- **Invoking.** A built-in `Def` instance for `Continuation[r, c]`, with
  `Args = (r,)`, `Return = Coroutine[c].Return` and
  `Effect = Coroutine[c].Effect`. `k(v)` is then an ordinary call that takes
  `k` by value, and an effectful one, so it is already a `Suspend`. No new
  expression is needed.
- **Syntax.** The `handle` keyword, the operation clauses
  `case Label.op(p) resume k:` and the `case return x:` clause.
- **Typed AST.** A `Handle` expression whose clauses are arms, like the arms
  of an `if`. The clause parameters and `k` are bindings of the enclosing
  function. `break`, `continue` and `return` resolve to the enclosing loop
  and function. The typing rules are those under
  [typing](YIELDING_EFFECT.md#typing).
- **IR.** `Terminator::Handle { continuation, value, return, clauses,
  abandon }`, with `abandon: None` in this phase. Each successor starts with
  the values the terminator gives it.
- **Analyses.** Memory analysis moves the continuation. Liveness and the
  borrow checker treat each clause as a successor.
- **MonoIR.** Panic on `Handle` and on the built-in `Def` instance.

**Fixtures:**

- `generator_loop`: the [example](YIELDING_EFFECT.md#example), accepted.
- `resume_wrong_type`: type mismatch.
- `resume_twice`: use after move.
- `handle_unknown_label`, `handle_opaque_row`: the label is not visible in
  the effect.
- `clause_unknown_operation`, `clause_duplicate`, `clause_arity`: errors.
- `answer_type_mismatch`: two clauses with different types.
- `clause_break_and_return`: accepted, and they act on the enclosing
  function.
- `continuations_of_different_types`: the `State` example, and storing `k1`
  and `k2` in one variable as an error.
- `recursive_driver`: the `supply` example, accepted.

## Phase 4: Abandon edges

**Goal.** A coroutine can be dropped while it is suspended, and the rules of
[what a function may hold across a suspension](YIELDING_EFFECT.md#what-a-function-may-hold-across-a-suspension)
are enforced.

**Changes.**

- `rayc_ir`: `Terminator::Abandon`. Every `Suspend` has an abandon block,
  and so does a `Handle` that forwards effects.
- `rayc_ir_builder`: the abandon block is the path an early `return` takes
  at that point, ending in `Abandon`.
- `rayc_memory`: the abandon edge is an ordinary edge, so the existing
  balancing selects its drops. New diagnostics for a value that is live
  across a suspension and has no `Drop` dictionary, and for a `@linear`
  one.
- `rayc_ir_builder` (`verification.rs`): scopes balance on abandon paths.
- `rayc_mono_ir_builder`: abandon blocks are not lowered.

**Fixtures:**

- `linear_across_suspension`: error.
- `opaque_across_suspension`: error without `Drop[t]`, accepted with it.
- `consumed_before_suspension`: a `@linear` parameter consumed before the
  first `perform`, accepted.
- `linear_argument_to_effectful_call`: accepted.
- `drop_on_abandon_is_a_use`: a value with a user-written `Drop` that holds
  a borrow across a suspension keeps the loan live.

This phase needs the audit under
[impact on existing code](#impact-on-existing-code).

## Phase 5: Other callees

**Goal.** `coro` works on any call. See
[nested bodies](YIELDING_EFFECT.md#nested-bodies) and
[coroutines of unresolved calls](YIELDING_EFFECT.md#coroutines-of-unresolved-calls).

**Changes.**

- `Constant::DefCoroutine` gets a second key, the identity of a closure, with
  the closure type as its argument. `coro` accepts a call of a closure.
- `Constant::TraitDefCoroutine`, keyed by the trait method, applied to the
  instance and the method's own arguments. It reduces in `reduce.rs` the
  way an associated type of an instance does. `coro` accepts a call through
  an instance, including a callable value.
- The built-in `Coroutine[c]` and `Drop` instances cover both.

**Fixtures:** `coro_closure`, `coro_instance_method`, `coro_callable`, and
`instance_coroutine_reduces`: a generic function instantiated with a
concrete instance.

## Phase 6: Saved locals and markers

**Goal.** `c: Send` is answered, with a diagnostic that names what is held.
See [saved locals](YIELDING_EFFECT.md#saved-locals),
[markers](YIELDING_EFFECT.md#markers) and
[staging](YIELDING_EFFECT.md#staging).

**Changes.**

- A query for the typed saved locals of a body. Its key is declared in
  `rayc_semantic_element`, so that the solver can call it, and its executor
  sits with `BuildIR`. It reads the liveness at every `Suspend` and
  forwarding `Handle`, adds the locals that may be borrowed while their
  storage is live, and adds what each expression waits for. Lifetimes are
  erased.
- `Constant::LocalCoroutine` for the body and the handlers of a
  `run ... with`. Only this query creates it.
- `rayc_solver` (`solver/marker_entailment.rs`): the structural rule for a
  `Coroutine` (upvars and saved locals) and for a `LocalCoroutine` (its
  embedded arguments). A goal that reaches a `Coroutine` has a third
  outcome, "deferred", wherever the saved locals may not be asked for.
- The typed-AST builder records deferred goals with the body. `BorrowCheck`
  proves them and reports the failures.

**Fixtures.** Each declares its own `marker Send`, as
`check/semantic/marker_entailment` does.

- `coroutine_send`: accepted.
- `not_send_saved_local`, `not_send_upvar`: errors that name the value.
- `drop_is_a_use`: the `guard` example under
  [what counts as saved](YIELDING_EFFECT.md#what-counts-as-saved).
- `borrowed_local_is_saved`: a borrowed local with a no-op drop.
- `callee_coroutine_is_saved`: the non-`Send` value is held by a callee.
- `run_body_is_saved`: it is held by the body of a `run ... with`.
- `marker_of_own_coroutine`: a function that asks `Send` of its own
  coroutine. No cycle, and a definite answer.
- `unreduced_instance_coroutine`: a marker goal on a call through an
  abstract instance fails, and the diagnostic says why.

## Decisions the prototype takes

The design document leaves these open, or says them differently. The plan
fixes them so the prototype can be built. Each is cheap to change later.

1. **Borrow checking is its own query** (phase 0).
2. **Invoking a continuation goes through a built-in `Def` instance.** The
   design describes it as a `Handle` with no clauses. The meaning is the
   same; the lowering can produce that `Handle` when MonoIR is written.
3. **Everything is invariant.** A coroutine type is invariant in its
   arguments. `Continuation[r, c]` is an ordinary struct, so its unused `r`
   is invariant. The label arguments at a `handle` are related invariantly.
   Deriving the variance of yielded arguments and resume values, which
   [borrow checker impact](YIELDING_EFFECT.md#borrow-checker-impact) asks
   for, is left for later.
4. **Clauses are spelled with `case`**, as in the design's examples, and
   every operation of an effect that has a clause needs one.
5. **`coro` takes a call**, and in `handle e1(e2)` the outermost call is the
   resume. `handle e1()` resumes with `()`, and `handle coro f(x)` starts
   the coroutine.
6. **No bound syntax.** A marker goal on a `TraitDefCoroutine` that does
   not reduce cannot be proved in the prototype.

## Impact on existing code

Phase 4 applies two new rules to every effectful function, whether or not
it is ever suspended. Following the project's rule, code that breaks is
fixed, and the checker is not weakened.

Before phase 4 lands, audit:

- **`corelib`.** Every effect-polymorphic function that holds a value of an
  opaque type across a call of its callback needs a `Drop` given.
- **`run` fixtures.** The same, for every function that holds a `t` across a
  `perform` or an effectful call.
- **`check/memory` fixtures with `@linear`.** A linear value held across an
  effectful call becomes an error.

A fixture that shows a real error moves to `check/coroutine/` as an
expected-error snapshot. The rest get the `Drop` given they are missing.

## Testing

The contract of the prototype is what `check` reports, so the tests are
end-to-end `check` fixtures: one directory per case under
`compiler/e2e/test/check/coroutine/`, with a `main.ray` and a
`snapshot.snap`.

```sh
cargo test -p rayc_e2e --test check_e2e
cargo test -p rayc_e2e --test run_e2e
```

- `check_e2e` runs with IR verification on, so every fixture also checks
  that the new terminators produce well-formed IR.
- `run_e2e` is the regression gate for phases 0, 1 and 4. No fixture there
  uses `coro` or `handle`, and all of them have to keep running.
- There are no `run` fixtures for coroutines until MonoIR exists.

One unit test is justified. Which drops memory analysis puts on an abandon
edge cannot be observed until a coroutine runs, and getting it wrong is
silent. It belongs with the existing tests in `rayc_memory`.

## After the prototype

In rough order: the state machine and the layout saved locals, MonoIR and
`run` fixtures, `run ... with` in a coroutine, the bound syntax, yielding
local borrows, and effect variance.

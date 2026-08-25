# Stage 2: Make Lambda Function Instances Explicit in Monomorphization

## Commit

`feat(mono): collect reachable lambda function instances`

## Goal

Represent each reachable IR lambda helper under the concrete substitution of its owning source def, and expose that inventory to code generation.

Today `rayc_mono` visits every function in an IR `FunctionMap` while collecting types and calls, but `MonoProgram` only records root def instantiations. C codegen therefore has no explicit work item for a nested lambda function.

## Dependency

Requires Stage 1 so an IR lambda context has a complete signature, including its return type.

## Model

Add a backend-independent lambda instance:

```rust
pub struct MonoLambda {
    owner: MonoFunction,
    function_id: rayc_ir::function::FunctionID,
}
```

`owner` supplies the source def and its concrete substitution. `function_id` selects the lambda function inside that def's IR `FunctionMap`.

Expose narrow construction/access APIs:

```rust
MonoLambda::new(owner, function_id)
MonoLambda::owner()
MonoLambda::function_id()
```

Derive the same equality, ordering, and hashing traits used by `MonoFunction`. Do not duplicate the substitution or copy IR capture/signature data into this model.

Extend `MonoProgram` with a private `FxHashSet<MonoLambda>` and a `lambdas()` iterator.

## Reachability Algorithm

Replace the current unconditional loop over `ir.functions()` with a traversal rooted at `FunctionMap::root_id()`:

1. Collect the root def signature from semantic-element queries as today.
2. Visit the root IR function.
3. Collect the current function context types:
   - lambda parameter types;
   - lambda return type;
   - capture pointee types.
4. Collect reachable variable and expression types.
5. For a direct call, enqueue the instantiated callee def exactly as today.
6. For `ExpressionKind::MakeLambda`, create and insert `MonoLambda(owner.clone(), target_id)`, then enqueue/visit that target IR function.
7. For an indirect lambda call, collect its already-recorded expression and argument types but do not invent another function instance.
8. Deduplicate visited local `FunctionID`s for each `MonoFunction` owner.

This makes `MakeLambda` the reachability edge between local IR functions. A lambda function present in the arena but not referenced by a reachable `MakeLambda` must not cause helper emission or introduce direct callees into the mono work queue.

Validate while traversing:

- the root function has `Context::Def`;
- a `MakeLambda` target exists in the same `FunctionMap`;
- every target has `Context::Lambda`;
- a target is compatible with the signature already stored on its `MakeLambda` expression;
- no lambda is owned by a different source def substitution.

Invariant failures should panic with actionable compiler-internal messages, not become recoverable user diagnostics at this stage.

## Files

### `compiler/codegen/mono/src/model.rs`

- Add `MonoLambda`.
- Add the private lambda-instance set to `MonoProgram`.
- Add insertion and iteration methods.

### `compiler/codegen/mono/src/collect.rs`

- Traverse local functions from the root through reachable `MakeLambda` nodes.
- Collect Stage 1's lambda return type.
- Register each discovered `MonoLambda`.
- Continue applying the owning `MonoFunction` substitution to every inspected type.
- Keep enum matches exhaustive.

### `compiler/codegen/mono/src/lib.rs`

- Re-export `MonoLambda` with the existing mono model types.

## Test Strategy

Do not add a formatting-oriented or field-storage unit test for `MonoLambda`.

The meaningful contract is that lambda helpers, nested direct calls, and polymorphic capture layouts appear in generated executable behavior. Stage 5's run-level fixtures exercise that stable boundary. During this isolated commit, run the existing mono consumers and workspace checks to catch API and exhaustive-match fallout.

If a small existing engine fixture can query `collect_target` without introducing a second semantic-pipeline test harness, one focused collector test may assert that a reachable `MakeLambda` is inventoried and an unreferenced arena function is not. Otherwise defer this assertion to the Stage 5 end-to-end fixtures.

Run:

```sh
cargo check -p rayc_mono -p rayc_c
cargo test -p rayc_e2e --test check_e2e
cargo clippy --workspace --all-targets
cargo +nightly fmt --check
```

## Acceptance Criteria

- `MonoProgram` separately exposes concrete root defs, lambda helpers, tuple types, and lambda signature types.
- Every reachable `MakeLambda` produces exactly one `MonoLambda` per owning def substitution.
- Direct calls inside lambda bodies participate in the normal mono work queue.
- Polymorphic types in parameters, returns, captures, variables, and expressions are instantiated with the owner substitution.
- Local functions are reached through `MakeLambda`, not by unordered arena iteration.

## Non-Goals

- No C-specific names or layout IDs in `rayc_mono`.
- No environment allocation policy.
- No C helper declarations or bodies.


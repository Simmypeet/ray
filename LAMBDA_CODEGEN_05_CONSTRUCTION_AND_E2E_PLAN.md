# Stage 5: Construct C Closures and Add End-to-End Coverage

## Commit

`feat(c): generate and execute lambda expressions`

## Goal

Lower `ExpressionKind::MakeLambda` to a concrete C closure value, connect it to the helper and environment emitted by Stages 3-4, and protect the complete source-to-native pipeline with run-level fixtures.

This commit removes the last intentional lambda-codegen panic.

## Dependency

Requires Stages 1-4.

## Environment Storage Policy

Use one function-scope C environment object for each reachable capturing `MakeLambda` expression:

```c
struct RayLambdaEnv_<target> ray_lambda_env_<expression-id>;
```

Declare these objects with the other function locals before CFG labels. This avoids invalid/less-portable declarations immediately after labels and gives the environment the lifetime of the enclosing C function invocation.

At the IR instruction that evaluates `MakeLambda`, assign the capture fields in layout order:

```c
ray_lambda_env_E = (struct RayLambdaEnv_<target>){
    .ray_capture_0 = ray_expr_capture_0,
    .ray_capture_1 = ray_expr_capture_1,
};
```

Then construct the signature-level closure value:

```c
ray_expr_E = (RayLambda_<signature>_t){
    .call = ray_<def>_<subst>_lambda_<target>,
    .env = &ray_lambda_env_E,
};
```

For a captureless lambda, declare no environment object and use a null erased environment such as `(void *)0`.

The current closure conversion captures source bindings by reference. Function-scope stack storage therefore matches the intended non-escaping borrow/lifetime invariant and avoids allocation. Do not add heap allocation, leaking, reference counting, or hidden copies in the C backend.

Until semantic lifetime checking is implemented, a captureful closure that escapes its enclosing function remains outside the safe supported subset. Captureless closures may escape because their environment is null. Record this limitation in code comments near environment storage; do not silently claim the backend has made escaping references safe.

## MakeLambda Emission

Add `compiler/codegen/c/src/expression/make_lambda.rs` and an exhaustive dispatch arm in `expression.rs`.

The writer should:

1. Resolve `MakeLambda::function_id()` in the current owner's IR `FunctionMap`.
2. Validate that the target is `Context::Lambda`.
3. Build the target `MonoLambda` identity using the current owner substitution.
4. Validate that capture operand count equals target capture count.
5. Zip operands with target captures in ordered layout order.
6. Emit the environment assignment before the closure-value assignment when captures exist.
7. Instantiate the `MakeLambda` expression type and require a C lambda signature type.
8. Emit a closure compound literal containing the helper function and erased environment pointer.

Capture operands are already-defined pointer expressions emitted left-to-right by IR lowering. Store those pointer values directly into environment fields; do not take their addresses again.

## Function Layout

Extend `FunctionLayout` in `compiler/codegen/c/src/ir.rs` to identify reachable capturing `MakeLambda` expressions.

- Declare exactly one environment object per reachable capturing expression.
- Use the target lambda instance, not the enclosing function, to select the environment struct type.
- Keep environment object identity based on the enclosing expression ID so two constructions of the same lambda target have distinct storage slots.
- Reuse the slot if CFG control evaluates the same IR expression again.

Special-case the `MakeLambda` instruction only for the environment assignment that must precede the normal expression result assignment. Keep closure literal formatting in its expression writer.

## Existing Indirect Calls

Retain the current indirect-call ABI:

```c
closure.call(closure.env, ordinary_arguments...)
```

With a real closure value, this existing path should work without another call representation change. Confirm zero-argument calls do not emit a stray comma.

## Files

- `compiler/codegen/c/src/expression.rs`
- `compiler/codegen/c/src/expression/make_lambda.rs` (new)
- `compiler/codegen/c/src/ir.rs`
- `compiler/codegen/c/src/identifier.rs`
- `compiler/codegen/c/src/context.rs` or narrow Stage 3 delegation APIs as needed
- `compiler/e2e/test/run/<lambda fixtures>/main.ray`
- corresponding `snapshot.snap` files

Avoid changes to TypedAST or closure conversion unless an end-to-end fixture exposes a genuine upstream bug.

## End-to-End Test Matrix

Use `compiler/e2e/test/run/` because the protected contract is generated C compilation plus executable behavior. Prefer several small fixtures over one opaque program.

### Captureless construction and invocation

Protect:

- lambda parameter addressing;
- helper call ABI;
- null environment;
- returning/calling a captureless lambda.

Representative source shapes:

```text
let increment = (value) -> value + 1
return increment(40)
```

and a captureless lambda returned from a def and called by its caller.

### Read-only and mutable captures

Protect:

- a def parameter captured by reference and read;
- an outer mutable local updated by a lambda;
- the updated value observed after the call;
- immutable versus mutable pointer field types compiling correctly.

Representative shape:

```text
let mut value = 1
let update = () -> value = 42
update()
return value
```

### Nested transitive capture

Protect:

- a parent helper loading its capture pointer;
- explicit reborrow into a child environment;
- child invocation while the parent frame is still alive;
- mutation observed in the root.

Use an immediately invoked nested lambda rather than returning a captureful inner closure, for example the equivalent of:

```text
let outer = () -> (() -> value = 32)()
outer()
```

Adjust parentheses to the accepted parser syntax while preserving the lifetime shape.

### Polymorphic owner substitution

Protect that a lambda inside a polymorphic def gets concrete helper signatures and capture fields for each reached substitution. Exercise at least two concrete instantiations if they can contribute to one executable fixture.

### Optional tuple projection case

If tuple capture projection is not already exercised by the cases above, add a small fixture that reads or writes a tuple element through a capture. Do not add a unit test for the corresponding private C syntax.

## Verification

Run the narrow suite while iterating:

```sh
cargo test -p rayc_e2e --test run_e2e
cargo test -p rayc_ir_builder
cargo check --workspace
```

Then run the repository gates:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
cargo +nightly fmt --check
```

When debugging, also emit a `.c` artifact for one capturing and one nested fixture and compile it with the same `cc` path used by the driver. Do not make generated-C text snapshots the primary test contract; executable results are more stable.

## Acceptance Criteria

- No valid reachable `MakeLambda` hits a C-codegen panic.
- Captureless, parameterized, capturing, mutating, nested, and polymorphic lambdas compile to native executables.
- Closure construction preserves capture operand/layout order.
- Indirect calls pass the erased environment followed by ordinary arguments.
- Capture fields preserve IR mutability as C pointer constness.
- Nested lambda helpers and environments use the owning def substitution.
- All new user-visible behavior is covered by run-level fixtures.
- Workspace tests, Clippy, and nightly formatting pass.

## Follow-Up Boundary

Backend completion does not itself prove that captured references outlive a closure. A later semantic lifetime/borrow-checking stage must reject escaping captureful closures (and other invalid reference escapes). That work should be planned independently because it changes language acceptance and diagnostics rather than closure code generation.


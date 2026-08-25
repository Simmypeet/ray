# Stage 1: Complete the Backend-Facing IR Lambda Contract

## Commit

`feat(ir): retain lambda return types in IR functions`

## Goal

Make every IR lambda function self-describing enough for a backend to emit its C function signature without searching for the parent `MakeLambda` expression that created it.

The current IR lambda context owns ordered parameters and captures, but it does not own a return type. That is the last missing part of the function signature needed by monomorphization and C code generation.

This stage should remain backend-independent. Do not put C layout, closure-environment, or symbol-naming details into `rayc_ir`.

## Dependency

This is the first stage. It builds on the completed TypedAST-to-IR closure conversion described in `LAMBDA_IR_PLAN.md`.

## Design

Extend `rayc_ir::lambda::LambdaContext` with:

```rust
return_ty: Interned<Ty>,
```

Expose a narrow `return_ty()` method. Keep the field private.

The lambda context then represents this backend-neutral signature and environment contract:

```text
parameters: ordered ordinary arguments
return_ty:  function result
captures:   ordered closure-environment fields
```

The complete lambda `Ty` does not need to be stored a second time. The ordered parameter types plus `return_ty` already describe the signature, while captures describe the environment separately.

## Implementation

### `compiler/semantic/ir/src/lambda.rs`

- Add `return_ty` to `LambdaContext`.
- Replace the default/zero-argument constructor with `LambdaContext::new(return_ty)`.
- Add `LambdaContext::return_ty()`.
- Preserve all required `StableHash`, `Encode`, `Decode`, and `Identifiable` behavior.
- Do not expose the context fields or its internal arenas.

### `compiler/semantic/ir/src/function.rs`

- Change `FunctionMap::insert_lambda` to accept the return type.
- Change `Function::new_lambda` to accept the return type and pass it to `LambdaContext::new`.
- Keep `Function::new`/`Default` for root def functions unchanged.

### `compiler/semantic/ir_builder/src/expression/lambda.rs`

- Read the already-solved lambda expression type before lowering its child function.
- Match it explicitly as `TyApplicationView::Lambda` and copy its return type.
- Treat any inference, polymorphic, error, primitive, tuple, or pointer shape here as a compiler-internal invariant violation. Match project-owned enums exhaustively; do not add a wildcard arm.
- Pass the return type into `Builder::lower_lambda_function`.

### `compiler/semantic/ir_builder/src/builder.rs` and `builder/function_build_state.rs`

- Thread the return type through `lower_lambda_function`, `start_lambda`, and `FunctionBuildState::new` only for lambda functions.
- Keep root-def construction separate so callers cannot accidentally attach a lambda signature to a def.
- When the `TypedContext` is `Lambda`, require the return type and call `ir_functions.insert_lambda(return_ty)`.
- When the `TypedContext` is `Def`, require that no lambda return type was supplied.

Prefer a small enum or separate constructors if that makes the def/lambda invariant clearer than an unstructured `Option<Interned<Ty>>`.

## Validation

Extend the existing focused IR-builder lambda test in `compiler/semantic/ir_builder/src/tests.rs`:

- Build a captureless lambda whose return type differs from its parameter type.
- Lower it.
- Assert that the target IR lambda context retains the exact solved return type.
- Keep the assertion at the public IR API boundary; do not test constructor call order or private storage.

This unit coverage is justified because the lambda function signature is a stable IR contract consumed by every backend and cannot yet be observed through C codegen until later stages.

Run:

```sh
cargo test -p rayc_ir_builder
cargo check -p rayc_ir -p rayc_ir_builder
cargo clippy --workspace --all-targets
cargo +nightly fmt --check
```

## Acceptance Criteria

- Every `Context::Lambda` has an ordered parameter list, a return type, and an ordered capture list.
- C codegen can obtain a lambda helper signature from the target IR function alone.
- Root def functions are unchanged.
- Invalid non-lambda types cannot silently initialize an IR lambda context.
- Existing closure-conversion tests continue to pass.

## Non-Goals

- No monomorphization model changes.
- No C symbol, environment, or helper-function generation.
- No closure escape/lifetime checking.


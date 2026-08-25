# Stage 3: Add C Lambda Helper and Environment Declarations

## Commit

`feat(c): declare lambda helpers and capture environments`

## Goal

Turn each `MonoLambda` into deterministic C-level identities, a capture-environment struct when needed, and a helper-function prototype compatible with the existing erased closure ABI.

The existing signature-level closure representation remains the public ABI:

```c
struct RayLambda_<signature> {
    ReturnType (*call)(void *env, ParameterTypes...);
    void *env;
};
```

This stage adds implementation-specific pieces for each lambda expression without changing that representation.

## Dependency

Requires Stage 2's explicit `MonoLambda` inventory.

## C Representation

For a concrete lambda instance owned by one `MonoFunction`, emit a helper identity based on:

```text
source def identity + substitution identity + IR FunctionID
```

Use a consistent shape such as:

```c
ray_<def>_<subst>_lambda_<function-id>
RayLambdaEnv_<def>_<subst>_<function-id>
```

Do not use pointer addresses, hash-map iteration order, or capture types alone as identity.

For a capturing lambda, emit:

```c
struct RayLambdaEnv_<instance> {
    const Captured0Type *ray_capture_0;
    Captured1Type *ray_capture_1;
};
```

The field type must be the monomorphized pointer type implied by each IR `Capture`'s pointee type and mutability. Capture order is the IR `LambdaContext::captures()` order and field names are based on `CaptureID`.

Do not emit an empty C struct for a captureless lambda. Its erased environment will be a null pointer in Stage 5.

Emit a prototype for every helper:

```c
ReturnType ray_<def>_<subst>_lambda_<id>(
    void *ray_raw_env,
    Parameter0Type ray_lambda_param_0,
    ...
);
```

The first parameter must remain exactly `void *` so the function pointer is compatible with the existing signature-level closure struct. Lambda parameter and return types come from the target IR lambda context and are instantiated with `MonoLambda::owner()`.

## Files

### `compiler/codegen/c/src/context/instantiation.rs`

- Add a small C lambda-instance identity derived from `MonoLambda`.
- Add deterministic sorting/access for mono lambda instances, as already done for tuples and lambda signature types.
- Add narrow helpers for writing lambda helper and environment identifiers.
- Keep C identity out of `rayc_mono`.

### `compiler/codegen/c/src/identifier.rs`

Add explicit identifier variants for:

- lambda helper functions;
- lambda environment struct tags;
- raw and typed environment parameters/locals;
- lambda parameters;
- capture fields;
- later per-`MakeLambda` environment values.

Use separate prefixes for def parameters and lambda parameters even when their arena indices happen to match.

### `compiler/codegen/c/src/context.rs`

Add narrow methods that:

- fetch a `MonoLambda`'s target function from its owner's IR map;
- instantiate its return, parameter, and capture-pointer types;
- validate that the selected function has `Context::Lambda`.

Do not expose the tracked engine or internal mono sets to writer modules.

### `compiler/codegen/c/src/write.rs`

- Write capture-environment struct definitions for capturing lambdas.
- Write helper forward declarations for all mono lambda instances.
- Keep root def declaration behavior unchanged.
- Match IR/type enums exhaustively.

### `compiler/codegen/c/src/translation_unit.rs`

- Sort root and lambda instances before emission so generated C is reproducible.
- Emit environment structs after composite type declarations/definitions and before function declarations.
- Emit lambda helper prototypes alongside root def prototypes.
- Preserve the requirement that all declarations precede definitions, allowing nested helpers to call one another through already-declared symbols.

Recommended translation-unit order:

```text
standard includes
tuple/lambda signature forward declarations
lambda signature and tuple definitions
lambda environment definitions
root def and lambda helper declarations
root def and lambda helper definitions
entry-point wrapper
```

## Validation

This is C emission plumbing whose meaningful behavior is compilation and execution, so do not add unit tests that snapshot private writer fragments. Stage 5 adds executable fixtures after closure construction exists.

Run:

```sh
cargo check -p rayc_c
cargo test -p rayc_e2e --test run_e2e
cargo clippy --workspace --all-targets
cargo +nightly fmt --check
```

The existing run suite must remain green. Lambda construction may still hit the existing explicit panic until Stage 5; do not replace it with a wildcard or dummy value.

## Acceptance Criteria

- Every `MonoLambda` has a deterministic helper name.
- Capturing lambdas have one environment struct field per ordered IR capture.
- Capture fields use concrete pointer types with the correct constness.
- Captureless lambdas do not rely on non-standard empty C structs.
- Every helper prototype exactly matches its signature-level closure function-pointer type after the erased environment parameter.
- Generated declaration order supports nested and mutually referenced emitted functions.

## Non-Goals

- No helper function bodies yet.
- No lowering of `AddressRoot::{LambdaParameter, Capture}` yet.
- No `MakeLambda` value construction or environment storage yet.


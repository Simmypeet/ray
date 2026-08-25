# Stage 4: Emit C Lambda Helper Bodies and Lambda Address Roots

## Commit

`feat(c): emit lambda helper bodies and captured addresses`

## Goal

Emit each monomorphized IR lambda function as a real C helper and make the existing CFG writer understand lambda parameters and capture slots.

After this stage, lambda bodies themselves are code-generatable. The enclosing `MakeLambda` expression remains the final missing operation handled in Stage 5.

## Dependency

Requires Stage 3's helper identities, environment structs, and prototypes.

## Function Instance Context

Generalize `compiler/codegen/c/src/expression/function_instance.rs` so an expression writer has all of:

```text
owning IR FunctionMap
current IR FunctionID
current IR Function
owning MonoFunction substitution
```

Keep fields private and expose delegation methods for:

- getting the current or a target expression/function;
- instantiating an expression type;
- instantiating a direct call under the owner substitution;
- determining whether the current function is a root def or lambda;
- constructing the `MonoLambda` identity for a local target.

Do not expose the `FunctionMap` merely so unrelated writers can navigate it directly.

## Helper Definition

For each `MonoLambda`:

1. Fetch its IR function and validate `Context::Lambda`.
2. Write the same signature used by its Stage 3 prototype.
3. If it has captures, cast the erased environment once in the function prologue:

   ```c
   struct RayLambdaEnv_<instance> *ray_env = ray_raw_env;
   ```

4. Reuse the existing CFG/body writer for variables, expression temporaries, blocks, instructions, phis, and terminators.

For a captureless helper, do not manufacture or dereference a typed environment. The raw parameter can remain unused.

## Address Emission

Change `Writer::write_address` to receive the current function instance and thread that argument through all callers, including:

- `Load`;
- `RefOf`;
- `Instruction::Store`;
- any future address-valued expression writer.

Handle every `AddressRoot` explicitly:

```text
Error             -> existing invariant panic
Variable(id)      -> current local variable identifier
Parameter(id)     -> root def parameter identifier
LambdaParameter   -> lambda helper parameter identifier
Capture(id)       -> typed_env->capture_field
Deref(expr)       -> (*expression_temp)
```

`AddressRoot::Capture` is the address of the environment field itself. Because that field stores a pointer, the existing IR sequence:

```text
Load Address(Capture(c))
Deref(pointer)
```

naturally becomes:

```c
ray_env->ray_capture_c
(*ray_expr_pointer)
```

Do not make capture loads or dereferences implicit in the C writer; preserve the IR semantics.

## Files

### `compiler/codegen/c/src/expression/function_instance.rs`

- Add the function map and current function ID to the private context.
- Preserve owner-substitution delegation for types and direct calls.

### `compiler/codegen/c/src/function.rs`

- Generate root definitions by selecting `FunctionMap::root_id()`.
- Add lambda helper definition generation using `MonoLambda`.
- Share declaration/signature writing with Stage 3 so prototypes and definitions cannot drift.

### `compiler/codegen/c/src/translation_unit.rs`

- Pre-generate or emit one definition for every sorted `MonoLambda`.
- Keep all helper declarations before all root/helper definitions.

### `compiler/codegen/c/src/ir.rs`

- Pass a complete function instance into body, instruction, and expression writers.
- Emit the typed environment cast at the beginning of capturing helper bodies.
- Thread the function instance into store-address generation.

### `compiler/codegen/c/src/expression/address.rs`

- Implement `LambdaParameter` and `Capture` roots.
- Keep all project-owned enum matches exhaustive.

### `compiler/codegen/c/src/expression/{load,ref_of}.rs`

- Pass the current function instance to `write_address`.

## Test Strategy

Do not add unit tests that assert exact private identifier strings or writer call order. The user-observable contract is that generated C compiles and lambda bodies read arguments/captures correctly; Stage 5 protects that with run fixtures once the enclosing closure can be constructed.

Run:

```sh
cargo check -p rayc_c
cargo test -p rayc_e2e --test run_e2e
cargo clippy --workspace --all-targets
cargo +nightly fmt --check
```

## Acceptance Criteria

- Each collected `MonoLambda` has exactly one C helper definition.
- Helper signatures match their prototypes and the closure function-pointer ABI.
- Lambda parameters lower to named C parameters.
- Capture roots lower to fields of the typed erased environment.
- Captured reads, writes, tuple projections, and forwarded reborrows follow the existing explicit IR operations.
- Direct calls made inside lambda helpers use the owning def substitution.
- Root def code generation remains unchanged apart from carrying richer function context.

## Non-Goals

- No C storage object for a closure environment.
- No `ExpressionKind::MakeLambda` writer.
- No closure escape/lifetime policy.


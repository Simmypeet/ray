# Explicit Lambda Captures in `rayc_ir`

## Goal

Lower every TypedAST lambda into a separate IR function and make closure creation, capture mutability, and captured-variable access explicit.

For a source lambda such as `() -> x = 20`, the important IR shape should be:

```text
# Enclosing function
e0 = RefOf.mut Address(Variable(x))
e1 = MakeLambda(lambda_function, [e0])

# lambda_function, whose capture c0 has type `&mut T`
e0 = Load Address(Capture(c0))       # load the captured pointer
e1 = Literal(20)
Store Address(Deref(e0)), e1         # mutate x through that pointer
Return e1
```

All captures are by reference. A capture starts immutable and is upgraded to mutable only when some use requires mutable access.

## Design decisions

1. The result of lowering one source `def` is an IR `FunctionMap`, not a single CFG. It owns the root IR function and every lambda function nested under it.
2. `ExpressionKind::MakeLambda` constructs a closure value from an IR `FunctionID` and an ordered list of reference-valued capture operands.
3. A lambda function owns an ordered capture layout. Operand `n` of `MakeLambda` initializes capture slot `n` of the target function.
4. `AddressRoot::Capture` addresses the closure slot itself. Since the slot contains a pointer, accessing the original source binding requires `Load(Capture(...))` followed by `Deref(...)`.
5. Capture discovery is a separate, bottom-up analysis performed before IR lowering. Lowering must not discover or change a capture layout on demand.
6. A capture is keyed during analysis by the TypedAST `name_binding::Source`, not by spelling or `NameBindingID`. This deduplicates all uses of the same variable/parameter while preserving shadowing.
7. Capture order is first lexical/evaluation-order occurrence. Upgrading a capture from immutable to mutable does not move its slot. Do not derive layout order by iterating an `FxHashMap`.
8. This pass records reference mutability but does not prove reference lifetimes, uniqueness, or whether a closure may escape. Those are semantic/borrow-checking concerns, not closure conversion.

## Proposed `rayc_ir` representation

### IR function container

Extend `compiler/semantic/ir/src/function.rs` with an IR-local function identity and container:

```rust
pub type FunctionID = ID<Function>;

pub struct FunctionMap {
    functions: Arena<Function>,
    root: FunctionID,
}
```

Expose narrow methods such as `root`, `get_function`, and `functions`, plus a construction API used by `rayc_ir_builder`; keep fields private. The query in `rayc_ir/src/lib.rs` should return `Interned<FunctionMap>`.

Each `Function` should gain a context:

```rust
pub enum Context {
    Def,
    Lambda(LambdaContext),
}

pub struct LambdaContext {
    parameters: LambdaParameterMap,
    captures: CaptureMap,
}
```

Add IR-owned `LambdaParameter`, `LambdaParameterID`, and `LambdaParameterMap` rather than leaking TypedAST IDs into IR. Copy parameter type/span information while lowering. Retain the existing semantic `ParameterID` for root-def parameters for now; this keeps the change focused.

### Capture layout

Add `compiler/semantic/ir/src/lambda.rs` containing approximately:

```rust
pub struct Capture {
    pointee_ty: Interned<Ty>,
    mutability: Mutability,
    span: RelativeSpan,
}

pub type CaptureID = ID<Capture>;

pub struct CaptureMap {
    captures: OrderedArena<Capture>,
}
```

`Capture::pointer_ty(engine)` is conceptually `Ty::new_pointer(pointee_ty, mutability, engine)`. Storing pointee type and mutability separately makes the closure layout and its least-privilege mutability easy to inspect. `CaptureMap` iteration order defines the `MakeLambda` operand ABI.

Add the following roots to `address.rs`:

```rust
pub enum AddressRoot {
    Error,
    Variable(VariableID),
    Parameter(ParameterID),
    LambdaParameter(LambdaParameterID),
    Capture(CaptureID),
    Deref(ExpressionID),
}
```

Add private-root construction helpers `Address::new_lambda_parameter` and `Address::new_capture`, following the existing address API.

### Closure construction expression

Add `compiler/semantic/ir/src/expression/make_lambda.rs`:

```rust
pub struct MakeLambda {
    function_id: FunctionID,
    captures: Vec<ExpressionID>,
}
```

and add `ExpressionKind::MakeLambda(MakeLambda)`.

Required invariants:

- `function_id` identifies a `Context::Lambda` in the same `FunctionMap`.
- `captures.len()` equals the target function's capture count.
- Capture operand `i` has pointer type `&T` or `&mut T` matching target capture `i`.
- Capture operands are already-defined expressions and are evaluated left-to-right before `MakeLambda` is emitted.
- The `MakeLambda` expression type remains the source lambda signature (`def(...) -> ...`); the environment layout is carried by the node and target IR function.

## Capture analysis in `rayc_ir_builder`

Add `compiler/semantic/ir_builder/src/capture_analysis.rs`. It consumes the TypedAST `FunctionMap` and returns a transient analysis result owned by the lowering coordinator. It is derived from TypedAST before lowering, but it is not stored in `rayc_typed_ast`, encoded, or interned.

Use the following concrete model (names may be adjusted during implementation, but keep these ownership and ordering properties):

```rust
use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::{Mutability, Ty};
use rayc_typed_ast::{
    function::FunctionID as TypedFunctionID,
    name_binding::Source,
};

/// Complete, temporary closure-conversion analysis for one source def.
pub(super) struct CaptureAnalysis {
    /// Map iteration order has no meaning; each plan owns its stable layout.
    plans: FxHashMap<TypedFunctionID, FunctionCapturePlan>,
    /// Child-before-parent order used to assign/lower IR function IDs.
    lowering_order: Vec<TypedFunctionID>,
}

/// Index into `FunctionCapturePlan::captures`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct CaptureSlot(usize);

/// The capture layout and lexical children of one TypedAST function.
pub(super) struct FunctionCapturePlan {
    /// Stable closure-field order, determined by first encounter.
    captures: Vec<CaptureRequirement>,
    /// Deduplication/lookup only; never iterate this map to create a layout.
    capture_slots: FxHashMap<Source, CaptureSlot>,
    /// Stable lexical/evaluation order of lambda expressions in this function.
    children: Vec<TypedFunctionID>,
}

/// One original binding that this function must receive by reference.
#[derive(Debug, Clone)]
pub(super) struct CaptureRequirement {
    /// Canonical defining variable, def parameter, or lambda parameter.
    source: Source,
    /// Type of the original binding, before wrapping it in a pointer.
    pointee_ty: Interned<Ty>,
    /// `Immutable` unless some direct or propagated use requires `Mutable`.
    mutability: Mutability,
    /// Defining-binding or first-use span for debugging/validation.
    span: RelativeSpan,
}

/// How the enclosing expression uses an lvalue-shaped child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UseMode {
    /// The binding's value is only loaded.
    Value,
    /// The binding's address is required with this mutability.
    Address(Mutability),
}
```

Keep these fields private. The analysis module should expose narrow operations such as:

```rust
impl CaptureAnalysis {
    pub(super) fn plan(&self, function_id: TypedFunctionID) -> &FunctionCapturePlan {
        self.plans.get(&function_id).expect("capture plan should exist")
    }

    pub(super) fn lowering_order(
        &self,
    ) -> impl ExactSizeIterator<Item = TypedFunctionID> + '_ {
        self.lowering_order.iter().copied()
    }
}

impl FunctionCapturePlan {
    fn new() -> Self {
        Self {
            captures: Vec::new(),
            capture_slots: FxHashMap::default(),
            children: Vec::new(),
        }
    }

    pub(super) fn captures(
        &self,
    ) -> impl ExactSizeIterator<Item = (CaptureSlot, &CaptureRequirement)> {
        self.captures
            .iter()
            .enumerate()
            .map(|(index, capture)| (CaptureSlot(index), capture))
    }

    pub(super) fn capture_slot(&self, source: Source) -> Option<CaptureSlot> {
        self.capture_slots.get(&source).copied()
    }

    pub(super) fn children(
        &self,
    ) -> impl ExactSizeIterator<Item = TypedFunctionID> + '_ {
        self.children.iter().copied()
    }

    fn push_child(&mut self, child: TypedFunctionID) {
        self.children.push(child);
    }

    /// Insert a source at the end on first use, or upgrade its existing
    /// mutability without changing its slot.
    fn require(&mut self, requirement: CaptureRequirement) -> CaptureSlot {
        if let Some(slot) = self.capture_slot(requirement.source) {
            let existing = &mut self.captures[slot.0];
            existing.mutability = join_mutability(existing.mutability, requirement.mutability);
            return slot;
        }

        let source = requirement.source;
        let slot = CaptureSlot(self.captures.len());
        self.captures.push(requirement);
        self.capture_slots.insert(source, slot);
        slot
    }
}

impl CaptureRequirement {
    pub(super) const fn source(&self) -> Source { self.source }

    pub(super) const fn pointee_ty(&self) -> &Interned<Ty> { &self.pointee_ty }

    pub(super) const fn mutability(&self) -> Mutability { self.mutability }

    pub(super) const fn span(&self) -> RelativeSpan { self.span }
}

const fn join_mutability(current: Mutability, requested: Mutability) -> Mutability {
    match (current, requested) {
        (Mutability::Immutable, Mutability::Immutable) => Mutability::Immutable,
        (Mutability::Immutable, Mutability::Mutable)
        | (Mutability::Mutable, Mutability::Immutable)
        | (Mutability::Mutable, Mutability::Mutable) => Mutability::Mutable,
    }
}
```

Add a delegating `Source::function_id()` method in `rayc_typed_ast` so analysis and lowering can ask who owns a source without repeatedly matching through the enum.

`FunctionCapturePlan::require` uses the insertion-ordered `Vec` plus the `FxHashMap<Source, CaptureSlot>`. On a repeated source, it joins mutability in place:

```text
Immutable + Immutable = Immutable
Immutable + Mutable   = Mutable
Mutable   + anything  = Mutable
```

This gives the most restrictive capture that satisfies all uses: immutable unless mutable access is actually required.

The vector index is the analysis-time capture slot. It is deliberately distinct from `rayc_ir::lambda::CaptureID`, which does not exist until the IR lambda context is constructed. During lowering, iterate `plan.captures()` in order, insert each IR `Capture`, and build the builder's `Source -> CaptureID` lookup from the returned IR IDs. Never assume the arena ID numerically equals `CaptureSlot`.

### Expression-use modes

Walk each function's reachable statements/expressions with an explicit use mode:

```text
Value
Address(Immutable)
Address(Mutable)
```

Apply these rules exhaustively to `TypedExprKind`:

- An identifier whose `Source::function_id()` is the current function is local and creates no capture.
- A non-local identifier in `Value` or `Address(Immutable)` requires an immutable capture.
- A non-local identifier in `Address(Mutable)` requires a mutable capture.
- Assignment analyzes its left side as `Address(Mutable)` and its right side as `Value`.
- `RefOf` analyzes its pointee as `Address(reference.mutability())`.
- `TupleIndex` and `Paren` preserve an incoming address mode for their operand.
- `Deref` always analyzes its pointer expression as `Value`, even when the dereference is assigned through. For example, `*p = 20` only reads the binding `p`; it does not mutate the binding `p`, so an outer `p` is captured immutably. The pointer's own type governs whether its pointee may be mutated.
- Tuple elements, call callee then arguments, arithmetic/logical operands, and `if` condition/branches are analyzed as `Value` in their language evaluation order.
- Error nodes should expose and visit their retained children for complete recovery behavior, without manufacturing captures for an empty error node.
- Match every project-owned enum variant explicitly; do not use wildcard arms.

Add small delegation APIs to TypedAST `FunctionMap` as needed, for example `statements_in`, `get_variable_in`, and `get_type_of_expr_id_in`. The IR builder should use these scoped APIs with its current TypedAST `FunctionID`; it should not assume expression or variable IDs are global.

### Transitive propagation for nested lambdas

Analyze lambdas bottom-up. When TypedAST function `F` contains a lambda expression for child `C`:

1. Analyze `C` first.
2. Visit `C`'s ordered capture requirements.
3. If a captured source is defined by `F`, `F` can take its address directly when constructing `C`; do not add it to `F`'s own captures.
4. If the source is defined outside `F`, merge the same requirement into `F`'s capture plan. This is the pass-through capture required to construct `C`.
5. Preserve `C`'s independent mutability. `F` may need a mutable slot because another child writes while a read-only child still expects an immutable operand.

Conceptually, the recursive analyzer is:

```rust
fn analyze_function(&mut self, function_id: TypedFunctionID) {
    let mut plan = FunctionCapturePlan::new();

    // Walk statements and expressions in source evaluation order.
    // Direct non-local identifiers call `plan.require(...)`.
    self.walk_function(function_id, &mut plan, |analysis, plan, child_id| {
        analysis.analyze_function(child_id);
        plan.push_child(child_id);

        for (_, child_capture) in analysis.plan(child_id).captures() {
            if child_capture.source.function_id() != function_id {
                // The current function does not own the source, so it needs a
                // pass-through capture in order to construct the child.
                plan.require(child_capture.clone());
            }
        }
    });

    self.plans.insert(function_id, plan);
    self.lowering_order.push(function_id); // postorder: children first
}
```

The actual implementation can avoid the closure/borrowing shape above, but it should preserve the algorithm: analyze at the lambda expression's encounter point, merge the child's requirements there, and append the current function to `lowering_order` only after all children.

For:

```text
let mut x = 0
let outer = () ->
    let inner = () -> x = 32
    ...
```

assuming `F0` is the root, `F1` is `outer`, and `F2` is `inner`, the concrete analysis result is conceptually:

```rust
CaptureAnalysis {
    lowering_order: vec![F2, F1, F0],
    plans: {
        F0 => FunctionCapturePlan {
            captures: vec![],
            capture_slots: {},
            children: vec![F1],
        },
        F1 => FunctionCapturePlan {
            captures: vec![CaptureRequirement {
                source: Source::Variable(FunctionLocalID::new(F0, x)),
                pointee_ty: int32,
                mutability: Mutability::Mutable,
                span: x_span,
            }],
            capture_slots: {
                Source::Variable(FunctionLocalID::new(F0, x)) => CaptureSlot(0),
            },
            children: vec![F2],
        },
        F2 => FunctionCapturePlan {
            captures: vec![CaptureRequirement {
                source: Source::Variable(FunctionLocalID::new(F0, x)),
                pointee_ty: int32,
                mutability: Mutability::Mutable,
                span: x_span,
            }],
            capture_slots: {
                Source::Variable(FunctionLocalID::new(F0, x)) => CaptureSlot(0),
            },
            children: vec![],
        },
    },
}
```

The map syntax and symbolic `int32`, `x`, and spans above are illustrative Debug-style output, not intended to compile as a literal. Its meaning is:

```text
inner captures: [x: mutable]
outer captures: [x: mutable]   # propagated even though outer never reads x directly
root captures:  []             # root owns x and can take its address directly
```

Analysis therefore propagates requirements **from child to parent**. Lowering later uses those plans in the opposite direction, passing references **from parent to child**:

```text
F0 constructs F1:
  source x is local to F0
  e0 = RefOf.mutable Address(Variable(x))
  e1 = MakeLambda(F1, [e0])

F1 constructs F2:
  source x is capture slot 0 in F1
  e0 = Load Address(Capture(0))
  e1 = RefOf.mutable Address(Deref(e0))
  e2 = MakeLambda(F2, [e1])

F2 writes x:
  e0 = Load Address(Capture(0))
  e1 = Literal(32)
  Store Address(Deref(e0)), e1
```

If a child captures something owned by its immediate parent, propagation stops at that parent. For example:

```text
outer = () ->
    let mut y = 0
    let inner = () -> y = 1
```

produces:

```text
outer captures: []
inner captures: [Variable(outer, y): mutable]
```

`outer` can construct `inner` with `RefOf.mutable Address(Variable(y))`, so `y` must not be added to `outer`'s own environment.

If separate children require different mutabilities, keep their plans independent while joining the parent's requirement. For example, a read-only child and a writing child each capture ancestor `x` as immutable and mutable respectively; their parent has one mutable `x` slot. During lowering, the parent creates an immutable reborrow for the reader and a mutable reborrow for the writer.

Validate that every TypedAST lambda reachable from the root has exactly one lexical parent and that every non-root free source is resolvable through the parent chain. Treat failures as compiler-internal invariant violations.

## Lowering architecture

### Finish the current `FunctionMap` refactor first

The current `rayc_ir_builder` fails because it now accepts TypedAST `FunctionMap` but still calls single-function APIs. Before lambda work:

- Make every expression/address lookup use `get_expression_in(builder.typed_function_id(), id)` or a narrow builder delegation.
- Make variable registration use `get_variable_in(current_function_id, id)`.
- Make statement iteration use `statements_in(current_function_id)`.
- Make lvalue classification and expression-type lookup function-scoped.
- Keep the existing borrowed `TypedExprKind` dispatch so `TypedExprWithID<&Node>` implementations continue to match.

This should restore `cargo check -p rayc_ir_builder` without pretending lambdas are error expressions.

### Lower all functions in a coordinated pass

Replace the single `Builder::lower -> IrFunction` entry point with a coordinator that owns:

- the TypedAST `FunctionMap`;
- all `FunctionCapturePlan`s;
- the TypedAST-function-ID to IR-function-ID map;
- the output IR `FunctionMap`.

Lower functions in child-before-parent order so a parent's `MakeLambda` can refer to an already assigned IR `FunctionID`. Record the root mapping as `IrFunctionMap::root`. Use a narrow `FunctionMap` construction API rather than exposing its arena.

Keep the existing CFG `Builder` as a per-function builder. Give it:

- its current TypedAST `FunctionID`;
- the target function's capture plan;
- `Source -> CaptureID` lookup;
- TypedAST-variable to IR-variable mapping;
- TypedAST-lambda-parameter to IR-lambda-parameter mapping;
- read-only access to the TypedAST-function-ID to IR-function-ID map.

When starting a lambda-function builder, copy its parameters in their `OrderedArena` order and install its analyzed captures in plan order before lowering the body.

### Lower local and captured addresses

Centralize `Source` resolution instead of duplicating it in identifier cases:

```text
address_of_source(source):
  source owned by current function:
    Variable        -> Address(Variable(mapped_id))
    def Parameter   -> Address(Parameter(id))
    LambdaParameter -> Address(LambdaParameter(mapped_id))

  source owned by an outer function:
    capture_id = current capture lookup[source]
    pointer = emit Load(Address(Capture(capture_id)))
    return Address(Deref(pointer))
```

Identifier-as-value remains `Load(lower_address(identifier))`. Assignment already stores into `lower_address(left)`. Therefore captured reads and writes become explicit without special cases in binary lowering.

### Lower `TypedExprKind::Lambda`

Add `expression/lambda.rs`. For each required capture of child `C`, in `C`'s layout order:

1. Resolve the source to its pointee address in the current function with `address_of_source`.
2. Emit `RefOf` with exactly the mutability required by `C` and type it with `Ty::new_pointer`.
3. Append that expression ID to the capture operand list.
4. Emit `ExpressionKind::MakeLambda` using the mapped IR function ID and operands.

Always reborrow from the pointee address when forwarding a capture:

```text
parent_pointer = Load Address(Capture(parent_slot))
child_pointer  = RefOf.<child mutability> Address(Deref(parent_pointer))
closure        = MakeLambda(child_function, [child_pointer])
```

This handles an outer mutable capture being forwarded as an immutable child capture without relying on an implicit `&mut T` to `&T` coercion in IR. It also makes every closure construction operand type exactly match the child's layout.

## Worked examples

### Read-only capture

```text
source: () -> x + 1

enclosing:
  e0 = RefOf.immutable Address(Variable(x))
  e1 = MakeLambda(f, [e0])

f:
  e0 = Load Address(Capture(c0))
  e1 = Load Address(Deref(e0))
  ...
```

`c0` is immutable even if the original declaration of `x` is mutable, because this lambda only reads it.

### Direct mutable capture

```text
source: () -> x = 20

enclosing:
  e0 = RefOf.mutable Address(Variable(x))
  e1 = MakeLambda(f, [e0])

f:
  e0 = Load Address(Capture(c0))
  e1 = Literal(20)
  Store Address(Deref(e0)), e1
```

### Transitive mutable capture

```text
source: () -> let inner = () -> x = 32; ...

root:
  e0 = RefOf.mutable Address(Variable(x))
  e1 = MakeLambda(outer, [e0])

outer:
  e0 = Load Address(Capture(outer_x))
  e1 = RefOf.mutable Address(Deref(e0))
  e2 = MakeLambda(inner, [e1])

inner:
  e0 = Load Address(Capture(inner_x))
  e1 = Literal(32)
  Store Address(Deref(e0)), e1
```

## Implementation sequence

1. **Repair scoped TypedAST access in `ir_builder`.** Complete the in-progress `FunctionMap` refactor and regain a compiling builder baseline.
2. **Add IR lambda data types.** Add `FunctionMap`, IR `FunctionID`, function context, lambda parameters, captures, `AddressRoot::{LambdaParameter,Capture}`, and `ExpressionKind::MakeLambda`, including encode/decode/stable-hash derives and exhaustive matches.
3. **Add capture analysis.** Implement the use-mode walker, stable deduplication/mutability join, child graph discovery, and bottom-up propagation.
4. **Introduce multi-function lowering.** Add the coordinator, ID mapping, child-before-parent construction, per-function capture/parameter setup, and change the IR query result to `FunctionMap`.
5. **Lower explicit source addresses.** Resolve local variables, def parameters, lambda parameters, and captures through one path; remove the current non-local/error fallbacks.
6. **Lower `MakeLambda`.** Materialize left-to-right `RefOf` capture operands (including explicit reborrows) and then emit the closure expression.
7. **Add validation and tests.** Protect capture analysis and externally observable lambda behavior as described below.
8. **Update downstream consumers.** Adapt monomorphization and C codegen to the new query container and exhaustive IR variants. Full closure layout/call ABI codegen can be a follow-up milestone, but the compile fallout should be made explicit rather than hidden behind wildcard matches.

## Validation and tests

The capture analyzer owns a non-trivial, stable contract, so focused `rayc_ir_builder` unit coverage is justified for:

- no capture for locals and current lambda parameters;
- read-only use produces one immutable slot;
- assignment and `&mut` produce a mutable slot;
- mixed reads/writes deduplicate to one mutable slot without changing order;
- tuple-field assignment propagates mutable address use to its root;
- `*p = value` captures the binding `p` immutably;
- a nested child propagates an ancestor source through an otherwise non-using parent;
- a child capture of a parent-local binding does not become a parent capture;
- two children can require different reborrow mutabilities from one parent slot.

Prefer end-to-end `compiler/e2e/test/run/` fixtures once closure codegen exists, covering:

- returning/calling a captureless lambda;
- reading a captured variable;
- mutating a captured variable and observing the changed value outside the lambda;
- nested transitive mutation;
- capturing a def parameter and a lambda parameter;
- tuple projection reads/writes through captures.

No new diagnostic fixture is needed merely for IR structure. Add a `check` fixture only if this work changes source-visible acceptance or diagnostics.

Run the narrow checks during implementation, then the workspace gates:

```sh
cargo check -p rayc_ir -p rayc_ir_builder
cargo test -p rayc_ir_builder
cargo test -p rayc_e2e --test check_e2e
cargo test -p rayc_e2e --test run_e2e
cargo clippy --workspace --all-targets
cargo +nightly fmt --check
```

## Downstream implications

Changing `rayc_ir::Key` from `Interned<Function>` to `Interned<FunctionMap>` affects current consumers:

- `rayc_mono` must visit variables, expression types, direct calls, and `MakeLambda` nodes in every reachable IR function belonging to a def. A local lambda function is not a new `GlobalSymbolID`; it is instantiated with its owning def's substitution.
- C codegen must select the root for a normal def definition and eventually emit helper functions/environment structs for nested IR functions.
- `AddressRoot::LambdaParameter`, `AddressRoot::Capture`, and `ExpressionKind::MakeLambda` require explicit codegen match arms.
- Indirect-call codegen must pass the closure environment alongside ordinary lambda arguments. The exact C closure ABI is a separate design decision; `rayc_ir` should not encode C-specific layout details.

## Acceptance criteria

- Every TypedAST function reachable from the root has a corresponding IR function.
- No valid non-local identifier lowers to `AddressRoot::Error`.
- Every lambda construction explicitly lists all and only the references required by its target function, in target layout order.
- Read-only captures are immutable; any direct or transitive mutable requirement upgrades exactly that source to mutable.
- Captured reads are `Load(Deref(Load(Capture)))`; captured writes store to `Deref(Load(Capture))` (with projections applied after dereference).
- Nested lambdas receive ancestor bindings through explicit pass-through captures.
- All project-owned enum matches remain exhaustive, and the IR/builder pass formatting, Clippy, and relevant tests.

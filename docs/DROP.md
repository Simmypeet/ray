This document describes how Ray releases resources: the `Drop` trait, how its
dictionaries are chosen, and exactly when the compiler inserts calls to
`Drop.drop`. The first half describes the language rules; the second half
describes where each rule is implemented in the compiler.

# The `Drop` Trait

`Drop` is declared in the core library:

```
trait Drop[t]:
    def drop(self: t)
```

The compiler calls `drop` automatically when a value would otherwise be lost,
for example when its variable goes out of scope. Unlike Rust, dropping is
transparent: a drop is an ordinary call through an ordinary dictionary, and the
compiler must be able to name that dictionary at the point of the drop.

For a polymorphic value, the dictionary has to come from the caller. The
following function is rejected, because `value` is dropped when the function
returns and nothing says how to drop an `a`:

```
def passPolyVar(value: a):
    # error: no implicit instance found: `core.Drop[a]`
```

The fix is to take the dictionary as a `given`:

```
def passPolyVar(value: a) given (dropDict: Drop[a]):
    # `dropDict.drop(value)` is inserted when the function returns.
```

In this sense Ray values behave a little like linear values: every value is
consumed exactly once, either explicitly (by moving it somewhere) or implicitly
(by the compiler inserting a `Drop.drop` call).

A value that is only ever moved needs no dictionary. `def identity(value: a) ->
a: return value` compiles as is, because `value` is moved out by the `return`.

## Callback Parameters

A parameter written with the callable shorthand, such as
`fn: def(int32) -> int32`, introduces a hidden type variable for the callable
together with two hidden dictionaries: `Def[callable]` to call it and
`Drop[callable]` to drop it on paths that do not call it. Callers fill both in
automatically. When the explicit form is used instead
(`fn: f given (d: Def[f])`), the `Drop[f]` given must be written by hand if `fn`
may be left unconsumed.

## `Copy` Values Are Still Dropped

`Copy` does not imply a trivial drop. A `Copy` value that is still alive at the
end of its scope is dropped like any other value, so a function which keeps a
value of type `a` where `a: Copy` still needs `given Drop[a]`.

Because `Copy` is derived structurally, a struct whose fields are all `Copy`
(such as a wrapper around a `cstr`) is `Copy` even when it has a user-written
`Drop` instance. Reading such a value copies it, and each copy is dropped.
Use `move` to transfer the value instead of copying it.

# Moving Out Explicitly: `move <expr>`

`move <expr>` moves out of a place even when its type is `Copy`:

```
let moved = 2
consume(move moved)
consume(moved)      # error: use of moved value
```

`move` applies to the postfix expression that follows it, so `move x.y` moves
the field `x.y`. Applying `move` to a value that is not a place (a call result,
a literal) has no effect, since such a value is a fresh temporary already.

Inside a closure, `move` on a captured binding also moves the binding out of
the enclosing function when the closure is created:

```
let first = make("first")
() -> move first        # `first` is no longer owned by the enclosing function
```

## How Closures Capture

Like Rust, a closure captures each outer binding in the weakest way its uses
allow:

- reading a `Copy` value only needs a shared reference;
- assigning to the binding, or taking `.&mut` of it, needs a mutable reference;
- moving the value, by reading a non-`Copy` value or writing `move`, captures
  it by value.

When uses disagree, the strongest one wins, and capturing by value is the
strongest. A closure that owns its copy of a binding can also read and assign
it.

Ray does not have a borrow checker yet, so nothing stops a closure that
captures by reference from outliving the binding it points to. Write `move` on
the captures of a closure that escapes, or that should keep a snapshot of a
value.

## Implementing `Drop` by Hand

The parameter of a `Drop.drop` implementation is an ordinary owned value, so
if the body does not consume it, it is dropped again when the method returns,
which would recurse into the same method. Consume it explicitly by moving it
into `core.NoDrop`, whose drop does nothing:

```
inst LoggedDrop for core.Drop[Logged]:
    def drop(self: Logged):
        puts(self.message)

        # Consume `self` without dropping it again.
        core.NoDrop { value = move self }
```

# Built-in and Generated Instances

Writing a `Drop` instance for every type would be tedious and error-prone, so
the compiler provides the following instances itself:

- **Primitives and pointers** have a no-op `Drop`.
- **`core.NoDrop[t]`** has a no-op `Drop`, whatever `t` is. It is the way to
  deliberately discard a value without running its drop.
- **Tuples** drop their elements in reverse element order.
- **Closures** drop their by-value captures in reverse capture order.
  By-reference captures are pointers and need no drop.
- **Structs** get a generated `Drop` unless a user-written instance exists (see
  below).

These built-in instances are chosen before any lexical `given` is consulted,
and users cannot declare `Drop` instances for primitives, pointers, or tuples.
A lexical `given Drop[...]` is therefore only ever used for opaque types:
polymorphic type variables and associated types that cannot be reduced.
A `given Drop[int32]` cannot replace the built-in behavior.

Tuple and closure instances record the dictionary chosen for each element, so
polymorphic element dictionaries survive monomorphization.

## Generated `Drop` for Structs

A user-written instance replaces the generated behavior entirely, including
which fields are dropped and in what order. The generated instance is only a
fallback; it never competes with a user-written instance for the same type.

A user-written `Drop` instance must have a struct as its head, such as
`Drop[LinkedList[t]]`. Heads that are type variables (`Drop[a]`), associated
types, primitives, pointers, tuples, or any other non-struct type are rejected.
It must also be declared in the target that defines the struct. The compiler
computes plans for all structs in a target together; a field whose type belongs
to another target uses that target's finished plan.

### Premises and Recursion

The generated instance requires dictionaries only for the struct's *external*
drop requirements. It does not list a `Drop` requirement for every field,
because a field can contain the enclosing type (directly or through another
type constructor), and resolving those requirements while building the
instance would recurse forever.

For example, assuming `Option` provides `Drop[Option[a]]` given `Drop[a]`:

```
pub struct LinkedList[t]:
    val: t
    next: Option[LinkedList[t]]

# Conceptual generated instance; the body is compiler-generated.
inst DropLinkedList[t] for Drop[LinkedList[t]] given (tDrop: Drop[t]):
    def drop(self: LinkedList[t]):
        # Drop `next` using Drop[Option[LinkedList[t]]]. Resolving that
        # applies DropLinkedList[t] (with tDrop) for Drop[LinkedList[t]].
        # Then drop `val` using tDrop.
```

The recipe for `next` refers to `DropLinkedList[t]` and supplies its external
`Drop[t]` argument; it does not inline that instance's own recipe. So deriving
`DropLinkedList[t]` needs `Drop[t]` but never expands another
`DropLinkedList[t]` plan. Recursive calls happen only at run time, when dropping
recursive values. The same applies to mutually recursive types.

Premises are the minimal external requirements of the fields, not one `Drop`
bound per type parameter. A type parameter that does not appear in an owned
field, or only appears behind a pointer, needs no bound. A field whose type is
a type variable needs that variable's `Drop`, and so does a field whose type is
an associated type that cannot be reduced.

For each field, the compiler resolves its `Drop` requirement using the
generated premises and the plans of other structs. A user-written instance for
a field's type may have requirements beyond these `Drop` premises. If those
cannot be established, generation fails with a diagnostic naming the field and
the missing requirement, and the programmer can write the instance by hand.
This keeps generation sound without inventing arbitrary premises.

### Generated Behavior

A struct drops its owned fields in reverse declaration order, each with the
dictionary selected for its instantiated type, whether that is a premise, a
built-in instance, or a user-written instance. A no-op dictionary emits no call.
Each field is dropped exactly once; the generated method does not also drop the
struct itself when it returns.

Enums are not part of the language yet. When they are added, a generated enum
`Drop` should drop only the fields of the active variant, in reverse
declaration order.

Recursive `Drop` resolution does not make a recursively embedded value type
well-formed: `LinkedList[t]` still needs indirection in its recursive field to
have a finite size.

# When Drop Is Invoked

The compiler inserts `Drop.drop` calls in four situations.

## When a Value Is Discarded in a Statement

An expression statement whose value is unused drops that value:

```
callFuncThatReturnsValue()
```

behaves like:

```
let _value = callFuncThatReturnsValue()
Drop.drop(_value)
```

The dictionary is chosen during type checking, so the value's type needs a
`Drop` dictionary at that point.

Two related rules keep this from dropping values twice:

- An assignment evaluates to unit, so `x = make()` as a statement stores the
  new value and drops nothing but the unit result.
- A statement that just names a place, such as `value` or `value.field`, does
  not read it. Nothing is moved and nothing is dropped.

## When Control Flow Merges

Where control flow merges, every incoming path must agree on which values are
alive. A value that some paths still hold but others have moved is dropped on
the paths that still hold it. For example:

```
let value = callFuncThatReturnsValue()
if condition:
    consume(value)
```

behaves like:

```
let value = callFuncThatReturnsValue()
if condition:
    consume(value)
else:
    Drop.drop(value)
```

Loops are merges too. Here `value1` is moved before the loop, but each
iteration assigns it again:

```
let mut value1 = callFuncThatReturnsValue()
consume(value1)

while condition:
    # ...
    value1 = callFuncThatReturnsValue()
```

On entry to the loop `value1` has been moved, while at the end of an iteration
it holds a value. The value is therefore dropped before the next iteration, so
the loop starts every iteration in the same state:

```
let mut value1 = callFuncThatReturnsValue()
consume(value1)

while condition:
    # ...
    value1 = callFuncThatReturnsValue()

    # drop the restored value before the next iteration
    Drop.drop(value1)
```

Leaving a loop through `break` is handled the same way: a value moved on the
`break` path but still held on the path where the loop condition fails is
dropped on the latter path before the two meet.

## When Variables Go Out of Scope

When a scope ends, every variable declared in it that still holds a value is
dropped. When a function returns, this includes its parameters and, for a
closure or the body of a `run`, its by-value captures. Operation handlers are
the exception, described below.

- Variables are dropped in reverse declaration order, followed by parameters in
  reverse order, followed by captures in reverse order.
- A moved variable is not dropped.
- A struct or tuple that has been partially moved drops each of its remaining
  initialized fields individually, in reverse order.

## After a `run … with` Returns

The operation handlers of one `run … with` share a single environment of
captures, and each handler may be called any number of times, including never.
The handlers therefore only borrow their captures, much like a Rust `FnMut`
closure:

- a handler does not drop its captures when it returns;
- a handler may read `Copy` values from its captures, borrow them, and assign
  to them, but it may never move out of them, not even briefly and not even a
  single field: `cannot move out of a value captured by an operation handler`.

```
let mut count = 0
let logger = make("logger")
run:
    Log.write()
with Log:
    def write():
        count = count + 1     # OK: assignment borrows `count`
        let taken = logger    # error: moves out of the capture
```

Once the handled body returns, the function running the `run … with` drops
the shared captures, in reverse order. Because a handler never moves, capture
inference borrows every capture a valid handler uses, so today these drops are
the no-op drops of pointers. The machinery is in place for by-value handler
captures, which need a Rust-style `move` on the closure itself (see
Limitations).

## When Reassigning a Live Variable

Assigning to a variable that still holds a value drops the previous value:

```
let mut value = callFuncThatReturnsValue()
value = callFuncThatReturnsValue()
```

behaves like:

```
let mut value = callFuncThatReturnsValue()
let _new = callFuncThatReturnsValue()
Drop.drop(value)
value = _new
```

The new value is computed before the old one is dropped, so the new value may
still use the old one. In `value = identity(value)`, the old value moves into
the call, so nothing is left to drop.

If the variable has already been moved, nothing is dropped:

```
let mut value = callFuncThatReturnsValue()
consume(value)
value = callFuncThatReturnsValue()    # no drop
```

Assigning to a field, such as `pair.first = make()`, drops only that field's
previous value.

# Implementation

This section maps the rules above onto the compiler. See
[COMPILATION_PIPELINE.md](COMPILATION_PIPELINE.md) for an overview of the
passes involved.

## Syntax and Typed AST

- `move <expr>` is `Leaf::Move` in `rayc_syntax` and `TypedExprKind::Move` in
  `rayc_typed_ast`. It is never an lvalue.
- Assignment (`BinaryOp::Assign`) has the unit type.
- An expression statement (`ExpressionStatement`) carries the `Drop` dictionary
  for its value. The typed-AST builder infers it together with the other
  constraints, because the value's type may not be known yet.
- The callable shorthand's hidden binders come from
  `PolyVarOrigin::CallableType`, `CallableDictionary` and
  `CallableDropDictionary`, created in `semantic_element_impl/src/poly_var_map.rs`.
- The capture plan (`typed_ast/src/capture_plan.rs`) records how each capture
  is received as a `CaptureMode`: `Value(LoadKind)` or `Reference(Mutability)`.
  `CaptureMode::join` lets a value capture win over a reference. A `move` of a
  captured binding makes its capture `Value(LoadKind::Move)`. `CapturePlan::analyze`
  is async and asks a `CopyOracle` whether each read value is `Copy`; a `Copy`
  read only needs a reference. The typed-AST builder passes its
  `ConstraintSolver`, which answers under the substitution solved so far: an
  undetermined numeric type counts as `Copy`, and any other undetermined type
  does not.

## IR

There is no dedicated drop instruction. Every inserted drop is two ordinary
instructions:

1. a `Load` of the place with `LoadKind::Move`, which consumes the value even
   when it is `Copy`;
2. a `Call` with `CallTarget::UnresolvedInstanceAssociated`, calling the core
   `Drop.drop` method (`CoreItem::DropMethod`) through the selected dictionary.

`LoadKind` (in `rayc_type::capture`) is `Implicit` for ordinary reads, which
copy `Copy` values and move everything else, and `Move` for `move <expr>` and
for inserted drops.

Discarded expression statements are the exception: they are lowered to an
`ExprDiscard` instruction that carries the dictionary chosen during type
checking.

A `Handle` expression (a `run … with`) carries one `Drop` dictionary per
handler capture (`Handle::handler_capture_drops`), in the same order as its
handler captures. `Handle::new` requires the complete list. A dictionary
depends only on the capture's type, not on the flow state, so the IR builder
resolves them as it lowers each `run … with`
(`Builder::handler_capture_drops`). Lowering is async for this reason, and
the `Builder` owns a `Solver` for the definition's environment. Each
dictionary is resolved for the environment field type of its capture
(`CaptureRequirement::storage_ty`), so a borrowed capture gets a pointer's
no-op dictionary. A missing dictionary is reported at the captured binding,
and the capture gets an error dictionary. These failures are reported only
when type checking succeeded, and they do not prevent memory analysis.

## Memory Analysis (`rayc_memory`)

`rayc_memory::analyze` runs as part of `BuildIR` in `rayc_ir_builder`, after
the IR is built and before it is verified, and only when neither type checking
nor IR building reported errors. It mutates the IR in place and returns its
diagnostics. For each function (nested functions are analyzed independently):

1. **Solve once.** Solve the stack-state dataflow on the IR as built. Every
   decision below comes from this one solution, so an inserted drop is never
   reported as the site of a move.
2. **Replay and select.** Replay each reachable block from its entry state.
   Report loads of moved or uninitialized places. Select drops before each
   `ScopePop` for its still-initialized roots, and before each `Store` for the
   still-initialized part of the target place. Then, for each control-flow
   edge, compare the exit state of its source with the joined entry state of
   its target, and select the merge-balancing drops: whatever is initialized
   on the edge but not on every edge into the target. Roots are balanced in
   drop order: local variables in reverse declaration order
   (`IRVariable::declaration_order`), then parameters and captures as at the
   root scope's exit.
3. **Insert.** `DropElaborator::insert_drops` inserts every selected drop at
   once through an `InstructionInsertion`, whose points all refer to the
   layout that was analyzed. An edge's drops go where they run only when
   control follows that edge. If the source ends in a jump, they go at the
   end of the source block. If it ends in a conditional, the edge is
   critical, so it is split with `Cfg::split_edge` and the drops go in the new
   block. Only edges that need drops are split.

One solution is enough because the join already describes the balanced IR.
A place that may be uninitialized on some edge into a merge is uninitialized
in the joined state, which is exactly the state that dropping it on the other
edges produces. So scope exits and reassignments after a merge see every place
initialized on all paths or on none, as they would after balancing.

The replay also checks operation handlers. In an operation handler, captures
are left out of the root scope's drops, and every load that moves out of a
place rooted in a capture is reported as `MoveOutOfHandlerCapture`. The
dataflow does not mark such a capture as moved, so the error does not cascade
into use-after-move errors.

The stack state tracks roots (`StackRoot`: variables, parameters, lambda and
operation-handler parameters, and captures) and, for each root, a `PlaceState`
that is either uniform or tracked per field. `DropElaborator`
(`memory/src/drop_elaboration.rs`) turns those states into drops and resolves
their dictionaries with the enclosing definition's `Solver`, caching the result
per type. Resolution itself is `drop_resolution::resolve_drop_instance`, which
the IR builder shares for handler captures. It also checks the where-clause
predicates required by the selected instances.

### Diagnostics

A missing dictionary is reported once per binding and type, at the binding's
declaration:

- `UnresolvedScopeDrop`, for example
  `no implicit instance found: core.Drop[a]`;
- `UnsatisfiedScopeDropPredicate`, when a selected instance's where-clause
  does not hold.

A type that already contains an error is skipped, since that error was reported
where the type was written.

## Mono IR

Inserted drops need no special handling in `rayc_mono_ir_builder`: they are
ordinary instance calls. `lower_call` handles every kind of `Drop` dictionary:

- no-op dictionaries emit nothing;
- tuple and closure dictionaries expand into per-element drops;
- nominal dictionaries call the generated drop body;
- user-written instances are ordinary calls.

`ExprDiscard` goes through `lower_drop`, which handles the same kinds.

`lower_handle` builds the handlers' shared environment once, in the enclosing
function, and passes a pointer to it to every handler. After the call to the
handled body, it drops the environment's capture fields in reverse order with
`lower_drop`, using the dictionaries recorded on the `Handle`.

# Limitations

- **Assignments through a pointer** (`p.* = v`) do not drop the previous value.
  The analysis does not track memory behind pointers.
- **Partial moves bypass a user-written `Drop`.** Moving a field out of a
  struct that has a user-written `Drop` instance means its remaining fields are
  dropped one by one, and the user's `drop` never runs. Rust rejects moving out
  of such types instead.
- **No borrow checker yet.** A closure that captures by reference can outlive
  the binding it points to, and a moved-out value can still be reached through
  a pointer. These programs compile today and misbehave; Rust's borrow checker
  rejects them, and Ray will too once it has one.
- **No by-value handler captures.** Since a handler may not move out of its
  captures, a handler that captures by value is always an error. Rust allows
  such captures through `move ||`, which captures by value without moving
  inside the body; Ray has no closure-level `move` yet.
- **Diagnostic wording.** The missing-dictionary error always says "this value
  is dropped when it goes out of scope", even when the drop comes from a merge
  or a reassignment.

# Yielding Effect

Status: design draft. Nothing here is implemented yet.

Today an effect handler is a callback passed to the callee: a `perform` calls
the handler, the handler returns a value, and the body carries on. Handlers
are therefore always tail-resumptive. That is enough for things like
dependency injection, but it cannot express generators, async, or anything
else where the computation has to stop and be picked up again later.

This document adds that: a function can be suspended into a **coroutine**,
which is a first-class value that can be resumed later or passed around. The
coroutines are stackless, in the style of Rust's `Coroutine` trait, which is
the basis of Rust's `async`.

The document has four parts:

1. [Surface design](#surface-design): what the user writes and how it is
   typed.
2. [The coroutine type](#the-coroutine-type): how the anonymous type of a
   coroutine is represented, and what the type system can ask of it.
3. [Lowering](#lowering): how an effectful function becomes a state machine.
   Nothing in this part is visible to the user.
4. [Borrow checker impact](#borrow-checker-impact) and
   [open questions](#open-questions).

## Example

```ray
eff Yield[t]:
    def yield(value: t) -> ()


def numbers() -> () \ {Yield[int32]}:
    Yield.yield(1)
    Yield.yield(2)


def main() -> int32:
    let mut con = coro numbers()
    let mut sum = 0

    loop:
        handle con():
            case Yield.yield(value) resume k:
                sum += value
                con = k
            case return:
                break

    return sum
```

`coro numbers()` suspends `numbers` before it runs and gives back a
`Continuation[(), ...]`: a new coroutine is always resumed with the unit
value. Each `handle con()` resumes the coroutine with it, and ends in one of two
ways:

- The coroutine performs `Yield.yield`. It suspends, and the `yield` clause
  runs with the yielded value and `k`, the continuation of the rest of the
  coroutine. `Yield.yield` returns `()`, so `k` is a `Continuation[(), ...]`
  too. It has the same type as `con` and can be stored back into it.
- The coroutine completes. The `return` clause runs with the value the
  coroutine returned, and leaves the loop.

The clauses are branches of `main`, like the arms of an `if`. That is why
`con = k` and `break` act on `main`'s own variable and loop, and why the move
analysis accepts the loop: `handle con()` moves `con`, the `yield` branch
initialises it again before the next iteration, and the `return` branch never
reaches the next iteration.

# Surface Design

## The `Coroutine[c]` trait

```ray
pub trait Coroutine[c]:
    pub type Return
    pub type Effect: Effect
```

- `c` is a coroutine type: the type of a suspended function. It is generated
  by the compiler and cannot be named, like the type of a closure.
  [The coroutine type](#the-coroutine-type) describes how it is represented.
- `Return` is the type the coroutine completes with. It is the counterpart of
  `Coroutine::Return` in Rust.
- `Effect` is the row of effects the coroutine may perform while it runs.

The compiler generates the instance for every coroutine type. The trait is
indexed by `c` alone, because neither associated type depends on where the
coroutine is currently suspended. It follows the shape of `core.Def[f]`, and
generic code constrains it the same way, with
`where (co.Return = ..., co.Effect = ...)`.

For every `c` that has an instance, `&mut c` has one too, with the same
`Return` and `Effect`. This is what the [pinned](#pinned-storage)
continuations of the first prototype use.

## The `Continuation[r, c]` type

```ray
pub type Continuation[r, c] = ...
```

A `Continuation[r, c]` is a suspended coroutine of type `c` that is waiting
for a value of type `r`.

- `c` stays the same for the whole life of the coroutine. In the first
  prototype it is always a `&mut` to a coroutine
  [pinned to the stack](#pinned-storage).
- `r` is the type needed to resume it from the point where it is suspended.
  It changes from one suspension to the next.

`Continuation` is a type-state: the only way to resume a coroutine is through
a continuation, and its `r` says exactly what the suspended `perform` is
waiting for. Resuming with the wrong type of value is a type error, and
resuming a coroutine that has completed is impossible, because completion
produces no continuation.

Only the compiler creates continuations: `coro` creates the first one, and
each suspension creates the next.

## Creating a coroutine: `coro`

```ray
let k = coro numbers()      # k: Continuation[(), &mut <coroutine type of numbers>]
```

`coro f(args)` evaluates the arguments, stores them in a new coroutine and
gives back a continuation for it, without running any of `f`. For the
coroutine type `c` it creates:

- `Coroutine[c].Return` is the return type of `f`;
- `Coroutine[c].Effect` is the effect row of `f`;
- the first continuation has `r = ()`.

### Pinned storage

In the first prototype, `coro` pins the coroutine to the stack. The coroutine
itself is placed in hidden storage in the enclosing scope, and the
continuation only refers to it: `coro` gives back a
`Continuation[(), &'a mut c]`, where `'a` is the lifetime of that storage.

This is the idea of `std::pin::pin!` in Rust. The pinned value can no longer
be named; it can only be reached through the reference that the macro gives
back, a `Pin<&mut T>`.

What follows from it:

- **The coroutine never moves.** Moving a continuation moves a reference. A
  continuation does not give out the `&mut` it holds, so nothing can move the
  coroutine from behind it.
- **A continuation cannot outlive the scope that created it.** Returning it,
  or storing it in something that lives longer, is an ordinary borrow error,
  which the borrow checker already reports:

  ```ray
  def make() -> Continuation[(), ...]:
      return coro numbers()       # ERROR: borrows storage local to `make`
  ```

- **It can still be passed down.** A continuation can be given to a callee,
  kept in a local, and assigned again, as the [example](#example) does with
  `con = k`. Generic code needs nothing new: its `c` is instantiated with
  `&'a mut <coroutine type>`.
- **No boxing and no type erasure by the user.** Both need a coroutine that
  can leave the stack.

These restrictions are meant to be lifted once moving and pinning have a
design; see [open questions](#open-questions).

## Invoking a continuation

A continuation can be called like a function:

```ray
let result = k(value)
```

For `k: Continuation[x, y]` and `value: x`, the call resumes the coroutine and
runs it to completion. Every effect the coroutine performs is forwarded to the
caller, so the call has the effect `Coroutine[y].Effect` and the type
`Coroutine[y].Return`.

This is the same as a `handle` with no clauses.

## Handling a coroutine: `handle`

```ray
handle expr1(expr2):
    case Label.op(p1, ..., pn) resume k:
        ...
    case return x:
        ...
```

A `handle` resumes the continuation `expr1` with the value `expr2`.
`handle expr1()` resumes with `()`, and `handle coro f(x)` starts the
coroutine that the `coro` creates. It has two kinds of clauses:

- An **operation clause** runs when the coroutine performs that operation.
  The operation is written as it is performed, so the clause binds exactly
  the operation's arguments. `resume k` names the continuation of the rest of
  the coroutine; a clause that leaves it out drops the continuation, which
  abandons the coroutine. The type arguments of the label may be written, as
  in `case Yield[int32].yield(value)`, and are inferred otherwise.
- The **return clause** runs when the coroutine completes. `case return x`
  binds the value the coroutine returned and `case return` ignores it. It may
  be left out, which is the same as writing `case return x: x`.

The clauses of one effect do not have to be next to each other, but an effect
that has a clause needs one for each of its operations.

### Clauses are branches

The clauses are not functions. They are branches of the enclosing function,
like the arms of an `if`: exactly one of them runs, and execution then
continues after the `handle`. Each clause is a plain edge in the control-flow
graph of the enclosing function, so:

- `break`, `continue` and `return` in a clause act on the enclosing loop and
  function.
- A clause uses the enclosing variables directly. There are no captures: it
  may move, assign and borrow them as any other branch may.
- The move analysis and the borrow checker follow each clause as they follow
  any branch. In the [example](#example), this is what lets `con` be moved by
  the `handle` and initialised again by the `yield` clause.

This differs from the handlers of `run ... with`, which are functions: there,
`return` gives the result of the operation.

The body of a clause is an arm, as in `if`: either an indented block, which
has no value, or `: expression`, whose value is the value of the `handle`.

### Typing

- `typeof(expr1) ~ Continuation[x, y]`
- `typeof(expr2) ~ x`
- there is an instance `Coroutine[y]`
- `Coroutine[y].Effect ~ {Label | rest}`
- for an operation `def op(p1: t1, ..., pn: tn) -> q` of `Label`, the clause
  binds `p1: t1, ..., pn: tn` and `k: Continuation[q, y]`
- the return clause binds `x: Coroutine[y].Return`
- every clause has the same type `b`, the **answer type**, which is the type
  of the whole expression. A clause that leaves through `break`, `continue`
  or `return` fits any `b`
- without a return clause, `b` is `Coroutine[y].Return`
- the `handle` performs `rest`. What the clauses perform is checked as part
  of the enclosing function, like any other code in it

A label can only be handled if it is visible in `Coroutine[y].Effect`. A row
that is opaque at the `handle`, such as a row variable or an associated type
like `d.Effect` that does not reduce, cannot be handled. It is forwarded as a
whole.

### Evaluation

1. Resume the coroutine with the value of `expr2`.
2. If the coroutine **completes** with a value, the return clause runs with
   it. The value of the clause is the value of the `handle`.
3. If the coroutine performs an operation that **has a clause**, the coroutine
   suspends and the clause runs with the operation's arguments and the
   continuation `k`. The value of the clause is the value of the `handle`.
4. If the coroutine performs an operation that **has no clause**, the
   operation is forwarded: it is performed in the context around the
   `handle`, the coroutine is resumed with the result, and evaluation
   continues from step 2 under the same `handle`.

### Shallow handlers

The handler is **shallow**. It handles at most one operation, and the
continuation `k` bound by the clause is not wrapped in the handler again. To
keep handling, the program has to put `k` through another `handle` itself.

There are two ways to do that. The loop in the [example](#example) stores `k`
and handles it again on the next iteration. The other is an explicit
recursion: the clause calls the function that contains the `handle`, passing
the new continuation.

### Continuations of different types

Each operation clause gets a continuation whose `r` is the return type of its
operation:

```ray
eff State[a]:
    def get() -> a
    def set(value: a) -> ()

handle k(v):
    case State.get() resume k1:         # k1: Continuation[int32, c]
        ...
    case State.set(value) resume k2:    # k2: Continuation[(), c]
        ...
```

As long as `r` stays the same from one suspension to the next, as with
`Yield` in the [example](#example), the continuation keeps one type and can
be stored in a single variable or field. That is what makes the loop
possible.

When `r` differs, no single variable can hold every continuation, and the
driver has to recurse:

```ray
eff Ask:
    def ask() -> int32


def addTwo() -> int32 \ {Ask}:
    return Ask.ask() + Ask.ask()


# Answers the first `ask` with `n`, the next with `n + 1`, and so on.
def supply(k: Continuation[r, c], value: r, n: int32) given (co: Coroutine[c]) -> int32
    where (co.Return = int32, co.Effect = {Ask}):
    return handle k(value):
        case Ask.ask() resume next: supply(next, n, n + 1)
```

`supply(coro addTwo(), (), 1)` is 3. The first continuation waits for `()`
and every later one for an `int32`, which is why `supply` is generic over
`r`. The clause does not return the answer to `ask`; it resumes `next` with
it. There is no return clause, so when the coroutine completes its value is
the value of the innermost `handle`, and each `supply` returns it outwards.

Later, once a coroutine can be boxed, type erasure could remove the coroutine
type as well, giving something like `Continuation[(), BoxedCoroutine]` that a
scheduler can keep in a queue.

## Ownership and drop

- A continuation is **one-shot**. Invoking or handling it consumes it by
  value, and `Continuation` is never `Copy`.
- A continuation **can be dropped** instead of resumed, which abandons the
  coroutine. The coroutine's [storage](#pinned-storage) owns its locals:
  those still live at the suspension point are dropped when the storage goes
  out of scope, as if the coroutine's scopes had ended there.
  [Dropping a coroutine](#dropping-a-coroutine) describes how.

# The Coroutine Type

The type system needs a representation for the anonymous coroutine type `c`.
This is more than an implementation detail: the user can ask `c: Send` or
`c: 'static` of it, as they can of a closure type, and the answers are part
of the language.

The representation follows the closure type where it can. Where a coroutine
differs from a closure, this part says so.

## Representation

A closure type is a `Constant::Closure` applied to these arguments:

```
Closure(poly_args, parameter_types, return_type, effect_row, (capture_1, capture_2, ...))
```

- `poly_args` are the polymorphic arguments of the scope the closure is
  written in. A closure inside a function with a type variable `a` has `a` in
  its `poly_args`.
- `parameter_types`, `return_type` and `effect_row` are the signature of the
  closure.
- The last argument is a tuple of the types of the captured variables.

A closure carries its signature and its captures as arguments because they
are inferred from its body. A coroutine has nothing to infer. It suspends a
declared function, and the declaration already fixes all of it. So a
coroutine type is only the function and its polymorphic arguments:

```
Coroutine(def_id, poly_args)
```

- `def_id` is the function given to `coro`. It is part of the constant, like
  the identity of a closure, and not a type argument. There is one coroutine
  type per function, not one per `coro` expression: `coro f(1)` and
  `coro f(2)` have the same type.
- `poly_args` are the arguments for the polymorphic parameters of that
  function, followed by those of each enclosing symbol. For a closure they
  belong to the scope the closure is written in; for a coroutine they belong
  to the function it suspends.

Everything else is derived by substituting `poly_args` into the signature of
`def_id`, which is a query:

- `Coroutine[c].Return` is the return type;
- `Coroutine[c].Effect` is the effect row;
- the **upvars** are the parameter types. A coroutine that has not been
  resumed yet holds its arguments, so they count as part of its contents.

Storing these as arguments would only repeat what `poly_args` already
determine.

Like a closure type, a coroutine type is invariant in all its arguments.

### Nested bodies

A `def` is not the only body that can be suspended. A closure, the body of a
`run ... with` and an operation handler are bodies of their own, and each of
them can perform effects.

**A closure** is the same kind of thing as a `def`. `coro` can be applied to
a call of it, so its coroutine type is a first-class type, created during
type checking. It is a `Coroutine` keyed by the identity of the closure
instead of a `def_id`. It takes the closure type as its argument, which
already carries the polymorphic arguments and the captures. Its saved locals
are looked up by the [query](#saved-locals), as for a `def`.

**The body and the handlers of a `run ... with`** are different. Their
coroutines are never values. They only appear among the saved locals of the
enclosing function (see
[`run ... with` in a coroutine](#run--with-in-a-coroutine)). They get a
constant of their own, the **local coroutine type**, which is laid out like
a closure type, with everything it holds embedded:

```
LocalCoroutine(poly_args, (upvar_1, upvar_2, ...), (saved_1, saved_2, ...))
```

- The constant identifies the nested body inside its enclosing definition,
  as the constant of a closure type does.
- `poly_args` are those of the enclosing function, as for a closure.
- The upvars are the captures of the body. For a handler they are the
  parameters of the operation and its captures.
- The last argument is a tuple of the saved locals of the nested body.

A `Coroutine` cannot embed its saved locals, for the two reasons given under
[saved locals](#saved-locals). Neither applies here:

- **They are known when the type is created.** Only the saved-locals query
  of the enclosing function creates a local coroutine type. That query runs
  after `BuildIR`, which builds the nested bodies together with the
  enclosing one.
- **They cannot mention the type itself.** The body and the handlers of a
  `run` have no name, so nothing can call them recursively. What they hold
  is either a `Coroutine`, which is named and not expanded, or the local
  coroutine type of a `run` nested inside them, which is strictly smaller.

What follows from the layout:

- **Markers** are structural, as for a tuple: every upvar and every saved
  local has to satisfy the marker. There is no query and nothing to
  [defer](#staging), because the type does not exist before the saved locals
  do.
- **Lifetimes** in it are erased, like everything the saved-locals query
  returns.
- **`Drop`, `Copy` and outlives** are never asked of it. It is not a value:
  it is dropped only as part of abandoning the `run` that owns it.

## Saved locals

The upvars are not everything a coroutine holds. It also holds its **saved
locals**: the locals that are live across a suspension point. They decide
questions like `c: Send`, so the type system has to be able to see their
types.

They are not arguments of the coroutine type, for two reasons:

- **They are not known when the type is created.** `coro f()` is typed from
  the signature of `f`. The saved locals come from the body of `f`.
- **They can mention the coroutine type itself.** While `f` waits for a
  callee `g`, the state machine of `g` is one of the saved locals of `f`. A
  recursive `f` therefore lists its own coroutine type, and the type system
  has no recursive types. The [box](#recursion) that makes such a coroutine
  finite is a matter of layout: the query lists the callee's coroutine type
  itself.

A struct has the second problem too, and solves it the same way: its fields
are not part of its type, they are looked up. So the saved locals are a
query, with a signature like `def_id -> Vec<Ty>`. The types it returns are
written over the polymorphic parameters of `def_id`, and the caller
substitutes `poly_args`, as it does for the fields of a struct.

### What counts as saved

Which locals are saved decides whether a program is accepted, so it needs a
fixed definition, not whatever the lowering happens to do:

```ray
def f() -> () \ {Yield[int32]}:
    let guard = lock()      # not `Send`, and has a `Drop` instance
    use(guard.&)
    Yield.yield(1)
    # `guard` is not used again, but it is dropped here
```

If only later uses counted, `guard` would not be saved and the coroutine of
`f` would be `Send`. But `guard` is dropped at the end of the scope, after
the `perform`, so it has to survive the suspension. The rules:

- **Drops are uses.** The saved locals are computed on the IR after memory
  analysis has inserted the drops. `guard` above is saved, and the coroutine
  is not `Send`. This is the same rule as in Rust, where a guard held across
  an `await` makes the future `!Send`. The fix is the same too: end the scope
  of `guard` before the `perform`.
- **Suspension points are fixed by the source.** Every `perform`, and every
  call of an effectful function, is a suspension point for this query,
  however it is lowered.
- **Precision is a promise.** A more precise analysis accepts more programs
  and can be added later. A less precise one breaks programs that used to
  compile.

There are therefore two notions of saved locals:

- The **typed saved locals** are the result of this query. They are fixed by
  the rules above and are what the type system sees.
- The **layout saved locals** are what the code generator stores. It
  computes them after monomorphisation and may store less, for example if an
  effect [stays a callback](#open-questions) inside a coroutine.

Only the first is part of the language. An optimisation may change the
layout of a coroutine, never whether `c: Send` holds.

[Computing the saved locals](#compute-the-saved-locals-of-a-coroutine)
describes how the query is implemented.

## Markers

Marker entailment treats a coroutine type like a struct. `c: M` holds if
every upvar and every saved local, after substituting `poly_args`, satisfies
`M`.

The solver is already coinductive, so a recursive coroutine is not a
problem: the goal for the inner coroutine is the goal being solved, and it
is assumed.

`Copy` is the exception. **A coroutine type is never `Copy`**, and this is
hard-wired before the structural rule:

- Copying a suspended coroutine would let it be resumed twice, which
  contradicts [one-shot](#ownership-and-drop) continuations.
- A coroutine may hold borrows of its own locals, so a bitwise copy would
  point into the original.

It also means that `Copy` never needs the saved locals. That matters for
[staging](#staging), because the type checker asks `Copy` on every implicit
load.

## Staging

The fields of a struct come from its declaration. The saved locals come from
a body: they need its typed AST, its IR and its memory analysis. Marker
entailment, however, already runs during type checking. Asked naively, a
function that wants a marker of its own coroutine type is a cyclic query:

```ray
def f() -> () \ {Yield[int32]}:
    requireSend(coro f())
    # type check f -> `Send` for the coroutine of f -> saved locals of f
    #   -> IR of f -> type check f
```

Mutually recursive functions give the same cycle. The coinductive solver
does not help here: it handles a goal that depends on itself, not a query
that does.

The rule that avoids it:

> The saved-locals query sits above `BuildIR`. Nothing that runs during type
> checking or during `BuildIR` may call it. Until then a coroutine type is
> opaque.

Each consumer has to follow this rule in its own way:

| Question                 | Answer without the saved locals                     |
| ------------------------ | --------------------------------------------------- |
| `c: Copy`                | always false ([markers](#markers))                  |
| `c: 'a`                  | from `poly_args` alone ([lifetimes](#lifetimes))    |
| `Drop` dictionary of `c` | from the upvars alone ([drop](#dropping-a-coroutine)) |
| any other marker         | deferred, see below                                 |

A marker goal that reaches a coroutine type is neither proved nor refuted
during type checking. The solver answers "deferred", the goal is recorded
with the body, and the borrow checker phase proves it. That phase runs on
the IR, after `BuildIR`.

Deferring is sound because a marker goal is a yes-or-no check. It binds no
inference variable and selects no instance, so answering it later cannot
change what type inference concludes. `Copy` is the one marker that feeds
back into inference, through moves and capture modes, and it is never
deferred.

No cycle is left. Proving the deferred goals of `f` needs the saved locals
of some `g`, which need `BuildIR` of `g`, which needs the typed AST of `g`.
Nothing in that chain proves a deferred goal or asks for saved locals, so it
ends even when `g` is `f`.

rustc does the same: obligations on a coroutine are stalled during type
checking and checked once its MIR is built.

The alternative is to declare instead of infer: a function states which
markers its coroutine form has, the body is checked against that, and
callers trust the declaration. Nothing then depends on a body, and changing
a body cannot break a caller. The cost is an annotation on every function
whose coroutine is sent anywhere. It is not pursued for now.

## Lifetimes

The types returned by the saved-locals query have their lifetimes erased
(`Lifetime::Erased`, which already means "any lifetime, it does not
matter").

This is sound because of what a lifetime in a saved local can be:

- **Local to the coroutine**: a borrow of another saved local. Nothing
  outside the coroutine can name it.
- **External.** A body can only get an external lifetime through its
  parameters, its effect row or its polymorphic parameters. All three are
  the signature instantiated with `poly_args`, so every external lifetime in
  a saved local also appears in `poly_args`.

Each question the type system asks then has an answer that does not need the
erased lifetimes:

- **Markers.** Lifetimes never decide marker entailment, and a marker
  implementation cannot depend on one (see
  [instance selection ignores lifetimes](BORROW_CHECKER_PLAN.md#instance-selection-ignores-lifetimes)).
  Erasing them loses nothing.
- **Outlives.** `c: 'a` holds when every argument in `poly_args` outlives
  `'a`. This is how a closure type is decomposed today, so it needs no
  change and no query. It is conservative: an argument that reaches no
  parameter, effect or saved local still counts. Looking only at the derived
  upvars and effect row would be more precise, but unsound. A function
  `f[t]` that builds a `t` from a `given` dictionary and keeps it across a
  `perform` has a saved local of type `t`, and `t` appears in neither.
- **Drop check.** This has two halves.
  - *Inside.* What an abandoned coroutine drops is code in the coroutine's
    own function (see [dropping a coroutine](#dropping-a-coroutine)). The
    borrow checker checks it there, on the IR, with real lifetimes.
  - *Outside.* The owner of a coroutine drops it at the end of the storage's
    scope, and the owner's borrow checker has to know which of the owner's
    regions that drop uses. For a struct it reads this from the fields. For
    a coroutine the fields are saved locals with erased lifetimes, so it
    cannot tell. Dropping a coroutine therefore uses every region in
    `poly_args`, as dropping through a `given Drop[a]` dictionary uses every
    region in `a`.

The outside half is where a coroutine is stricter than a closure:

```ray
def f(p: &'a int32) -> () \ {Yield[int32]}:
    ...


def owner():
    let mut x = 1
    let k = coro f(x.&)
    x = 2       # ERROR: the coroutine is dropped at the end of `owner`,
                # and that drop may use the borrow of `x`
```

A closure that captured `x` by reference and is not used again would allow
`x = 2`,
because dropping a reference uses no region. The coroutine might hold a
saved local with a user-written `Drop` instance that reads through `p`, and
the owner cannot see whether it does.

All of this has to be revisited if a marker implementation is ever allowed
to depend on a lifetime, that is, once marker heads are no longer restricted
to one constructor applied to distinct variables. The saved locals then need
real lifetimes. Rust binds them as higher-ranked lifetimes, which Ray does
not have yet. It may also be possible to be more precise than that, by
writing them over the external lifetimes that the borrow checker solved for
the body.

## Dropping a coroutine

A coroutine can be [dropped](#ownership-and-drop) instead of resumed. What
it holds at that moment depends on its state:

- A coroutine that was **never resumed** holds its upvars, the arguments it
  was created with.
- A **suspended** coroutine holds the saved locals of the suspension point
  it is at.
- A **completed** coroutine holds nothing.

The two kinds of contents are dropped in different ways.

**Upvars are dropped like the captures of a closure.** `Drop[c]` for a
coroutine type `c` is a built-in instance that takes one `Drop` dictionary
per upvar, as the instance of a closure takes one per capture. The upvars
come from a signature, so this needs no body. The dictionaries are requested
where the coroutine is created: `coro f(args)` needs `Drop` for the type of
every argument, resolved with the solver and givens of the function that
contains the `coro`. A `@linear` argument has no `Drop` instance, so giving
one to `coro` is an error.

Only a coroutine created by `coro` can be dropped before it starts. The
coroutine of a callee, and of the body or a handler of a `run ... with`, is
started as soon as it is created, so its upvar dictionaries are never
needed. An ordinary call of an effectful function is therefore not
restricted: it may take a `@linear` argument.

**Saved locals cannot be dropped that way**:

- What has to be dropped depends on where the coroutine is suspended.
- The saved locals are not in the type. A dictionary per saved local in the
  instance would bring back the recursion that the query avoids.
- Memory analysis of the owner selects the drop, and by the
  [staging](#staging) rule it may not ask for the saved locals.

So their drop is compiled into the coroutine's own function, as **abandon
edges**. In the coroutine form, every suspension point gets a second
successor, the abandon edge. It is taken when the coroutine is dropped while
it is suspended there, and it leaves the function through the enclosing
scopes. Memory analysis of the coroutine's own function treats it like any
other edge: it inserts drops for whatever is initialised there, and resolves
their dictionaries with the function's own solver and givens. This is the
`drop` edge of a `Yield` terminator in rustc.

Dropping a coroutine dispatches on its state. Before the first resume it
drops the upvars with the dictionaries of the instance, in the order a
function drops its parameters. At a suspension it runs the abandon edge.
After completion it does nothing.

This is the first place the saved locals matter, before any marker: in the
first prototype every coroutine is dropped, at the end of its
[storage](#pinned-storage).

### What a function may hold across a suspension

The abandon edge drops every local that is live across its suspension point,
so each of them must have a `Drop` dictionary in the function itself:

- A local of an opaque type, such as a polymorphic `t`, needs `Drop[t]`
  among the function's own givens.
- A `@linear` local has no `Drop` instance. Holding one across a suspension
  point is an error.

The start of a function is not a suspension point. A parameter that is
consumed before the first suspension needs nothing here: before the
coroutine starts, its arguments are dropped with the dictionaries from the
`coro`. A parameter that is live across a suspension is a saved local like
any other.

Every `perform` and every call of an effectful function is a suspension
point, and any effectful function can be given to `coro`. So these rules
apply to every effectful function, whether or not it is ever suspended.

They are stricter than the direct form needs:

```ray
def keep[t](x: t) -> t \ {Yield[int32]}:
    Yield.yield(1)
    return x
```

In direct form `keep` never drops `x`, but `x` is live across the `perform`,
so `keep` needs a `given` for `Drop[t]`. This is a known rough edge.

In return, whether a coroutine type has a `Drop` instance depends only on
its upvars, which come from a signature. No question about dropping a
coroutine depends on a body, and nothing about `Drop` has to be
[deferred](#staging).

A coroutine that is itself linear, one that must be run to completion, would
have no abandon edges and would lift these restrictions. See
[open questions](#open-questions).

## Coroutines of unresolved calls

`Coroutine(def_id, poly_args)` needs to know the callee. A call knows it
when its target is a function (`CallTarget::Direct`). A call through an
instance that is not known yet does not
(`CallTarget::UnresolvedInstanceAssociated`): in
`def f[t]() given (d: Tr[t])`, the callee of `d.m()` is known only when `d`
is. Calling a callable value is the same case, because `fn(x)` is
`Def.call` on the `Def` dictionary of `fn`.

The coroutine type of such a call is needed in two places:

- as a saved local: while `f` waits for `d.m()`, the state machine of
  `d.m` is a saved local of `f`;
- as the type of `coro d.m()` itself.

So there is a second constant, which mirrors the call target. Its name is a
placeholder:

```
InstanceCoroutine(trait_def_id, instance, poly_args)
```

- `trait_def_id` is the trait method. Like `def_id` above, it is part of the
  constant.
- `instance` is the instance term, such as `d`.
- `poly_args` are the arguments for the method's own polymorphic parameters.
  Those of the trait are told by `instance`.

It is a projection, and behaves like an associated type of an instance
(`Constant::InstanceAssociated`):

- **It reduces.** Once `instance` normalises to a concrete instance, the
  type reduces to the `Coroutine(...)` of that instance's implementation of
  the method. For the built-in `Def` instance of a closure, it reduces to
  the coroutine of the closure body: a `Coroutine` that is keyed by the
  closure instead of a `def_id`, and takes the closure type as its argument.
  After monomorphisation every instance is concrete, so every
  `InstanceCoroutine` has reduced.
- **Until then it is rigid.** It is equal only to itself.
  `Coroutine[c].Return` and `Coroutine[c].Effect` come from the signature of
  the trait method. For `Def.call` they are `d.Return` and `d.Effect`.
- **Markers.** It is opaque: a marker goal on it fails unless a given proves
  it. `Copy` is never proved, even with a given.
- **Outlives.** It is kept whole as an outlives component, like an
  associated type.
- **Drop.** It has the same built-in instance as every other coroutine
  type: one dictionary per upvar, where the upvars are the parameter types
  of the trait method. The projection itself needs no given. This works
  because the `Drop` instance of a coroutine
  [never depends on a body](#what-a-function-may-hold-across-a-suspension).

Two consequences for the lowering:

- **Indirect calls need no boxing.** Ray has no function-pointer types
  today, so every indirect call goes through a dictionary, and the
  dictionary is known after monomorphisation. The layout is only computed
  then, when the callee's state machine has a concrete type.
- **Recursion can hide behind an instance.** A call through `d` may turn out
  to be a call back into the same function. The [boxing](#recursion) for
  recursion therefore has to look for cycles among the concrete coroutine
  types after monomorphisation, not in the source.

### Naming it in a bound

While an `InstanceCoroutine` does not reduce, a marker goal on it needs a
given, so the user needs a way to write one. This is the problem Rust has
with `Send` bounds on the future returned by a trait method. It only comes
up when the goal is asked while the instance is still abstract. At a
concrete instantiation the type reduces and the goal is solved as usual.

The spelling is not decided. The proposal is `coro` in type position:

```ray
def sendIt[t]() given (d: Tr[t]) where (coro d.m: Send):
    requireSend(coro d.m())
```

- `coro d.m` names the coroutine type of the call target, without its
  arguments: there is one coroutine type per callee. A method with its own
  type parameters is written `coro d.m[a]`.
- Lifetime arguments are left out. Markers never depend on a lifetime, so
  one bound covers all of them. This is why Ray does not need the `(..)` of
  Rust's return type notation.
- A bound on `coro d.m[a]` is for that `a` only. A bound for every `a` needs
  higher-ranked bounds over types, which Ray does not have.
- The callable shorthand `fn: def(int32) -> int32` hides its dictionary, so
  there is nothing to write `coro` on. Either the bound requires the
  explicit form, `given (d: core.Def[f])` and `coro d.call`, or the
  shorthand gets its own spelling.
- The same syntax could name the coroutine type of a known function, as in
  `coro f[int32]`. Whether that should be allowed is open: so far a
  coroutine type cannot be named, like the type of a closure.

# Lowering

This part describes how effectful code and `handle` could be compiled. The
types below only illustrate the lowering. None of them is exposed to the
user, and their names are placeholders.

## Two compilation modes

Every effectful function can be compiled in two ways:

- **direct**: what the compiler does today. Handlers are callbacks, and a
  `perform` is a call.
- **coroutine**: the function becomes a state machine. A `perform` stores the
  live locals in the coroutine, returns to whoever resumed it, and continues
  from the same point on the next resume.

`coro f()` uses the coroutine form of `f`. An effectful function called from
a coroutine has to be able to suspend too, so it is also used in its
coroutine form. The exception is a call under a `run ... with` that lets no
effect escape; see [`run ... with` in a coroutine](#run--with-in-a-coroutine).

## The state machine

The state machine has the shape of Rust's `Coroutine` trait:

```ray
enum CoroutineState[y, r]:
    Yielded(y)
    Complete(r)


trait StateMachine[c]:
    type OpYield
    type OpResume
    type Return

    def resume(self: &mut c, arg: this.OpResume)
        -> CoroutineState[this.OpYield, this.Return]
```

`OpYield` and `OpResume` are two sum types generated from the effect row of
the function. Take this function:

```ray
eff State[a]:
    def set(value: a) -> ()
    def get() -> a


eff Print:
    def print(message: cstr) -> ()


def someFunction() -> () \ {State[int32], Print}:
    ...
```

`OpYield` has one variant per operation, holding the operation's arguments:

```ray
enum StateInt32PrintYield:
    StateSet(int32)     # State.set(value: int32)
    StateGet()          # State.get()
    PrintPrint(cstr)    # Print.print(message: cstr)
```

`OpResume` has one variant per operation, holding the operation's return
value, plus a `Start` variant that runs the coroutine up to its first
`perform`:

```ray
enum StateInt32PrintResume:
    Start()                 # start the coroutine
    StateSetResume()        # State.set(...) -> ()
    StateGetResume(int32)   # State.get() -> int32
    PrintPrintResume()      # Print.print(...) -> ()
```

## Why this interface is not the surface

Nothing in `resume` stops the caller from answering an operation with the
wrong variant:

```ray
def askInt32() -> int32 \ {State[int32]}:
    return State.get()


def main() -> int32:
    let mut machine = <state machine of askInt32>

    match machine.resume(Start()):
        # The coroutine asks for `State.get`.
        Yielded(StateGet()):
            # Wrong: this answers `State.set`. It should have been
            # `StateGetResume(<value>)`.
            machine.resume(StateSetResume())
        ...
```

The same goes for passing `Start` twice, or resuming after `Complete`.
`Continuation[r, c]` closes all three holes: `r` selects the one variant that
is valid at the current suspension, and a completed coroutine has no
continuation left to resume.

A consequence is that the tag of `OpResume` carries nothing the type-state
does not already know, so the lowering is free to pass the resume value
untagged.

## Lowering `handle`

`handle k(v): case Label.op(...) ...` is lowered in place, into the control-flow
graph of the enclosing function. It becomes a `resume` followed by a switch
on the result:

1. Wrap `v` in the resume variant that `k`'s type selects and call `resume`.
2. On `Complete(value)`, jump to the block of the return clause, with `x`
   bound to `value`.
3. On `Yielded` with an operation of `Label`, jump to the block of its
   clause, with the parameters bound to the payload and `k` bound to the
   coroutine, wrapped as a `Continuation` of the operation's return type.
4. On `Yielded` with any other operation, perform it in the enclosing
   function, wrap the result in the matching resume variant, and go back to
   step 1.

Every clause block ends by jumping to the code after the `handle`, unless it
leaves through `break`, `continue` or `return`. Since the clauses are
ordinary blocks, the move analysis and the borrow checker need nothing
special for them: the `handle` is a move of `k`, and each clause is one
successor of it.

Invoking a continuation directly is the same without step 3, with a return
clause that passes the value through.

## `run ... with` in a coroutine

`run ... with` is not merged into `handle`. The two differ in where control
is when a clause runs:

- In a `run`, the body is still on the stack, in the middle of a call. The
  handler runs on top of it. So a handler is a function, it reaches the
  enclosing variables through its captures, and all it can do is answer.
- In a `handle`, the body is suspended and control is back in the enclosing
  function. So a clause is a branch, and it may `break`, `return` or drop
  the continuation.

They are still the same thing semantically. `run body with E: ...` means:
create the coroutine of the body and drive it to completion, answering every
operation of `E` with the result of its handler. Passing the handlers as
callbacks, as the compiler does today, is an optimisation of that, valid
because a `run` handler always resumes, exactly once and at once.

A function in direct form compiles every `run` as it does today. In a
function in coroutine form, it depends on the effect of the `run` itself.
That effect has two sources: effects of the body that the handlers do not
handle, and effects that the handlers perform.

1. **Nothing escapes.** Every effect of the body is handled and no handler
   performs an effect. The `run` is not a suspension point. It is compiled
   as today: the body is a regular function and the handlers are callbacks.
   The same holds for everything the body calls.
2. **The body has an unhandled effect.** The effect has to propagate through
   the `run`, so the body has to be able to suspend. It is reified into a
   coroutine like any other function.
3. **A handler performs an effect.** The handler is reified. This forces the
   body to be reified as well, even when none of its own effects escape. A
   handler runs when the body performs the operation, that is, from inside
   the body. Coroutines are stackless: for the handler to suspend the
   enclosing function, every frame between them has to be a state machine,
   and the body, with whatever it called to reach the `perform`, is between
   them.

| Body has an unhandled effect | A handler performs an effect | Body      | Handlers            |
| ---------------------------- | ---------------------------- | --------- | ------------------- |
| no                           | no                           | function  | callbacks           |
| yes                          | no                           | coroutine | called at the `run` |
| either                       | yes                          | coroutine | coroutines          |

In cases 2 and 3 the `run` becomes a driver, in place, of the same shape as
[lowering `handle`](#lowering-handle):

1. Build the coroutine of the body from its captures and start it.
2. On `Complete(value)`, the `run` has the value `value`.
3. On `Yielded` with an operation of `E`, call its handler. If the handler
   is reified, drive it in the same way: every operation it yields is
   forwarded, until it completes. Resume the body with the result and go
   back to step 2.
4. On `Yielded` with any other operation, forward it: perform it in the
   enclosing function, resume the body with the result, and go back to
   step 2.

Inside a reified body, a `perform` of `E` is therefore a suspension that
goes up to the `run`, not a call of the handler. This keeps every callee
static. The `run` knows its body and its handlers, which are nested bodies
of the same function, so it can hold their state machines without a box. A
`perform` does not know which handler it will reach, so it could not.

The price is that an operation of `E` now passes through every frame
between the `perform` and the `run`, where a callback is one indirect call.
In case 2 that is not necessary: the handlers do not suspend, so `E` could
stay a callback inside the reified body. That needs a function to be
compiled per effect, with some effects suspending and others not; see
[cost of reification](#open-questions).

For the type system, a `run` that is a suspension point adds to the
[typed saved locals](#what-counts-as-saved) of the enclosing function: the
[local coroutine type](#nested-bodies) of the body, the local coroutine
type of each handler, and the handler captures.

## Recursion

A coroutine stores the state machine of the function it is currently calling.
A recursive function would therefore have to store itself, so recursion needs
an indirection. For the first version the compiler boxes the state machine
implicitly, rather than asking the user to do it.

This boxing is internal to the lowering. It is separate from letting the user
box a coroutine, which the first prototype does not support.

## Compute the saved locals of a coroutine

**Drafting!**

[What counts as saved](#what-counts-as-saved) defines the typed saved
locals. This section is about how to compute them, and the layout saved
locals.

- The typed saved locals are computed on the IR after memory analysis, from
  the liveness at each `Suspend` and each forwarding `Handle` (see
  [IR update](#ir-update)). `rayc_ir::liveness` already computes what is
  needed: the locals and the expression values that are use-live or
  drop-live.
- A local or expression value is saved if it is live on either edge out of
  the terminator.
- Liveness is not enough for a borrowed local. A `RefOf` is a use only where
  it is evaluated, so a local that is borrowed, has a no-op drop and is not
  used again is not live, but its storage has to survive the suspension. A
  local that may be borrowed while its storage is live is saved too.
- What the expression waits for is saved as well: the coroutine type of the
  callee, or what a `run` adds.

## Optimal coroutine storage layout

**Drafting!**

- concerns about how do we assign a storage location for each saved local
- alignment optimization!
- once the local is saved into the coroutine storage, it will be pinned
- how do we reuse the storage of a coroutine once some of its locals are 
  dropped or no longer used?

## IR Update

There is no new level of IR, and the IR is not rewritten into a coroutine
form. A body is lowered to IR once, by the IR builder, and that IR serves
both [compilation modes](#two-compilation-modes). What changes is that the
IR makes suspension explicit, so that every analysis sees what the coroutine
form can do. The two modes only part when the IR is lowered to MonoIR.

The analyses therefore run once, and they check the rules of the coroutine
form. That is the stricter of the two. The direct form runs the same
control-flow graph and never takes an abandon edge, so a body that is
accepted here is valid in both.

The names below are placeholders.

### Suspension points

Today a `perform` and a call are expressions in the middle of a block. An
[abandon edge](#dropping-a-coroutine) is an edge of the control-flow graph,
so a suspension point has to end its block. A new terminator does that:

```
Suspend { expression, resume, abandon }
```

- `expression` is the `Perform`, `Call` or `RunWith` expression. It is
  unchanged: it is still an expression and still defines the value. The
  terminator only says where it is evaluated, and that control may not come
  back from it.
- `resume` is the block that continues with the value. The value is defined
  on this edge only.
- `abandon` is the block taken when the coroutine is dropped while it is
  suspended here.

The expressions that suspend are:

- every `perform`;
- every call whose effect row is not empty. A row variable counts, because
  it may be instantiated with an effect;
- a `run ... with` that has an effect of its own (see
  [below](#run--with-in-the-ir)).

A call with an empty row stays an ordinary instruction.

The abandon block does what a `return` at the same point does: it pops the
enclosing scopes, in order. It ends in a new exit terminator, `Abandon`,
which is a `Return` without a value.

Taking the abandon edge first abandons whatever the expression was waiting
for: the coroutine of the callee, or the body and the handler of a `run`.
The terminator implies this and the lowering adds it. That coroutine has
been started, so it is at a suspension point of its own and no upvar
dictionary is involved. The arguments of a call are not dropped on the
abandon edge. They were moved into the callee.

The start of a function is not a suspension point and has no abandon block.
A coroutine that was never resumed is dropped by its
[`Drop` instance](#dropping-a-coroutine), with the dictionaries of its
upvars.

### `coro`

`coro f(args)` is lowered to several instructions, not one:

1. A hidden variable is declared for the
   [pinned storage](#pinned-storage). Its type is the coroutine type.
2. A new `Coroutine` expression, analogous to `Closure`, builds the
   coroutine from the call target and the arguments, and a `Store` puts it
   in the variable. The call target is the one a `Call` has, so the type is
   a `Coroutine(...)` for a known function and an
   [`InstanceCoroutine(...)`](#coroutines-of-unresolved-calls) for a call
   through an instance.
3. A `RefOf` borrows the variable mutably.
4. The reference is wrapped in a `Continuation[(), &mut c]`.

The borrow checker then sees an ordinary `&mut` loan of an ordinary
variable, and pinning needs no rule of its own:

- A continuation that escapes is "borrowed value does not live long enough".
- Moving the storage is a move out of a borrowed variable.
- The drop at the end of the scope is an ordinary `AddressDrop`. Its
  dictionary is the built-in `Drop` instance of the coroutine type, with one
  dictionary per argument. Memory analysis resolves them here, in the
  function that contains the `coro`, and reports a missing one at the
  `coro`.

The hidden variable belongs to the enclosing block scope, not to the
temporary scope of the statement. Otherwise the storage of
`let k = coro f()` would end with the statement.

### `handle`

A `handle` is a terminator too. Its clauses are
[branches](#clauses-are-branches), so they are its successors:

```
Handle { continuation, value, return, clauses, abandon }
```

- `continuation` and `value` are the operands. The continuation is moved.
- `return` is the block of the return clause. It starts with the value the
  coroutine returned.
- `clauses` has one block per operation clause. Each starts with the
  arguments of the operation and the next continuation `k`.
- `abandon` is present when the `handle` forwards effects.

No clause is a nested function, and there are no captures.

The forwarding loop of [lowering `handle`](#lowering-handle) is not written
out in the IR. One `Handle` means: resume the coroutine until it completes or
performs an operation that has a clause, and forward every other operation.
A `handle` that forwards is a suspension point of the enclosing function,
which is why it has an abandon edge.

Invoking a continuation, `k(v)`, is a `Handle` with no operation clauses.

### `run ... with` in the IR

A `run ... with` keeps its shape. It is still a `RunWith` expression, and its
body and handlers are still nested bodies with their own IR. Only two things
change:

- A `run` has an effect of its own when its body has an effect that is not
  handled, or when a handler performs one. Such a `run` is evaluated by a
  `Suspend`, like an effectful call.
- The nested bodies have suspension points of their own, like any other
  body.

Whether those bodies are compiled as coroutines is not visible in the IR. It
is decided when the IR is lowered; see
[`run ... with` in a coroutine](#run--with-in-a-coroutine).

On the abandon edge of a `run`, the handler captures are dropped as they are
when the `run` returns, after the body and the handler have been abandoned.

### What the analyses see

- **Memory analysis.** An abandon edge is an ordinary edge, and `Abandon` is
  an exit where nothing is initialised any more. The existing rule that
  balances every edge selects the drops, and their dictionaries are resolved
  with the solver of the body, as for a scope exit. A missing dictionary or
  a `@linear` value on an abandon edge is the error of
  [what a function may hold across a suspension](#what-a-function-may-hold-across-a-suspension),
  and it should say so.
- **Liveness.** A `Suspend` uses the operands of its expression and defines
  the value on the resume edge. The
  [typed saved locals](#compute-the-saved-locals-of-a-coroutine) are read
  off the liveness at these terminators.
- **Borrow checker.** Abandon edges are ordinary edges and the drops on them
  are ordinary uses. A `coro` is an ordinary borrow. A `Handle` moves the
  continuation, and each clause is one of its successors. `'this` adds a
  loan on the edge into a clause, and a check on the edges out of a
  `perform`; see [yielding local borrows](#yielding-local-borrows).
- **Verification.** Scopes have to balance on the abandon paths as well.
- **Lowering to MonoIR.** In direct form a `Suspend` evaluates its
  expression and jumps to `resume`, and the abandon blocks are unreachable
  and left out. In coroutine form every `Suspend` is a state.

### Not decided

- **Size.** Every suspension point adds a block. Suspension points that
  drop the same things could share one abandon block.

## MonoIR Update

**Drafting!**


# Borrow Checker Impact

`BORROW_CHECKER_PLAN.md` assumes every handler is tail-resumptive: "the
handler finishes the operation before the computation continues, so the
handler really is just an argument the computation calls". Its sections on
[effect variance](BORROW_CHECKER_PLAN.md#effect-variance) and
[handler captures](BORROW_CHECKER_PLAN.md#handler-captures) both rely on
this, and the plan already notes that they must be revisited together once
continuations are first-class. Coroutines are that case, so the borrow
checker rules have to be revised before this feature can be checked. What
needs a rule:

- **Regions held by a continuation.** A suspended coroutine holds its
  arguments and every local live at the suspension. The saved locals are not
  part of the coroutine type and their lifetimes are erased.
  [Lifetimes](#lifetimes) explains why the arguments of the coroutine type
  still account for every region that matters, and gives the rules for
  `c: 'a` and for dropping a coroutine. `Continuation[r, c]: 'a` then holds
  when `r: 'a` and `c: 'a`. In the first prototype `c` is a `&mut` to the
  [pinned storage](#pinned-storage), and the lifetime of that reference is
  what keeps the continuation from escaping.
- **Yielding borrows of the coroutine's own locals.** An operation argument
  may borrow a local of the function that performs it, through the built-in
  lifetime `'this`. The handling site has to be kept from using the borrow
  after the computation continues. See
  [yielding local borrows](#yielding-local-borrows).
- **Borrows held across a suspension.** A local that borrows another local of
  the same coroutine, and is still live at a `perform`, makes the coroutine
  self-referential. The first prototype can allow this, because the coroutine
  is pinned to the stack and never moves. It becomes a problem again as soon
  as a coroutine can be moved, returned or boxed; see moving and pinning
  under [open questions](#open-questions).
- **Effect variance.** The variance of effect parameters was derived for
  handlers that are called by the computation. It has to be re-derived for
  yielded arguments and resume values, together with the variance of `r` in
  `Continuation[r, c]`.

The clauses of a `handle` need no rule for captures. They are
[branches](#clauses-are-branches) of the enclosing function and have none,
so the rules for the handlers of `run ... with` do not apply to them. Their
one rule of their own is for `'this`, below.

## Yielding local borrows

Status: proposal, still being researched. Nothing else in this document
depends on it.

The parameter types of an operation may use one built-in lifetime, `'this`:

```ray
eff Log:
    def log(msg: &'this str) -> ()
```

`'this` is the lifetime of the suspension: it starts at the `perform` and
ends when the computation continues. It is not declared and nobody chooses
it. A lifetime parameter of the effect, as in `eff Log['a]`, is fixed by the
signature of the function that performs; `'this` is new for every `perform`.
So it can be a borrow of a local:

```ray
def worker() -> () \ {Log}:
    let msg = makeMessage()
    Log.log(msg.&)
```

It is the lifetime `'a` in the lending form of Rust's coroutine trait,
`resume(self: Pin<&'a mut Self>) -> Yield<'a>`: what is yielded is borrowed
from the coroutine until it is resumed again.

`'this` may not appear in the return type of an operation. A result is used
after the computation continues, and `'this` has ended by then.

### Why not a lifetime parameter on the operation

A declared parameter, as in `def log['a](msg: &'a str)`, is chosen by the
`perform`, and it can last longer than the suspension. That is what lets an
operation return a borrow of its argument, as in
`def get['a](key: &'a k) -> &'a v`. But then whoever handles the operation
does not know how long `'a` lasts, and needs a second set of rules for
everything it gives back as `'a`. `'this` has one meaning and needs one
rule on each side.

What is lost is an operation that returns a borrow of its argument. A
borrow that has to outlive the operation uses a lifetime parameter of the
effect, as today.

### At the `perform`

A `perform` is checked like a call, with a fresh region for `'this`. One
rule is added: **that region may not be live on any edge out of the
`Suspend`**, neither the resume edge nor the abandon edge.

The arguments themselves end at the `perform`, so this only bites when the
region is tied to something that lives on. That happens through an
invariant position, as in `slot: &'this mut &'this str`: the variable behind
`slot` then has `'this` in its type, and using it after the `perform` is an
error. This is what makes the handling side simple. Whatever a handler
stores through an argument, the computation cannot read it afterwards.

The region is live at the `Suspend`, so the loans in it are live for the
whole suspension. A local that is borrowed this way is a
[saved local](#compute-the-saved-locals-of-a-coroutine).

### In a `run` handler

The handler is a function, and `'this` is a universal region of it that
nobody in the function chooses. The lifetime of the handler's environment
is already such a region, and `'this` is checked the same way: a relation
the body requires of it is an error, never a requirement of the function
that creates the handler. The handler returns before the body continues, so
nothing more is needed.

### In a `handle` clause

In a clause, `'this` is an ordinary fresh region, created on the edge into
the clause. The arguments are treated as reborrowed through the
continuation. In the first prototype `k` is a
`Continuation[q, &'s mut C]`, where `'s` is the lifetime of the borrow of
the [pinned storage](#pinned-storage). On the edge into the clause, two
things are added, as for a reborrow `&'this mut *r` of a reference `r`:

- **A loan** of the coroutine behind `k`, with the region `'this`. A move of
  `k` conflicts with it, so `k` cannot be resumed or stored while something
  that came from an argument is live.
- **The constraint `'s: 'this`.** The loan of the storage flows into
  `'this`, so the storage cannot be dropped or reused while an argument is
  live.

Both are needed. The constraint alone keeps the storage alive but does not
stop `k` from being resumed: `k` is the `&'s mut` itself, and keeping `'s`
alive does not forbid using it. The loan alone stops the resume but lets the
storage die once `k` is gone.

```ray
case Log.log(msg) resume k:
    print(msg)
    con = k         # OK: `msg` is not used again
```

```ray
case Log.log(msg) resume k:
    con = k         # ERROR: `msg` is still in use
    print(msg)
```

When `c` is opaque, as in a generic driver that takes a
`Continuation[r, c]`, there is no `'s` to name. The loan is then of `k`
itself, and there is no constraint. The storage belongs to a caller and
outlives the function anyway. The cost is that an argument cannot outlive
the binding `k` there, even when the continuation is dropped and never
resumed.

Nothing is needed for what the clause gives back. The type of `k` never
mentions `'this`, because a return type cannot, so the resume value is
ordinary. And what the clause stores through an argument cannot be read by
the computation, by the rule at the `perform`.

A `handle` without a clause for the operation forwards it. The arguments
never appear in the handling function, so there is nothing to check.

# Open Questions

- **Moving and pinning.** Pinning every coroutine to the stack is a
  restriction of the first prototype. Returning a coroutine, boxing it and
  erasing its type all need a way to say that a value must not move once it
  has been resumed. The candidates, which do not exclude each other:
  - **`Pin`**, as in Rust: a wrapper type around a pointer. It needs no new
    kind of type, but brings pin projection and an API that leans on
    `unsafe`.
  - **A `Move` marker**: types that cannot be moved are known to the type
    system. This fits the existing markers such as `core.Copy`, but every
    generic parameter then needs a default bound and a way to opt out of it.
  - **In-place construction**: a value is built directly where it will live,
    so it never has to move there. Returning or boxing an immovable coroutine
    needs this under either of the other two.
- **Syntax of the clauses.** The clauses are spelled `case`, like the arms
  of a match, because they are branches and not functions: `return` inside
  one returns from the enclosing function, while inside a handler of
  `run ... with`, which is spelled `def`, it gives the result of the
  operation. Koka has the same split in one construct, as `fun` clauses,
  which answer in place, and `ctl` clauses, which get the continuation. That
  leaves room for a `def` clause inside `handle` later. Still open:
  - A clause gives the `handle` a value only in the `: expression` form,
    following `if`. That is a proposal, not a decision.
- **Lending through a generic effect.** [`'this`](#yielding-local-borrows)
  is only available in the declaration of an operation, so the effect fixes
  the shape of the borrow. `Yield[t]` cannot lend, because `t` is fixed by
  the signature of the performer. Allowing `'this` in a row, as in
  `{Yield[&'this int32]}`, would let a generic effect lend: the label then
  stands for a new lifetime at every `perform`. The handling site needs
  nothing new for it. The price is in rows: such a label is higher-ranked,
  and needs rules for when it matches an instantiated one, and for which
  parameters of an effect may take it, since an operation may not return
  `'this`.
- **Borrows returned by an operation.** With `'this`, an operation cannot
  return a borrow of its argument. A declared lifetime parameter on the
  operation would allow it, at the price described under
  [yielding local borrows](#why-not-a-lifetime-parameter-on-the-operation).
- **Syntax of `coro`.** In `handle expr1(expr2)` the outermost call is the
  resume, so `handle f(x)(y)` resumes the continuation `f(x)` with `y`.
  Undecided: whether `coro` accepts only a call or any expression.
- **Naming a coroutine type.** A bound on the coroutine of an unresolved
  call needs syntax. `coro d.m` in type position is the proposal; see
  [naming it in a bound](#naming-it-in-a-bound) for what is undecided.
- **Linear coroutines.** Because any coroutine may be abandoned, a function
  may not hold a `@linear` local across a suspension point, and needs a
  `Drop` given for a local of an opaque type
  ([details](#what-a-function-may-hold-across-a-suspension)). For the same
  reason `coro` needs `Drop` for its arguments. A coroutine
  that is itself linear must be run to completion, has no abandon edges, and
  would lift these restrictions. It needs a way to say which coroutines are
  linear.
- **Cost of reification.** `coro` turns every effect in the row into a
  suspension, including ones the driver only forwards. Each forwarded
  operation costs a suspend and a resume. It may be worth letting an effect
  stay a plain callback inside a coroutine. The clear case is an effect
  handled by a `run ... with` whose handlers do not suspend (case 2 of
  [`run ... with` in a coroutine](#run--with-in-a-coroutine)). It makes the
  compilation mode a property of each effect of a function, not of the
  function. Whatever is done here may change
  the layout of a coroutine but not
  [what counts as saved](#what-counts-as-saved) for the type system.
- **Rows in the lowering.** Labels of different effects commute, so
  `{State[int32], Print}` and `{Print, State[int32]}` need the same generated
  sum types. A row can also hold the same effect twice, for example
  `{State[int32], State[bool]}`, which needs variants per label and a rule
  for which label a clause handles. A callee with a smaller row than its
  caller needs its variants mapped into the caller's at each call.
- **Type erasure.** `Continuation[(), BoxedCoroutine]`, as mentioned under
  [continuations of different types](#continuations-of-different-types). It
  depends on moving and pinning above.

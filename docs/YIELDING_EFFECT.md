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

The document has three parts:

1. [Surface design](#surface-design): what the user writes and how it is
   typed.
2. [Lowering](#lowering): how an effectful function becomes a state machine.
   Nothing in this part is visible to the user.
3. [Borrow checker impact](#borrow-checker-impact) and
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
        handle con(())
        with Yield[int32]:
            def yield(value, k):
                sum += value
                con = k
        return x:
            break

    return sum
```

`coro numbers()` suspends `numbers` before it runs and gives back a
`Continuation[(), ...]`: a new coroutine is always resumed with the unit
value. Each `handle con(())` resumes the coroutine, and ends in one of two
ways:

- The coroutine performs `Yield.yield`. It suspends, and the `yield` clause
  runs with the yielded value and `k`, the continuation of the rest of the
  coroutine. `Yield.yield` returns `()`, so `k` is a `Continuation[(), ...]`
  too. It has the same type as `con` and can be stored back into it.
- The coroutine completes. The `return` clause runs with the value the
  coroutine returned, and leaves the loop.

The clauses are branches of `main`, like the arms of an `if`. That is why
`con = k` and `break` act on `main`'s own variable and loop, and why the move
analysis accepts the loop: `handle con(())` moves `con`, the `yield` branch
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
handle expr1(expr2)
with Label:
    def op(p1, ..., pn, k):
        ...
return x:
    ...
```

A `handle` resumes a coroutine and has two kinds of clauses:

- An **operation clause** runs when the coroutine performs that operation.
  It binds the operation's arguments and `k`, the continuation of the rest of
  the coroutine.
- The **return clause** runs when the coroutine completes. It binds the value
  the coroutine returned. It may be left out, which is the same as writing
  `return x: x`.

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

handle k(v)
with State[int32]:
    def get(k1):            # k1: Continuation[int32, c]
        ...
    def set(value, k2):     # k2: Continuation[(), c]
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
    return handle k(value)
    with Ask:
        def ask(next): supply(next, n, n + 1)
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

`coro f()` uses the coroutine form of `f`. A function called from a coroutine
has to be able to suspend too, so it is also used in its coroutine form.

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

`handle k(v) with Label: ...` is lowered in place, into the control-flow
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

## Recursion

A coroutine stores the state machine of the function it is currently calling.
A recursive function would therefore have to store itself, so recursion needs
an indirection. For the first version the compiler boxes the state machine
implicitly, rather than asking the user to do it.

This boxing is internal to the lowering. It is separate from letting the user
box a coroutine, which the first prototype does not support.

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
  arguments and every local live at the suspension. The coroutine type has to
  account for the regions in them, and `Continuation[r, c]: 'a` needs a
  definition. In the first prototype the continuation also carries the
  lifetime of its [pinned storage](#pinned-storage), which is what keeps it
  from escaping.
- **Yielding borrows of the coroutine's own locals.** In direct mode a
  `perform` may pass a borrow of a local to the handler, because the handler
  returns before the local goes away. In a coroutine the argument is handed
  to a clause together with the continuation, and resuming the continuation
  can end the life of the local it borrows from. **The first prototype
  disallows this**: an operation argument may not carry a lifetime local to
  the coroutine. How to allow it needs its own discussion.
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

The clauses of a `handle` need no rule of their own. They are
[branches](#clauses-are-branches) of the enclosing function and have no
captures, so the rules for the handlers of `run ... with` do not apply to
them.

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
- **Syntax of the clauses.** The semantics are settled, the spelling is not:
  - The return clause `return x:` sits at the indentation of the `handle`,
    where a `return` statement could also start. Only the colon after the
    name tells the two apart.
  - Operation clauses are written with `def`, like the handlers of
    `run ... with`, but they are branches and not functions. `return` inside
    one returns from the enclosing function, while inside a `run` handler it
    gives the result of the operation. A different spelling may be clearer.
  - A clause gives the `handle` a value only in the `: expression` form,
    following `if`. That is a proposal, not a decision.
- **Syntax of `handle` and `coro`.** `handle expr1(expr2)` reads as `handle`
  applied to a call. That matches "invoke the continuation, and intercept
  some effects", but the grammar has to say how `expr1` is delimited when it
  is itself a call, as in `handle f(x)(y)`. Also undecided: whether `coro`
  accepts only a call or any expression.
- **Indirect calls.** A coroutine that calls through a `def(...) -> ...`
  value or a `Def` dictionary does not know the callee's state machine type.
  This probably needs the same boxing as recursion, plus an erased `resume`.
- **Linear values.** A coroutine that is abandoned has its live locals
  dropped with its storage. If one of them is `@linear`, it has no `Drop`
  instance, so the coroutine presumably has none either and must be run to
  completion.
- **Cost of reification.** `coro` turns every effect in the row into a
  suspension, including ones the driver only forwards. Each forwarded
  operation costs a suspend and a resume. It may be worth letting an effect
  stay a plain callback inside a coroutine.
- **Rows in the lowering.** Labels of different effects commute, so
  `{State[int32], Print}` and `{Print, State[int32]}` need the same generated
  sum types. A row can also hold the same effect twice, for example
  `{State[int32], State[bool]}`, which needs variants per label and a rule
  for which label a clause handles. A callee with a smaller row than its
  caller needs its variants mapped into the caller's at each call.
- **Type erasure.** `Continuation[(), BoxedCoroutine]`, as mentioned under
  [continuations of different types](#continuations-of-different-types). It
  depends on moving and pinning above.

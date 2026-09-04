# Implicit instance resolution

Status: proposed design

## Summary

When a given argument is omitted in a function body, the typed-AST builder will
insert a fresh inference variable of `TyKind::Instance` and record an instance
obligation that relates that variable to the required `TraitRef`. Ordinary type
inference runs first. Once it reaches a fixed point, every obligation whose
trait reference contains no inference variables is resolved in two strictly
ordered tiers:

1. an exact matching given parameter in lexical scope; then
2. the unique most-specific viable global instance.

Rigid polymorphic variables are considered resolved. For example, `Eq[a]` can
be resolved inside a polymorphic function; `Eq[?0]` cannot be searched until
`?0` is solved or defaulted.

Global instances are treated as Horn clauses. An instance head is matched
against the requested trait reference, and its given parameters become
recursive instance obligations. Matching is transactional: substitutions made
while testing a candidate never enter the function's ordinary type solution.
Among candidates whose complete prerequisite trees resolve, specificity is a
partial order over their original heads. There is no declaration-order
tie-breaker.

The resolver must always terminate. It uses an active-goal stack to diagnose
exact cycles, memoization to avoid repeated work, and hard depth/work budgets as
a final safety boundary. A declaration-time structural-decrease check is the
recommended stable language rule for rejecting expanding cycles that never
repeat an identical goal.

## Goals

- Infer omitted given arguments only while building function bodies.
- Prefer a matching lexical dictionary over every global instance.
- Select a global instance only when it conforms to the requested trait and is
  the unique most-specific viable candidate.
- Recursively fill the selected instance's own given parameters.
- Permit rigid variables in goals and selected instance arguments.
- Produce deterministic results and useful diagnostics for missing, ambiguous,
  cyclic, underconstrained, and resource-exhausted searches.
- Preserve the existing representation consumed by monomorphization: an
  instance term is either `Ty::PolyVar` or a concrete `Ty::Application` whose
  view is `ApplicationView::Instance`.

## Non-goals for the first implementation

- Resolving omitted given arguments in declarations, signatures, or other
  semantic-element queries.
- Using instance selection to infer an otherwise unknown ordinary type. Search
  only consumes an already-ground goal.
- Automatically turning a direct abstract call such as
  `Eq[int32].equals(...)` into an instance call. This proposal fills given
  argument slots; it does not change trait-member path semantics.
- Higher-rank, negative, or associated-type constraints.
- Silently choosing between incomparable or alpha-equivalent instances.

## Existing compiler fit

The necessary representations largely already exist:

- `PolyVarKind::Instance(TraitRef)` records the requirement of a given
  parameter in `compiler/semantic/type/src/poly_var.rs`.
- `Ty::Inference` already carries a `TyKind`, including `TyKind::Instance`.
- `Ty::new_instance` creates the concrete dictionary type expected by typed AST,
  IR, and `mono_ir_builder`.
- `PolyVarStack` is ordered from the current symbol outwards and is therefore
  the right source for lexical dictionaries.
- The typed-AST builder already owns ordinary inference, substitutions,
  provenance, and the final substitution of the AST.
- `get_instance_trait_ref` supplies an instance declaration's head.

There are two important gaps:

1. `resolution::resolve_arguments` reduces polymorphic parameters to
   `(name, TyKind)`. A missing given slot needs the full `PolyVar`, especially
   its `TraitRef`, plus the parameter's global ID.
2. A missing given argument is currently replaced immediately by an error type
   and a `MissingGivenArgument` diagnostic. Body resolution needs a capability
   that creates an obligation instead, while all non-body callers retain the
   current error.

## Semantic model

### Instance terms and obligations

An instance term is an `Interned<Ty>` of instance kind:

- `Ty::PolyVar(id)` for a lexically supplied dictionary; or
- `Ty::Application(Instance(symbol_id), args)` for a selected global instance.

Omission creates a constraint-like obligation:

```text
ResolveInstance {
    output: ?dict : Instance,
    required: TraitRef,
    origin: { span, called_symbol, given_parameter },
}
```

On success, resolution contributes the substitution `?dict := instance_term`.
On failure, it contributes `?dict := Error(Instance)` and a diagnostic, so later
compiler phases do not encounter an unbound dictionary inference.

This should be modeled as a first-class obligation, but initially it should not
be another case handled directly by `rayc_type::Solver::entail`:

- structural type solving is synchronous and independent of name/symbol scope;
- instance search needs asynchronous semantic queries, the call site's lexical
  environment, recursive search state, and richer failure information; and
- candidate matching must be speculative and isolated from the main
  substitution.

Keep `PendingInstanceObligation` alongside the typed-AST builder's constraint
state. It participates in the same final substitution and diagnostic
provenance, but is discharged by an `InstanceResolver`. If the solver is later
generalized around asynchronous/domain-specific constraints, this obligation
can become a `Constraint` variant without changing its language semantics.

Do not encode the required `TraitRef` inside a new `Ty` variant. A normal
instance-kind inference variable plus an explicit obligation keeps type syntax,
substitution, and interning simple and makes the relationship inspectable.

### Groundness

A trait reference is ready for search when, after applying the current ordinary
substitution and fully normalizing it, none of its arguments recursively contain
`Ty::Inference`.

`Ty::PolyVar` is rigid, not unknown, and is allowed. These goals are therefore
ready:

```text
Eq[int32]
Eq[(a, int32)]
Eq[(a, a)]
```

This goal is not ready:

```text
Eq[(?0, int32)]
```

The initial implementation should resolve instance obligations after ordinary
constraints have reached a fixed point and numeric defaulting has run. This is
semantically equivalent to waking each obligation as soon as it becomes ground,
because instance selection is deliberately forbidden from solving ordinary
inference variables. It also avoids making the current synchronous constraint
worklist asynchronous.

An obligation still containing inference variables at finalization produces an
"cannot infer instance requirement" diagnostic. It must not be treated as "no
instance found", because search was never well-defined.

### Normal form

Before groundness checks, equality, memoization, cycle checks, or matching:

1. apply the latest ordinary substitution;
2. repeatedly call `Reduce` until no reduction is possible; and
3. intern the resulting components through the normal engine APIs.

Add shared `normalize_ty` and `normalize_trait_ref` helpers rather than
duplicating fixed-point reduction loops. Normalization must terminate
independently of instance search. If reducible type aliases are added later,
alias-cycle protection belongs in this layer.

Canonical goal identity preserves the IDs of caller-owned rigid variables:
`Eq[a]` and `Eq[b]` are different goals when `a` and `b` are different rigid
variables. Candidate-head comparison separately alpha-normalizes
declaration-owned variables by first occurrence.

## Creating obligations at omitted arguments

Argument resolution should operate over the ordered `PolyVarMap`, not a vector
of names and kinds. It builds the call substitution from left to right:

1. Resolve or infer every ordinary type/effect argument and add its mapping.
2. For each instance-kind parameter, apply the mappings already built to that
   parameter's `TraitRef`.
3. If the given argument was explicit, use its resolved instance term.
4. If it was omitted and the resolver has the body-only implicit-instance
   capability, create `?dict : Instance`, enqueue
   `ResolveInstance(?dict, substituted_requirement)`, and place `?dict` in the
   argument list.
5. If it was omitted without that capability, preserve today's
   `MissingGivenArgument` behavior.

This supports both wholly omitted lists and holes left by partial named given
arguments. Explicit arguments always win for their slots and never trigger
search.

The capability boundary is preferable to checking a symbol kind or a global
mode flag. Only the `Resolver` constructed by `TAstBuilder::resolve_path` (and
other path resolution that genuinely occurs within that body) receives an
implicit-instance sink. Resolvers used for signatures, traits, instances, and
semantic elements do not.

Because the current resolver borrows the same constraint solver to generate
ordinary inferences, expose one combined body-inference interface rather than
two simultaneous mutable trait-object borrows. Conceptually it provides:

```text
new_inference(kind, constraint) -> Inference
new_implicit_instance(required, origin) -> Interned<Ty>
```

## Resolution API and state

The public-looking operation should return structured failure rather than
`Option`:

```text
async resolve_instance(
    required: CanonicalTraitRef,
    context: &mut InstanceResolutionContext,
) -> Result<Interned<Ty>, InstanceResolutionError>
```

`Option` cannot distinguish no candidate from ambiguity, a cycle, an
underconstrained goal, or a search limit. The context contains:

- the call-site `PolyVarStack` with scope depth retained;
- the resolution universe (the current target plus linked targets);
- an active stack of canonical goals and the instance edges that introduced
  them;
- a memo table for completed searches;
- depth and candidate-visit budgets; and
- diagnostic trace data.

Nested prerequisites use the same context and the same caller lexical
environment. This is important for the standard pattern where a global
container instance consumes a dictionary supplied by the enclosing function.

## Resolution algorithm

At a high level:

```text
resolve(goal):
    goal = normalize(goal)
    reject if goal contains inference variables
    return memoized success when present
    reject with Cycle if goal is already active
    reject with SearchLimit if a budget is exhausted

    push goal onto active stack

    lexical = exact lexical matches at the nearest matching scope depth
    if lexical has one member:
        pop goal and return Ty::PolyVar(member)
    if lexical has multiple members:
        pop goal and return AmbiguousLexical

    matched = globally visible instance heads that match goal
    viable = []
    for candidate in matched in stable symbol-ID order:
        match the candidate head transactionally
        reject the candidate if any ordinary declaration variable is undetermined
        recursively resolve every instantiated given requirement
        if all prerequisites resolve:
            construct the full instance application and add it to viable

    maxima = candidates in viable for which no other viable candidate is
             strictly more specific

    result = structured failure  if maxima is empty
             the instance term   if maxima has exactly one member
             ambiguity           otherwise

    pop goal, memoize context-independent completion, and return result
```

Lexical and global lookup are precedence tiers, not one combined candidate
set. A lexical match prevents global lookup. Within lexical lookup, search maps
from the innermost scope outward. One match at the nearest matching depth wins;
multiple matches at that same depth are ambiguous. This provides normal lexical
shadowing without relying on parameter declaration order.

When no candidate is viable, preserve why matched candidates failed. If every
possible proof is cyclic, report a cycle; if a nested prerequisite is missing,
report the root goal with that failed prerequisite; and if a budget was reached,
report resource exhaustion. Reserve plain "no implicit instance" for searches
with no more informative failure. This failure aggregation should be bounded so
diagnostics cannot grow with the full search tree.

### Global instance index and resolution universe

Add an incremental query indexed by trait ID, for example:

```text
VisibleInstances { from_target, trait_id } -> [GlobalSymbolID]
```

The implementation may initially build this from `get_all_symbol_ids` for the
current and linked targets, filtering `SymbolKind::Instance` and
`get_instance_trait_ref`. The result must be sorted by stable global symbol ID.
Do not scan every symbol independently at every obligation.

"Global" means the closed universe linked into this compilation, not every
instance installed on the machine. When module visibility is implemented, the
query must filter by visibility as well. This closed-world choice should be
documented: adding a linked package can introduce a more-specific instance or
turn a formerly unique result into an ambiguity. Orphan/coherence rules can
further restrict the universe later; they are not replaced by specificity.

Malformed instance heads (`get_instance_trait_ref == None`) are excluded. Their
declaration diagnostics remain the primary error.

## Candidate matching and conformance

Suppose an instance declaration has ordinary variables `a...`, given parameters
`d...`, head `H`, and premise trait references `P...`.

1. Replace every declaration-owned ordinary type/effect variable with a fresh,
   candidate-local metavariable. Do not replace caller-owned rigid variables.
2. Require the candidate and goal to name the same trait.
3. Equality-match the instantiated head arguments against the ground goal and
   solve the resulting structural constraints exhaustively.
4. Reject on conflict, occurs-check failure, or any residual matching
   constraint.
5. Require every declaration-owned ordinary variable to have a mapping. All of
   them occupy instance-application argument slots even if the source variable
   is absent from the head. A mapping whose result still contains a
   candidate-local metavariable is not determined.
6. Apply the mapping to each premise `P` and recursively resolve it.
7. Build the final `Ty::new_instance(instance_id, args)` in the instance's
   `PolyVarMap` order. Ordinary slots contain matched types/effects and instance
   slots contain recursively resolved dictionary terms.

The existing `Subtype` machinery currently implements equality-like structural
unification despite its name, so it can be reused behind an isolated helper.
However, candidate matching must be transactional:

- candidate-local metavariables need IDs that cannot collide with main solver
  inferences;
- only the winning instance term is committed to the main substitution; and
- failed candidate substitutions and diagnostics do not leak.

A dedicated one-way/head matching helper is preferable in the long term. It
also directly supports specificity checks and makes it impossible to bind a
caller rigid variable accidentally.

### Range restriction

The draft's distinction should be stated in terms of parameter kinds:

- Every ordinary type/effect variable declared by an instance must be
  determined by matching the instance head.
- Instance-kind given parameters are intentionally not determined by head
  matching; their `TraitRef`s are instantiated and recursively resolved.

Thus this is invalid or, defensively, never a viable candidate:

```text
inst Something[a, b] for Eq[a]   // b is not determined by the head
```

Whereas this is valid in principle:

```text
inst Something[a] for Eq[a] given (f: Eq[int32])
```

Here `a` is determined by the head and `f` is resolved recursively. Add a
declaration-time "instance parameter is not determined by its head" diagnostic
so this is reported at the bad declaration, with the candidate-side check kept
as a defensive invariant.

### Explicit argument conformance

Implicit search must only produce conforming dictionaries. There is an adjacent
existing issue: resolving an explicit given argument currently checks that it
has instance kind, but does not appear to prove that its implemented trait
matches the required `TraitRef`.

The reusable conformance operation introduced for candidate heads should also
be used for explicit given arguments, either in the same change or a tightly
following change. It is not acceptable for implicit inference to be sound while
an explicitly supplied dictionary of an unrelated trait is accepted. Explicit
arguments still bypass search and specificity.

## Specificity

Specificity is a partial order on original, normalized instance-head patterns,
not on heads after matching the requested goal. After goal matching, every
candidate head would look equal to the goal and the useful structure would be
lost.

Let `A` and `B` be candidate heads. `A` is strictly more specific than `B` when:

1. `B` can be one-way instantiated to `A`; and
2. `A` cannot be one-way instantiated to `B`.

During each direction, variables belonging to the pattern side may bind and
variables belonging to the target side are rigid. Alpha-rename each
declaration's variables before comparison.

This produces the intended chain:

```text
Eq[(a, b)]  <  Eq[(a, a)]  <  Eq[(int32, int32)]
```

It also handles the cases a score cannot:

- `Eq[(int32, a)]` and `Eq[(a, bool)]` are incomparable.
- Heads that differ only by variable names are equivalent, not ordered.
- Repetition of the same variable is more specific than two independent
  variables.

Rank only candidates whose entire prerequisite tree is viable, as proposed in
the draft. Compute the maximal elements. Exactly one maximal element wins;
zero means no instance; two or more means ambiguity. Symbol order is used only
for deterministic work and diagnostics, never as a semantic tie-breaker.

## Cycles and termination

### Exact cycles

Keep canonical goals on an active stack. Re-entering an active goal fails that
candidate branch immediately and records the cycle path, including the
instances whose premises introduced each edge.

For example:

```text
inst Loop[a] for Eq[a] given (again: Eq[a])
```

Resolving `Eq[int32]` re-enters the same canonical goal. The diagnostic should
show the requested goal, `Loop`, and its `again` premise rather than merely
saying that no instance exists.

Cycle failure is branch-local. Other global candidates for the same goal may
still succeed. Do not cache a cycle-dependent failure as a universal
`NoInstance`, because the active ancestry is part of that failure.

### Expanding cycles

Exact-goal detection alone is insufficient:

```text
inst Grow[a] for Eq[a] given (next: Eq[(a, a)])
```

This produces a different, larger goal at every step. Therefore every search
also has hard safety limits. Start with named constants such as a maximum depth
of 64 and a maximum of 4096 candidate visits per root obligation. Exceeding a
limit produces a dedicated "instance resolution limit exceeded" diagnostic,
including the recent goal chain. It must not be reported as no matching
instance.

Budgets guarantee that malformed programs cannot hang the compiler, but they
are not a satisfying final language rule because a sufficiently large valid
proof can hit an arbitrary limit.

### Recommended stable termination rule

Before stabilizing recursive instances, add a declaration-time termination
check analogous to the Paterson conditions:

- Build a trait-dependency graph from an instance head trait to the traits of
  its given parameters.
- For premise edges within the same strongly connected component, require all
  premise variables to occur in the head, require no variable to occur more
  often in the premise than in the head, and require the premise's structural
  constructor size to be strictly smaller than the head's.
- Edges between different acyclic components do not require a size decrease.

The conventional container instance then passes:

```text
inst ListEq[a] for Eq[List[a]] given (element: Eq[a])
```

while direct and expanding recursive instances fail at their declarations. The
runtime active stack and budgets remain as defense in depth even after this
static rule exists.

If Ray intentionally wants to accept programs outside a conservative
structural-decrease rule, that should be an explicit future relaxation (for
example, a checked annotation), not an accidental reliance on a larger search
budget.

### Memoization

Memoize successful canonical goals for the duration of one typed-AST build.
Definitive, ancestry-independent no-candidate results may also be memoized.
Do not globally memoize cycle- or limit-dependent failures. A memoized result is
valid only for the same lexical environment and global resolution universe;
keeping the cache inside `InstanceResolutionContext` establishes that
invariant naturally.

## Finalization order

`TAstBuilder::finish` needs to become asynchronous because global instance
lookup uses semantic queries. Its order should be:

1. validate lvalue requirements;
2. exhaust ordinary subtype/effect constraints;
3. produce ordinary constraint diagnostics and numeric defaults;
4. retain the resulting ordinary substitution;
5. canonicalize and discharge all recorded root instance obligations;
6. compose successful dictionary bindings (and error bindings for failed
   obligations) into the final substitution;
7. apply the final substitution to the complete `TypedFunctionMap`; and
8. return the typed AST and all diagnostics.

If a normalized requirement contains an existing error type from an earlier
failure, bind its dictionary output to `Error(Instance)` without starting
search or emitting a second instance diagnostic. This follows the compiler's
usual error-recovery rule and avoids cascades.

Resolve root obligations in stable source order for deterministic diagnostics.
Candidate recursion constructs its dictionary tree synchronously within that
root search. Since root requirements are ground and do not constrain ordinary
types, resolving one root must not change another root's requirement except by
replacing its own instance-kind output variable.

The existing concrete instance representation already stores recursive
dictionaries naturally in its argument list. For example:

```text
ListEq[int32, EqInt]
```

is the complete resolution tree for an `Eq[List[int32]]` request. No new typed
AST node is required.

## Diagnostics

Add typed-AST diagnostics with the omission site as the primary span:

- **Cannot infer instance requirement**: the final normalized `TraitRef` still
  contains ordinary inference variables. Show those variables and the given
  parameter that introduced the obligation.
- **No implicit instance**: no lexical match and no viable global candidate.
  Show the required trait reference.
- **Ambiguous lexical instances**: multiple exact dictionaries occur at the
  nearest matching lexical depth. Show their declarations.
- **Ambiguous global instances**: multiple maximal viable heads remain. List
  their qualified names, heads, and source spans in stable order.
- **Cyclic instance resolution**: show the minimal repeated-goal chain and the
  instance/premise edges.
- **Instance resolution limit exceeded**: distinguish resource exhaustion from
  semantic failure and show the recent chain.
- **Instance variable not determined by head**: declaration-site diagnostic for
  range-restriction violations.
- **Instance context does not decrease**: declaration-site diagnostic for the
  stable termination rule.

Candidate mismatches are normally internal rejection reasons, not one
diagnostic per candidate. If no candidate is viable, attach a short bounded list
of the closest rejected candidates and reasons as notes only when useful. This
prevents an exponential recursive search from producing an exponential
diagnostic.

## Concrete component plan

### 1. Preserve parameter requirements during path resolution

- Change the resolver's argument-parameter API to expose ordered global
  polymorphic parameter IDs and `PolyVar` data rather than `(name, TyKind)`.
- Build a partial call substitution while processing parameters.
- Add substitution and full-normalization implementations/helpers for
  `TraitRef`.
- Keep omission errors unchanged when no body inference capability is present.

Likely files:

- `compiler/semantic/resolution/src/resolver.rs`
- `compiler/semantic/resolution/src/ty.rs`
- `compiler/semantic/type/src/poly_var.rs`
- `compiler/semantic/type/src/trait_ref.rs`
- `compiler/semantic/type/src/reduce.rs`

### 2. Record and finalize body obligations

- Add `PendingInstanceObligation`, its origin, and storage to the typed-AST
  constraint state.
- Let the body resolver create an instance inference and record the obligation.
- Refactor ordinary constraint finalization so numeric defaulting occurs before
  instance groundness checks.
- Make `TAstBuilder::finish` async and compose instance bindings before applying
  the final AST substitution.

Likely files:

- `compiler/semantic/typed_ast_builder/src/tast_builder.rs`
- `compiler/semantic/typed_ast_builder/src/query.rs`
- `compiler/semantic/typed_ast_builder/src/tast_builder/constraint_solver/*`
- `compiler/semantic/typed_ast_builder/src/diagnostic.rs`

### 3. Add indexed instance search

- Add `VisibleInstances` (or an equivalent per-trait index) as a semantic query.
- Implement `InstanceResolutionContext`, lexical exact lookup, isolated head
  matching, recursive premise resolution, memoization, and budgets.
- Construct `Ty::new_instance` arguments in declaration order.

This logic should live in a focused instance-resolution module, not in
`expression/call.rs`; omitted givens also occur on non-call paths such as effect
arguments.

### 4. Add specificity and declaration checks

- Implement bidirectional one-way head matching and maximal-candidate
  selection.
- Add declaration-time range restriction.
- Add the SCC-based decreasing-context check before recursive instances are a
  stable feature.
- Reuse head conformance to validate explicit given arguments.

## Test strategy

The observable language behavior belongs primarily in
`compiler/e2e/test/check/`. Use small fixtures that cover:

- one omitted given resolved globally;
- a partially named given list whose remaining slots are inferred;
- lexical dictionary preference over a conforming global instance;
- nearest-scope lexical shadowing and same-scope lexical ambiguity;
- a rigid goal such as `Eq[(a, int32)]` resolved through a local or generic
  instance;
- a goal initially containing inference, then grounded by an ordinary call
  argument;
- a goal grounded only by numeric defaulting;
- a final underconstrained goal;
- recursively constructed dictionaries such as `ListEq[int32, EqInt]`;
- a candidate with an unsatisfied premise falling back to another viable head,
  matching the draft's viability-before-ranking rule;
- the three-level specificity chain and selection of its unique maximum;
- incomparable, alpha-equivalent, and duplicate-head ambiguities;
- a declaration variable absent from its head;
- direct and mutual exact cycles;
- an expanding cycle stopped by the termination check or safety budget;
- stable results independent of declaration order; and
- omission outside a function body retaining the current missing-given error.

Use `compiler/e2e/test/run/` only where the selected dictionary must be proven by
runtime behavior rather than typed-AST acceptance. Focused unit tests are also
justified for the stable algorithmic contracts that are awkward to express in
Ray source: one-way matching, the specificity partial order, maximal-element
selection, and canonical active-goal detection.

Before landing implementation changes, run the narrow e2e target, relevant unit
tests, `cargo +nightly fmt`, and `cargo clippy` as required by the project.

## Acceptance criteria

The first usable implementation is complete when:

- omitted given arguments in bodies become instance obligations rather than
  immediate missing-argument errors;
- all such obligations are either replaced by a lexical/concrete instance term
  or by an instance-kind error type with a specific diagnostic;
- global candidates match in isolation, resolve all prerequisites, satisfy
  range restriction, and construct complete ordered instance arguments;
- the unique maximal viable head wins and every non-unique maximum is an error;
- exact and expanding recursive programs cannot hang the compiler;
- resolution and diagnostics are deterministic across declaration/table
  iteration order; and
- typed AST, semantic IR, and monomorphization never observe an unbound
  instance inference after successful type checking.

## Decisions to retain explicitly

These choices are semantically significant and should not be left to incidental
implementation behavior:

- lexical instances strictly outrank global instances;
- only ground goals participate in search;
- rigid polymorphic variables count as ground;
- candidate prerequisites determine viability before head specificity is
  ranked;
- specificity is a partial order over original heads, not a numeric score;
- ambiguity is an error, never resolved by source or symbol order;
- recursive cycle failure is branch-local; and
- search-limit exhaustion is a distinct compiler diagnostic, not "no instance".

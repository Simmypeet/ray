---
name: test-strategy
description: Choose appropriate test coverage for changes to the Ray compiler. Use when implementing or fixing behavior, deciding whether to add tests, or reviewing proposed tests. Prefer observable end-to-end coverage, avoid implementation-coupled unit tests, and allow no new test when a change has no meaningful observable contract.
---

# Test Strategy

Treat tests as protection for behavior that programmers or Ray users care about,
not as a requirement to exercise every changed line.

## Choose the Test Boundary

Before adding a test, state the behavior it protects and who can observe it. Then
choose the broadest stable boundary that makes failures understandable:

1. Follow a programmer's explicit request for a particular test or test level.
2. For user-observable compiler or language behavior, prefer an end-to-end fixture.
3. Use a unit test only for a valuable internal contract that cannot be covered
   clearly or practically through the end-to-end infrastructure.
4. Add no new test when the change is only an implementation detail and there is
   no meaningful behavior or stable invariant to protect. Run relevant existing
   tests instead.

Do not add tests merely to increase coverage, mirror the implementation, or prove
that trivial code executes.

## Prefer End-to-End Tests

Use `compiler/e2e/test/check/` for observable compiler behavior such as:

- accepting or rejecting a Ray program;
- diagnostics, source spans, and other compiler output;
- a new language construct or a change to its static semantics;
- regressions that can be expressed as a Ray source file without executing the
  resulting program.

Use `compiler/e2e/test/run/` when the contract is observable only after compiling
and running a Ray program, including stdout, stderr, or its exit code.

Follow a nearby fixture's directory layout. Each case contains `main.ray` and an
Insta `snapshot.snap`. Run the narrowest applicable target:

```sh
cargo test -p rayc_e2e --test check_e2e
cargo test -p rayc_e2e --test run_e2e
```

Prefer a small source program and its externally visible result over assertions
against compiler data structures or pipeline steps.

## Unit-Test Gate

A unit test is justified when the unit owns a non-trivial, stable contract and a
focused test gives materially better coverage or fault localization than an
end-to-end fixture. Typical candidates include critical algorithms and invariants
in the type system, solver, IR validation, interning, or similarly foundational
modules.

Do not add unit tests for:

- getters, setters, constructors, field storage, or straightforward delegation;
- formatting or printing that is already observable in end-to-end output;
- panic-only checks unless the panic itself is an intentional, stable contract;
- private call sequences, intermediate representations, incidental ordering, or
  other details likely to change during refactoring;
- behavior already covered more clearly by an end-to-end fixture.

When a unit test passes this gate, keep it focused on the stable contract and
follow the `write-unit-tests` skill and the conventions of the surrounding crate.

## Review Existing or Proposed Tests

Ask whether the test would remain valid after a behavior-preserving refactor. If
not, replace it with an observable assertion, move it to an end-to-end fixture,
or omit it when it protects no meaningful contract. Do not preserve a low-value
unit test merely because test code has already been written.

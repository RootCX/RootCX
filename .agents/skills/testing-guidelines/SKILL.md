---
name: testing-guidelines
description: "Strict testing guidelines. Use when writing, reviewing, or refactoring tests. Triggers on test, #[test], #[cfg(test)], mod tests, test coverage, unit test, integration test, spec, describe, it(."
---

# Testing Guidelines

> A test exists to catch a **future real bug**. If it can't, delete it.

## The Only Question

Before writing any test, ask: **"What realistic mistake would make this fail?"**

If the answer is "a compiler/framework/dependency bug" — don't write it.
If the answer is "nothing, really" — don't write it.

## Rules

**1. Never test the framework.** You are not QA for your dependencies. Serde roundtrips on derived structs, stdlib parsers accepting valid input, ORM query builders — these test someone else's code.

**2. Test decisions, not plumbing.** Worth testing: branching logic, computations, validation, security boundaries, state transitions, protocol contracts. Not worth testing: struct construction, static lookups, trivial wrappers, field mappings the type system already guards.

**3. Parameterize, don't duplicate.** N tests with identical structure and different inputs = 1 test with a loop. Always include the input in the failure message so you know which case broke.

**4. One test, one behavior.** A test verifies one property. Multiple assertions are fine if they describe facets of that single property. A test named `everything_works` is a smell.

**5. Test the sad path.** Happy-path-only is half a test. Every validator needs: empty/zero, malformed, and boundary inputs.

**6. Integration tests test boundaries.** They earn their boot cost by verifying what unit tests structurally cannot: the real path across process/network/DB boundaries. Never duplicate unit-level assertions in integration tests.

**7. Helpers set up, assertions stay visible.** Test helpers reduce boilerplate. They must never hide what is being verified. The reader sees the assert in the test body, always.

## Litmus Test

- Would a real bug in my code make this fail? → keep
- Would only a dep/compiler bug make this fail? → delete
- Is there another test that already covers this? → delete
- Does this survive a behavior-preserving refactor? → keep
- Are there 3+ tests with the same shape? → parameterize into 1

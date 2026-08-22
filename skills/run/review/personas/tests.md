# Tests

You evaluate whether the tests in the diff actually prove the code works — not whether tests
exist. A test that calls the function and asserts it doesn't panic is worse than no test, because
it signals coverage it doesn't provide.

**Fires always, except T0.** The design's exception is docs-only or format-only T0 diffs;
production-file presence alone never fires it on its own — a T0 diff with a real behavior change
still gets Correctness's single pass, and Tests joins from T1 up. No justification line is needed
when it fires by default at T1+; state one only if you were asked to evaluate a genuinely marginal
case.

## What you read

The diff, the relevant plan units (including each unit's declared test-file paths, per the Plan
stage's own contract), and this file.

## Checklist

- **TST-1 — New branch coverage.** Every new `if`/`match` arm the diff introduces has at least one
  test that exercises it — not merely a test file that touches the same function.
- **TST-2 — Behavioral change without a test.** The diff changes observable behavior (a returned
  value, a written file's shape, a CLI's output, a new error path) but adds or modifies zero test
  files. Formatting, comments, and doc-only changes are excluded from this check.
- **TST-3 — Literals-tested pure functions.** A new or changed pure function in `decide`, `policy`,
  `job`, `observe`, or `rung` (ADR-0007's testable-from-literals discipline) has a test constructed
  from literal inputs, not only an integration-style path that happens to exercise it indirectly.
- **TST-4 — False-confidence assertions.** A test calls the code under test but only asserts it
  doesn't panic, asserts a type instead of a value, or mocks so heavily it verifies the mock rather
  than the code.
- **TST-5 — Fixture drift.** A test relying on a checked-in fixture (`tests/fixtures/run2`, the
  enqueue template's example table) is checked for whether the diff changed the real thing the
  fixture stands in for without updating the fixture to match.
- **TST-6 — Safety-property carrier coverage.** A change touching a documented safety property —
  `DENIED_TOOLS`, the Wait predicate (`Attempt::is_wait`), the sole-writer rule, a prohibited-shape
  rule from ADR-0006 — is checked for whether its existing carrier test (`tests/topology.rs`, a
  compile-fail test, a literals test) still exercises the changed code, or whether the diff narrowed
  what that test actually covers.
- **TST-7 — Error-path coverage.** New error-handling code (a new `Blocker` reason, a new
  fail-closed default, a new suppressed-confidence branch) has a test that drives that path, not
  only the happy path alongside it.

## What you don't flag

- Missing tests for trivial getters/accessors with no logic.
- Test style preferences — assertion macro choice, file layout, naming conventions.
- Missing tests for pre-existing, untouched code the diff didn't make riskier.

## Confidence

Anchor **100** — verifiable from the diff alone: a new public function with no test at all, an
assertion referencing a removed symbol. Anchor **75** — a new branch with no corresponding test
case, provable from the diff. Anchor **50** — coverage is inferred from file naming or structure,
not confirmed; write only at P0/P1 or route to a residual-risk note per the Validate stage's
demotion rule for weak Tests findings. **Below 50: suppress.**

## What you write

`<stages-dir>/review/tests/findings.json`, `rule_id` from `TST-1`..`TST-7`. Empty array if nothing
survives confidence 50. Touch nothing.

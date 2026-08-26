---
date: 2026-08-26
run: 20260826-055920-grind-154
paths: [src/decide.rs]
statement: When a conjunction feeds a predicate that can already be decided by its first conjunct, tests that only exercise the first-false shape leave every later conjunct unprobed — a regression dropping a later arm compiles and stays green until a test drives each remaining conjunct to its failing value on its own.
status: candidate
---

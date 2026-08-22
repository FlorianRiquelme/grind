---
date: 2026-08-22
run: run-107
paths: [src/world.rs]
statement: A test reading a subprocess result must not skip via `let Some(x) = ... else { return }` on the value under test — that guard turns a real regression's None into a silent pass; gate the skip on the narrower precondition (can the binary spawn at all) and assert unconditionally once it runs.
status: candidate
---

---
date: 2026-08-27
run: 20260827-063907-grind-178
paths: [src/cli.rs]
statement: When an arm's underlying reading is inherently volatile — two live `df` spawns never share byte-identical figures — pin its composition at the callsite by reproducing it independently and asserting verdict agreement plus small numeric drift, never rendered bytes.
status: candidate
---

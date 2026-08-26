---
date: 2026-08-26
run: 20260826-055920-grind-154
paths: [src/decide.rs]
statement: A fold that special-cases one row of a labelled tuple must dispatch on a typed marker carried in the row, never on the row's display string — the label serves rendering into output, and letting it double as the control-flow key means a rename silently reverts behaviour with no compile error anywhere.
status: candidate
---

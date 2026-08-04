# Findings: transcript parsing (wayfinder #33)

Question answered: this is the one place a compiled Rust base is expected to be
*worse* than the Python it replaces. How much worse, measured.

Setup: `src/main.rs` implements the same extraction ("now" skill from
`attributionSkill`, and progress mtime across a fan-out) three ways — (a) strict
derive, (b) derive with `Option`+`#[serde(default)]`, (c) hand-rolled
`serde_json::Value` lookups — run against the same real transcripts and a set of
deliberately damaged copies under `fixtures/`. Run with `cargo run -p transcript`.

## What the real files actually look like

`~/.claude/projects/` has 204 project directories on this machine, 265 transcript
files containing `attributionSkill`. One file
(`fixtures/real-parent-heterogeneous.jsonl`, copied read-only from a real
session) confirms the ground truth given in the ticket and extends it:

- 316 lines, **44 distinct top-level keys**, no line has all of them.
- `sessionId` (300 lines) and `session_id` (190 lines) both occur; wherever both
  are present on the same line, they always agree — but a struct that only
  declares one of the two spellings silently drops the other 100+ lines' worth
  of session identity.
- `attributionSkill` appears on 75 of 316 lines — i.e. on the *majority* of
  lines it is simply absent. It is always a string when present (checked across
  every real file with the field, no exceptions found) — so the ticket's
  "type change" risk had to be constructed, not found; see below.

## Fan-out: how subagent activity is actually represented

Contrary to the ticket's lead ("there is an `isSidechain` field... there may
also be separate `.jsonl` files") — **it is the separate files, not the
field.** Concretely, empirically, on this machine:

- `isSidechain` appears on 88,514 lines across every `.jsonl` under
  `~/.claude/projects/`. **Every single one of them is `false`.** Zero lines
  with `isSidechain: true` exist anywhere in a parent transcript.
- Subagent transcripts live in a **sibling directory named after the parent
  session's UUID**, one level down: for parent `<dir>/<uuid>.jsonl`, its
  fan-out (if any) is at `<dir>/<uuid>/subagents/agent-<id>.jsonl`, one file per
  subagent, each paired with an `agent-<id>.meta.json` (`agentType`,
  `description`, `toolUseId`, `spawnDepth`, `model`). *Inside* those
  `subagents/*.jsonl` files, `isSidechain: true` does appear on every line.
  110 such `subagents/` directories exist on this machine.
- So `isSidechain` is real, but it marks lines *inside* a subagent's own
  transcript, not a parent line pointing at one. It is useless for finding a
  fan-out from the parent file alone — you have to know to look at the
  sibling directory.

This makes "newest mtime across parent + subagents" straightforwardly
computable: `fixtures/real-fanout-session/00366bef.../subagents/*.jsonl` next
to `fixtures/real-fanout-session/00366bef....jsonl` mirrors the real layout
byte-for-byte (copied from `~/.claude/projects/.../metasys/`). The program's
`progress_mtime` walks `<stem>/subagents/*.jsonl` next to the parent file and
takes the max. Demonstrated against the fixture with parent mtime forced to
09:00 and one subagent forced to 10:00: parent-only mtime reports 09:00 (an
hour "stale"); parent+subagents reports 10:00 (healthy). That is exactly the
false-stall the ticket describes, and it is fixed by a ~15-line directory
walk, not by trusting `isSidechain`.

## (a) strict derive: verbatim errors on real data

A three-field struct (`type`, `sessionId`, `attributionSkill`, all required,
no `Option`) — the shape someone would write after skimming a couple of
lines — fails on **every real file tested**, on line 1, because
`attributionSkill` isn't present on most lines:

```
line 1: missing field `attributionSkill` at line 1 column 82
```

(from `real-small-1.jsonl`; same failure mode, different column, on
`real-small-2.jsonl` and `real-parent-heterogeneous.jsonl`). One `?` per line
means the first miss kills the whole file — you get nothing, not "skill
unknown for that line."

On the deliberately type-changed fixture (`attributionSkill: "sensei"` →
`attributionSkill: {"name": "sensei"}`):

```
line 1: invalid type: map, expected a string at line 1 column 1054
```

Not found in the wild — every real occurrence of `attributionSkill` was a
string — so this had to be constructed by editing a copy. Still real and
still plausible: this is exactly what a field promoted from a scalar to a
richer shape in a future Claude Code release would produce.

## (b) `Option` + `#[serde(default)]`: what it buys, what it doesn't

Struct declaration cost: **+3 lines** over strict (8 lines vs 5, wrapping each
field in `Option<...>` and adding `default` to each `#[serde(rename = ...)]`).
That part is cheap.

The real cost is at the call site, and it's not the struct — `default`
degrades *absent keys* for free, but a single `serde_json::from_str::<Line>`
call is still one atomic operation: if the line is not JSON at all, or a
present field has the wrong *type* (not: wrong presence), the whole call
still returns `Err` for that line, and **every other field on that line is
lost too**, not just the bad one. Measured on the fixtures:

| fixture | (a) strict | (b) defaulted | (c) tolerant |
|---|---|---|---|
| real transcripts (x3) | `Err`, aborts file | recovers, 0 skipped | recovers, 0 notes |
| `not-json.jsonl` (one non-JSON line) | `Err`, aborts file | recovers, **1 line skipped** | recovers, 1 note |
| `truncated.jsonl` (killed mid-write) | `Err`, aborts file | recovers, **1 line skipped** | recovers, 1 note |
| `renamed-field.jsonl` | `Err`, aborts file | recovers, skill silently `None` | recovers, skill `None` |
| `type-changed.jsonl` | `Err`, aborts file | recovers file, **1 line skipped** (loses the WHOLE line, not just the bad field) | recovers, skill `None`, 1 note naming which field was wrong-shaped |
| `empty.jsonl` | `Ok(None)` | `Ok(None)` | `Ok(None)` |
| missing file | `Err` (io) | `Err` (io) | `Err` (io) — caller degrades this uniformly for all three |

To get row 2-5's "recovers" result at all, (b) cannot use the idiomatic `?`
per line — it needs an explicit `match`/`continue` loop identical in shape to
(c)'s loop (see `defaulted::now_skill` vs `tolerant::now_skill`, 21 vs 17
lines including the enum). So (b) pays the struct-declaration tax from
`Option`-wrapping *and* still needs (c)'s per-line-error-handling loop *and*
still loses whole lines on a type mismatch that (c) survives field-by-field.
It is strictly worse than (c) on every axis except "the fields are visible in
one place as a struct."

## (c) hand-written tolerant `Value` lookups: the actual cost

No struct. `skill_of` (17 lines) does one `serde_json::from_str::<Value>`,
then a `match` on `.get("attributionSkill")` distinguishing "absent" from
"wrong type," returning an enum, never an `Err`, never a `panic!`. It is the
*only* one of the three that:

- survives every fixture without losing sibling fields on a bad line, and
- reports *which* field degraded and *why* (`"attributionSkill present but
  not a string"`), which is what a status view needs to say "could not
  observe: X" rather than silently rendering `None`.

Cost: **one function per field you care about** (here, one — `attributionSkill`).
For grind's status view this scales linearly with how many distinct fields it
actually reads (a handful: `attributionSkill`, maybe `message.role`,
`timestamp`), not with the transcript's 44 keys — you never have to look at, name,
or declare the 41 keys you don't need. That is the load-bearing difference
from (a)/(b): a derive struct has to be kept in sync with (or ignore-by-default)
every key the format has, forever, across an undocumented format that changes
without notice; a `Value`-lookup function only has to be kept in sync with the
handful of keys it actually reads.

## Recommendation, with a stated cost

**Use (c).** Its cost is real and worth naming plainly: no compile-time
guarantee that `skill_of` matches any particular shape, no autocomplete on
`value.get("...")` string keys, and a typo in a field name degrades silently
to "absent" instead of a compile error — the same trade Python's
`data.get("attributionSkill")` already makes today, which is *why* this crate
is expected to look worse than the Python it replaces: Rust's whole
value-proposition (static shape checking) is inapplicable to a format that
has no stable shape and offers no versioning contract. `serde` derive is a
tool for *your own* wire format (see `record/`, which owns `run.json` and can
legitimately be strict); it is the wrong tool for a format you do not control
and that has already been observed to carry two spellings of the same field.

Stated cost at every call site, concretely: for N fields grind's status view
actually reads from a transcript, (c) costs N small `match`-returning
functions (~10-20 lines each, one-time) plus a call per field per line
(`value.get("field").and_then(...)`, one line). No struct to maintain, no
`Option`-wrapping tax, no risk of a single bad field silently destroying
sibling fields on the same line. The price paid for that is exactly the price
Python already pays for the same reason — this crate should not claim to have
solved undocumented-format parsing better than the Python it replaces; it
should be honest that it costs the same per-field vigilance, just spelled in
`match` instead of `.get(...)`.

## Surprises

- `isSidechain` is a red herring for *finding* a fan-out from the parent side
  — it is real, but only inside the subagent's own file, never on a line the
  parent wrote. The directory convention (`<uuid>/subagents/*.jsonl`) is the
  actual signal, and it is completely undocumented; nothing in a parent
  transcript announces "subagents exist, look in this sibling directory."
  Grind's real code needs this exact assumption commented loudly, because it
  is the kind of detail a future Claude Code release changes without notice.
- The `session_id`/`sessionId` duplication is not a one-off glitch: it splits
  cleanly along message-type lines (metadata-ish lines use `session_id`,
  most transcript entries use `sessionId`) but the split isn't durable
  strict-typed knowledge — it's exactly the kind of "worked in the sample I
  looked at" trap a derive struct hides until a rename flips which key wins.
- `attributionSkill` being *always* a string in every real occurrence found
  (hundreds of lines, dozens of files) means the ticket's type-change failure
  mode had to be manufactured. That is itself a finding: the truly dangerous
  shape drift in this format is field *disappearance*/*renaming*, observed
  constantly (44 keys, no line has more than a dozen), not field type
  changes, which are rare enough that none turned up empirically.

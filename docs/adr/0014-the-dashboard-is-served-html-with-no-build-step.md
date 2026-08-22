---
status: accepted
date: 2026-08-21
---

# The dashboard is served HTML with no build step

The dashboard is server-rendered HTML produced by Rust, refreshed by roughly a hundred and
fifty lines of hand-written vanilla JavaScript, styled by one CSS token sheet — all
embedded in the binary. The HTTP layer is hand-rolled on `std::net` inside `src/serve.rs`:
GET-only, bounded, `Connection: close`. Updates arrive by polling, never by a held-open
connection. There is no npm, no bundler, no frontend framework, and no vendored runtime.

Decided in session on 2026-08-21 alongside
[ADR-0013](0013-the-ui-serves-the-record-it-owns-nothing-and-writes-nothing.md), whose
process model this builds on.

## Why no frontend stack

The client's entire job is: poll a fragment endpoint, swap it in, tick relative timestamps,
follow a log, and move a keyboard cursor. That is five loops, not an application. A
framework's value — composability, state management, an ecosystem — buys nothing here, and
its costs are real: a build step, a dependency tree, and a second place where the truth
about a Run gets shaped.

HTMX was weighed seriously and rejected on evidence, not taste: its polling has **no
built-in hidden-tab pause** (open issue `bigskysoftware/htmx#824`, filed 2022 — the
workaround is a trigger-filter hack that misses the immediate refresh on return), and its
swap model replaces content wholesale, which fights a byte-offset log append with an
evolving cursor. The decisive feature of this dashboard — pause when unseen, resume
instantly, append a log without re-rendering it — is exactly the part HTMX does not carry.
Vanilla wins on capability here, before dependency count even enters.

## Why the socket lives in serve, not world

[ADR-0007](0007-side-effects-live-in-one-module.md) gave effectful primitives one home:
`tests/topology.rs` lets `std::fs`, `std::process` and `std::env` be named in `world.rs`
only. `std::net` breaks that pattern deliberately. File reads are primitives the whole
program shares, so they centralize; the TCP listener is the *essence* of one module —
accepting, parsing, routing, responding is what `serve` is. Routing a listener through
`world::bind` / `world::accept` wrappers would drag stream reads and writes through
ceremony without making anything more testable: the request parser is pure and tested
directly; the accept loop is not testable anywhere.

The rule is therefore stated as an amendment to the topology, not an exception to
[ADR-0007](0007-side-effects-live-in-one-module.md): **`std::net` may be named in
`src/serve.rs` only**, carried by `tests/topology.rs` beside the fs/process/env rules. If a
second network consumer ever appears, that is the moment to reconsider the home — not
before.

## The HTTP kernel's non-negotiables

Boring and correct, in the order a request meets them:

- **Always answer with `Content-Length`.** Bodies are known — embedded assets, rendered
  fragments, file deltas — so framing is free, close-delimited ambiguity is gone, and
  `Connection: close` plus an actual close retires the connection cleanly.
- **Validate before serving.** Malformed request-line → 400; an HTTP/1.1 request without
  exactly one `Host` → 400; known-but-unsupported method → 405 with `Allow: GET`; unknown
  method token → 501; unknown path or run-id → 404 that leaks nothing about the filesystem.
- **Decode per segment, after splitting.** Percent-decoding the whole path first turns
  `%2F` into a separator and `%2E%2E` into traversal — the classic bug. Segments decode
  individually; run-ids must match a strict character allowlist; `.` and `..` segments are
  refused. Raw-file routes whitelist exact filenames; no route ever builds a path from
  unparsed input.
- **Bound the input.** Request-line and headers capped (≈8 KiB), read timeouts set, one
  thread per connection — a rogue or half-open client pins one thread, not the server.
- **Label every body.** `text/html; charset=utf-8` for pages, `text/plain;
  charset=utf-8` for logs and raw evidence — never HTML for bytes a Run wrote — plus
  `X-Content-Type-Options: nosniff` throughout, `Cache-Control: no-store` on dynamic
  routes.

## Why polling and not push

A correct server-sent-events implementation needs per-viewer held connections, heartbeat
comments to notice dead peers, and reconnect bookkeeping — real state, owned forever, to
buy sub-second latency nobody asked for. Polling is stateless, self-healing across server
restarts, and at one to five viewers costs single-digit requests per second. The client
adapts its own cadence — quick while anything is live, slow when everything is terminal —
and pauses entirely when the tab is hidden, because browser timers throttle anyway and the
only correct response to *unseen* is *paused*. If sub-second push is ever genuinely wanted,
SSE is the upgrade path and needs no client framework; that is the tripwire, and it has no
measured example.

## Wording discipline crosses into HTML

The dashboard renders the same facts as the terminal surfaces, so it inherits the same
law: every string derived from a Run passes through an escaping function (tested against a
hostile payload — model output is arbitrary bytes until proven otherwise); the
quality-word bans the renderer's tests enforce on prose apply to the page's labels and
hints; state badges carry the state *verbatim as the record spells it*, because a badge is
data wearing color, not a verdict. Numbers render `tabular-nums`; color never carries
meaning alone; motion respects `prefers-reduced-motion`.

## Consequences

- **Four flat module siblings join `src/`** — `serve` (kernel, router, lifecycle), `page`
  (pure HTML), `style` and `script` (embedded constants). No directory under `src/`, per
  the existing topology rule; assets are Rust string constants, so the binary stays the
  only artifact.
- **The JS budget is a ruling.** Growth past ~200 lines is a smell to inspect, not a
  license to reach for a framework — it usually means the server stopped rendering.
- **Sorting and filtering are server-side query parameters**, shareable as URLs, costing
  zero client code — the server already owns rendering.
- **Log following is a byte-offset protocol** with the offset transmitted explicitly by
  the server (UTF-8 makes client-computed lengths wrong), truncation answered by a reset,
  and each poll capped.

## Explicitly out of scope

A JSON API, WebSockets, themes beyond the one dark sheet, internationalization, and a
mobile-first layout are all out of scope. The pages are the interface; the terminal
surfaces remain the interface for anyone already there; and nothing here serves anything
but the host it runs on.

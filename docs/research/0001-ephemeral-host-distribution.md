# What shipping a Rust binary to an ephemeral host actually costs

Research against [issue #30](https://github.com/FlorianRiquelme/grind/issues/30): given the base
is Rust (decided on [the map](https://github.com/FlorianRiquelme/grind/issues/29), not reopened
here), what does getting that binary onto a host that did not exist an hour ago cost, with no
human present at boot? TypeScript and every JS runtime are out of scope per the map — this is not
a language comparison.

Method: first-hand measurement on this machine (Darwin arm64, macOS 26.0.1, `rustc`/`cargo`
1.95.0) wherever the question could be reproduced locally — real cross-compiles, real binaries run
inside real Linux container images via Docker/OrbStack, real timings. Where a claim rests on docs
or a vendor's own README instead of something I ran, it's marked **documented** rather than
**measured**, matching the register `docs/research/0001-headless-auth.md` (on
`research/headless-auth`) already uses for the equivalent claude-auth question. This ticket found
no existing convention in-tree for numbering `docs/research/` files that would clash with that
branch — it isn't merged, so both branches independently start at `0001`.

A single probe crate (`gridprobe`) stood in for what Grind actually does: it spawns a child
process and parses its stdout as JSON (exactly `bin/grind`'s shape today), then also stats a file
for mtime, resolves `github.com` over DNS, and takes/releases an `flock` — the four extra things
the ticket named as open questions for a future Rust base, whether or not it ends up needing them
all.

## Answer

**Cross-compiling from this Darwin arm64 host works today, needs no Docker, and takes seconds per
target — via `cargo-zigbuild`, not `cross`.** `cross` requires Docker or Podman by its own README;
`cargo-zigbuild` needs only `zig` (installed via `brew install zig`, ~30s) and
`cargo install cargo-zigbuild` (~23s, one-time, on the always-on dev machine — never on the
ephemeral host). After that one-time setup, `cargo zigbuild --target <triple>` cross-compiled all
four Linux triples the ticket named in 2–4 seconds each.

**The glibc-skew risk is real and I reproduced it directly, not hypothetically.** A default
`cargo zigbuild --target x86_64-unknown-linux-gnu` build links against `zig`'s bundled default
glibc floor (`GLIBC_2.30` here, on zig 0.16.0). Run on an `amazonlinux:2` container (glibc 2.26,
still a common EC2 default-AMI family) it refuses to even start:
`version 'GLIBC_2.28' not found`. Two independent fixes both worked, measured, on the identical
image: (1) `cargo zigbuild --target x86_64-unknown-linux-gnu.2.17` — `cargo-zigbuild`'s documented
glibc-version-pinning syntax — produced a binary capped at `GLIBC_2.16`, no build-time cost over
the default; (2) the plain `x86_64-unknown-linux-musl` static build of the same source, unmodified,
ran on the same image with zero glibc dependency at all. **Static musl linking measurably removes
glibc skew for everything Grind's probe exercised** — process spawn, JSON parsing, file mtime, DNS
resolution, and `flock` all behaved identically across an old-glibc host (Amazon Linux 2, glibc
2.26), a new-glibc host (Debian 12, glibc 2.36), and a musl-native host (Alpine) — no difference
found for any of the four things the ticket asked about, for this exact set of operations.

**Building on the host instead is measurably slower and adds a live network dependency to the boot
path.** A from-scratch `rustup-init.sh --profile minimal` plus a cold `cargo build --release` of
the 3-dependency probe crate, timed inside a bare `debian:bookworm-slim` container, took
44–108s for the toolchain install (two runs, real variance) plus ~10s to compile — call it roughly
a minute to a minute and a half before a binary exists, entirely dependent on `static.rust-lang.org`
and `crates.io` being reachable at exactly the moment nobody is watching. Shipping a prebuilt
artifact instead means copying one already-linked file — no toolchain, no package index, no
compiler anywhere in the boot path.

**`gh` is already exactly as load-bearing as `git` in the current codebase, so "zero dependencies"
was never true even for the stdlib-only Python script.** `bin/grind` shells to both for the
majority of what it does (see below). If [#31](https://github.com/FlorianRiquelme/grind/issues/31)
keeps that shape, the Rust binary's own linking (musl or glibc, static or dynamic) is only one of
at least three things that have to exist and authenticate on the host at boot — `claude`, `git`,
and `gh` are the other two regardless of language.

**`gh` has no single-command equivalent of `claude setup-token`.** `gh`'s own `gh help environment`
documents `GH_TOKEN`/`GITHUB_TOKEN` as first-class, explicitly for automation
("This method is most suitable for 'headless' use of gh such as in automation"). The closest match
to `claude setup-token`'s shape — mint once with a human present, then reuse indefinitely with none
— is a classic personal access token created once via GitHub's web UI with expiration set to
"No expiration," injected as `GH_TOKEN`. GitHub's own docs describe that as opt-out, not
mandatory: "we highly recommend adding an expiration to your personal access tokens" — implying
"no expiration" is a real, supported choice, not a workaround. The same docs also state the token
is auto-revoked after a year of *disuse* — the identical one-year clock the Claude research already
found for `claude setup-token`, just framed as an inactivity timer instead of a hard mint-time
expiry. **Not tested**: I did not create a real "no-expiration" PAT or exercise the GitHub-App
installation-token path end to end — flagged as a gap in §6, not filled in with a guess.

## 1. Cross-compilation from Darwin arm64

Targets added via `rustup target add`: `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu`,
`aarch64-unknown-linux-musl`, `x86_64-unknown-linux-musl`. (Only `aarch64-apple-darwin` and
`wasm32-wasip2` were installed beforehand.)

| Tool | Requires | Tried this session |
|---|---|---|
| `cross` ([cross-rs/cross](https://github.com/cross-rs/cross)) | Docker or Podman — stated directly in its own README ("One of these container engines is required... `cross` will default to `docker`") | **Not installed/run** — documented from its own README, not exercised |
| `cargo-zigbuild` ([rust-cross/cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild)) | `zig` as the linker, installable via `brew`, `pip`, or a direct download | **Installed and used for every measurement below** |

Setup, timed: `brew install zig` (0.16.0_1) — a few minutes including the initial download;
`cargo install cargo-zigbuild --locked` — 23s to compile `cargo-zigbuild` 0.23.0 from source. Both
one-time costs on the developer machine, never on the ephemeral host.

Build times, per target, `cargo zigbuild --target <triple> --release` on the 3-dependency probe
crate (`serde_json`, `libc`), warm dependency cache:

| Target | First build (cold cache) | Rebuild (warm cache) |
|---|---|---|
| `aarch64-unknown-linux-gnu` | ~4s | ~3s |
| `x86_64-unknown-linux-gnu` | ~3s | ~3s |
| `aarch64-unknown-linux-musl` | **~37s** (first-ever musl build: zig builds and caches musl's libc runtime objects for that triple) | ~3s |
| `x86_64-unknown-linux-musl` | **~35s** (same, separate cache per triple) | ~3s |

The musl one-time cost is per-triple-per-machine (a zig runtime cache), not per-build — every
musl rebuild afterward was as fast as gnu. `gnu` targets never paid this because `cargo-zigbuild`
links against pre-built glibc stub objects rather than compiling any libc.

Verified with `file` and `strings | grep GLIBC_` that the artifacts are real: gnu targets produce
dynamically-linked PIE ELF binaries with a real interpreter path
(`/lib/ld-linux-aarch64.so.1`, `/lib64/ld-linux-x86-64.so.2`); musl targets produce statically
linked ELF binaries with zero `GLIBC_*` version symbols.

## 2. glibc version skew — reproduced, not just asserted

Default `cargo zigbuild --target x86_64-unknown-linux-gnu` (zig 0.16.0's default glibc floor) links
against symbols up to `GLIBC_2.30`. I ran the resulting binary — via Docker/OrbStack,
`--platform linux/amd64` since this host is aarch64 — inside five real Linux images, and independently
confirmed each image's own glibc version with `ldd --version` inside the same container:

| Image | glibc (confirmed via `ldd --version` in-container) | Default build result |
|---|---|---|
| `amazonlinux:2` | 2.26 | **fails**: `version 'GLIBC_2.28' not found` (also 2.29, 2.30) |
| `debian:buster-slim` (Debian 10) | 2.28 | **fails**: `version 'GLIBC_2.29' not found` (also 2.30) |
| `debian:bullseye-slim` (Debian 11) | 2.31 | passes, full probe output correct |
| `debian:bookworm-slim` (Debian 12) | 2.36 | passes, full probe output correct |
| `alpine` | n/a (musl, no glibc) | n/a — see musl row below |

Two independently-verified fixes on the identical failing image (`amazonlinux:2`, glibc 2.26):

1. **Pin the glibc floor at build time.** `cargo zigbuild --target x86_64-unknown-linux-gnu.2.17`
   is `cargo-zigbuild`'s own documented syntax for choosing a minimum glibc version. The resulting
   binary's `GLIBC_*` symbol ceiling dropped to `2.16` (verified with `strings`), ran cleanly on
   `amazonlinux:2`, and took no longer to build than the unpinned default (~2–3s).
2. **Link musl statically instead.** The unmodified `x86_64-unknown-linux-musl` build of the same
   source ran on `amazonlinux:2` with no glibc dependency to skew against — confirmed no
   `GLIBC_*` strings in the binary at all.

Both fixes were then cross-checked for full correctness (not just "doesn't crash on startup") on
`amazonlinux:2`: child-process spawn, JSON parse of the child's stdout, file mtime, DNS resolution
of `github.com`, and `flock` acquire/release all returned correct results for both the
2.17-pinned gnu build and the musl build.

**Caveat on the pinning approach**, from `cargo-zigbuild`'s own README (documented, not
independently re-derived): pinning a glibc floor "does not necessarily match the behaviour of
dynamically linking to a specific version of glibc on the build host" and statically linking
glibc itself via `+crt-static` is explicitly **not supported** upstream in `zig cc`. The pin
changes which symbol versions the linker is willing to reference; it is not a from-scratch glibc
build, and its accuracy depends on zig's bundled headers matching the real target closely enough —
worth remembering as a residual gap rather than a proven-airtight fix.

## 3. What musl static linking costs, and what changes under it

Cost, measured: the one-time ~35–37s per-triple libc-runtime cache build (§1), and a modest binary
size increase — with only two dependencies (`serde_json`, `libc`):

| Target | gnu (dynamic) | musl (static) | Difference |
|---|---|---|---|
| `aarch64-unknown-linux-*` | 466,568 bytes | 508,488 bytes | +9.0% |
| `x86_64-unknown-linux-*` | 523,352 bytes | 557,784 bytes | +6.6% |

At Grind's likely real dependency count, actual code will dominate this delta far more than musl's
static-libc overhead does.

Behavior, tested directly for the four things the ticket named, across an old-glibc host
(`amazonlinux:2`, 2.26), a new-glibc host (`debian:bookworm-slim`, 2.36), and a musl-native host
(`alpine`) — **no difference found in any of them** for this probe:

- **Process spawning** (`std::process::Command` spawning `echo`, reading stdout) — identical
  success everywhere.
- **Filesystem mtimes** (`std::fs::metadata(...).modified()`) — identical success everywhere.
- **`flock`** (raw `libc::flock(fd, LOCK_EX | LOCK_NB)` then `LOCK_UN`, as a run-state lock file
  would use) — identical success everywhere, both the gnu and musl builds.
- **DNS resolution** (`ToSocketAddrs` resolving `github.com:443`, real outbound query from inside
  each container) — identical success everywhere, resolving to the same address ranges
  (`140.82.x.x`).

What I could not exercise but is documented from musl's own project wiki
([Functional differences from glibc](https://wiki.musl-libc.org/functional-differences-from-glibc)):
musl's resolver queries all `/etc/resolv.conf` nameservers **in parallel** rather than glibc's
sequential-with-fallback; it does not support glibc's `single-request`/`single-request-reopen`
options; it added DNS-over-TCP only in musl 1.2.4 (matters for large DNSSEC/DKIM responses); and
critically, **musl implements no NSS (Name Service Switch) module system at all** — no
`/etc/nsswitch.conf`, no `nscd`, no LDAP/custom-plugin resolution, only `/etc/hosts` and plain DNS.
None of this should affect Grind resolving `api.github.com` over plain DNS from a fresh host — my
probe's identical, successful resolution across all three hosts is consistent with that — but it's
a documented difference I didn't independently stress-test (no NSS-backed environment was set up
to try to break).

**TLS was not exercised hands-on this session** (no HTTP client crate in the probe — out of scope
for what a one-session probe should build), but is directly relevant if
[#31](https://github.com/FlorianRiquelme/grind/issues/31) moves Grind off `gh`/`git` subprocesses
and onto a direct HTTP client. Documented from primary sources: `rustls` is pure Rust with no
OpenSSL/system-TLS dependency and is the standard low-friction choice for a statically-linked musl
binary; `native-tls`/`openssl-sys` need a real C OpenSSL build against the target and are a
well-documented source of musl cross-compile failures (`reqwest` issue
[#495](https://github.com/seanmonstar/reqwest/issues/495); a `files-sdk-rs` PR fixing exactly this
by switching to `rustls-tls` explicitly to support "static builds that run on older-glibc enterprise
distros like RHEL 8"). One nuance surfaced in `rustls`'s own issue tracker
([#1945](https://github.com/rustls/rustls/issues/1945)): `rustls`'s common `aws-lc-rs` crypto
backend has itself been reported to fail C-toolchain header checks on
`aarch64-unknown-linux-musl` in some cross-compilation setups; the plainer `ring` backend is
reported not to hit this. If Grind ever needs direct HTTPS, this is a concrete thing to verify
before assuming "use rustls" fully closes the question — flagged, not resolved, here.

## 4. Building on the host instead of shipping a prebuilt artifact

Measured inside a bare `debian:bookworm-slim` container (apt-get install of `curl`,
`build-essential`, `ca-certificates`, then the official `sh.rustup.rs` installer, then a cold
`cargo build --release` of the probe crate with its target dir removed):

| Stage | Run 1 | Run 2 |
|---|---|---|
| `rustup-init.sh --profile minimal --default-toolchain stable` | 108s | 44s |
| `cargo build --release` (3 deps, cold) | — (not separately timed) | 10s |
| **Total to a working binary** | — | **54s** |

The spread between runs (108s vs 44s) is real, not noise-free — most likely apt/network cache
warmth differences between the two container invocations — so treat "roughly a minute, plausibly
up to two" as the honest range rather than either single number.

**Failure modes with nobody watching**, inferred directly from what the install path depends on
(not separately fault-injected this session): the ~100MB-class `rustup-init` download and the
`crates.io`/`static.rust-lang.org` fetches are both live network dependencies sitting in the boot
path. `rustup-init.sh` itself has no built-in retry; a transient network blip during either step
fails the boot with no automatic recovery, which is exactly the shape `bin/grind`'s own
`LIMIT_SLEEP_SECONDS`-based re-entry already exists to survive for `claude` — a from-scratch
toolchain install would need the same kind of wrapper to be as resilient as the rest of Grind
already is, or it becomes a second, un-instrumented failure class on top of the one Grind was built
to survive.

Contrast: shipping a pre-cross-compiled artifact is copying one already-linked file. Nothing in
that path touches a package index, a compiler, or (beyond fetching the one file) the network at
all. The cross-compile step itself, measured in §1, takes 2–4 seconds per target — but that's on
the always-on developer machine, never in the ephemeral host's boot path.

## 5. What must be on the host regardless

Read directly out of `bin/grind` (not inferred):

- **`claude`** — not optional. `invoke()` (`bin/grind:362`) builds and runs
  `[state.get("claude_bin") or "claude", "-p", ...]`; `resolve_claude_bin()` (`bin/grind:177`)
  exists specifically to find the real binary rather than a shim. This is the process Grind
  supervises; there is no version of Grind, in any language, that doesn't need it on the host.
- **`git`** — confirmed load-bearing today, not hypothetical. `resolve_repo_path()`
  (`bin/grind:197`) runs `git remote get-url origin`; `resolve_worktree()` (`bin/grind:212`) runs
  `git worktree list --porcelain` and `git worktree add`; `observe()` (`bin/grind:245`) runs
  `git rev-list --count` to compute `commits_ahead`; `cmd_run()` (`bin/grind:498`) runs
  `git rev-parse HEAD` and `git status --porcelain`.
- **`gh`** — equally load-bearing today, not a smaller dependency than `git`. `parse_job()`
  (`bin/grind:104`) runs `gh issue view --json ...` to read the Job itself; `observe()`
  (`bin/grind:245`) runs `gh pr view --json` to detect the open PR; `DENIED_TOOLS`
  (`bin/grind:42`) specifically denies `Bash(gh pr merge*)` — a constraint that only makes sense
  because `gh` is how the dispatched Run touches GitHub in the first place.

**Consequence for this ticket's own math**: whether the future Rust binary is musl-static or
glibc-dynamic changes nothing about `git` and `gh` needing to exist and authenticate on the host
independently. [#31](https://github.com/FlorianRiquelme/grind/issues/31) is genuinely undecided —
whether the Rust base keeps shelling out to them or moves to a library/HTTP client — but as
`bin/grind` is written **today**, both are already required child-process dependencies of a
"stdlib-only, no dependencies" script. "Zero dependencies" describing the current Python
implementation was already not quite true before any Rust question entered the picture; framing
static-linking-removes-a-dependency as removing *the* dependency overstates what it removes. If
#31 keeps the subprocess shape, provisioning `git` and `gh` onto the ephemeral image (and
authenticating `gh` — see §6) is a cost this ticket's glibc/musl analysis doesn't touch at all.

## 6. Auth at boot

[#7](https://github.com/FlorianRiquelme/grind/issues/7)'s research (`docs/research/0001-headless-auth.md`
on `research/headless-auth`, not yet merged) already established `claude`'s answer: `claude setup-token`
mints a one-year, portable OAuth bearer token via `CLAUDE_CODE_OAUTH_TOKEN`, generated once with a
browser on any machine, reusable at boot with zero human interaction until the year is up. That
finding isn't reopened here — restated only as the baseline `gh` gets compared against.

Read directly from this host's `gh help environment` (v2.96.0):

> `GH_TOKEN`, `GITHUB_TOKEN` (in order of precedence): an authentication token that will be used
> when a command targets either `github.com` or a subdomain of `ghe.com`... This method is most
> suitable for "headless" use of gh such as in automation.

So `gh` does have a first-class headless path — an env var — but **no single command that mints
the credential the way `claude setup-token` does.** The practical options, established from
GitHub's own docs and `gh`'s own CLI maintainers on the record, not inferred:

| Option | Mint step | Lifetime | Human-at-boot? |
|---|---|---|---|
| Classic PAT, "No expiration" | One-time, via github.com's UI (or `gh api`) | Documented: auto-revoked after **one year of no use** — GitHub's own docs: "GitHub automatically removes personal access tokens that haven't been used in a year" | No, once minted and injected as `GH_TOKEN` |
| `gh auth login` interactive OAuth (`gho_...`) | Browser device-flow, same shape as `claude`'s interactive `/login` | Documented by `gh` maintainers as not expiring on its own ([issue #12009](https://github.com/cli/cli/issues/12009), [#3855](https://github.com/cli/cli/issues/3855)) — but GitHub's general token-expiration docs also state OAuth tokens are auto-revoked after a year of disuse | **No** — needs the same browser flow every time it's (re-)established, so it doesn't answer "no human at boot" any better than `claude`'s ruled-out interactive path did |
| GitHub App installation token | JWT signed by a stored private key, minted programmatically, no human ever | Short-lived (hours) by design, auto-renewed from the key — the "ceiling" option | Yes, and never again after initial App setup — **not evaluated this session** |

The classic-PAT-with-no-expiration row is the direct functional analogue of `claude setup-token`:
one human action, then indefinite unattended reuse, subject to the same category of "renew
roughly annually" schedule the Claude research already treats as a known, schedulable event rather
than a design blocker. GitHub's own phrasing — "we **highly recommend** adding an expiration" —
reads as the option being real and supported, not a loophole; but I did not mint a real token
this way and inject it into a fresh container to confirm the end-to-end path the way I did for the
cross-compilation and musl claims above. **This row is documented, not measured** — flagged
explicitly rather than presented with the same confidence as §§1–4.

## What I could not establish

- **`cross` (Docker/Podman-based) was not installed or run.** The Docker requirement is
  well-documented in its own README, but I didn't independently verify build times or failure
  modes for it the way I did for `cargo-zigbuild` — the "lower friction path" conclusion rests on
  cargo-zigbuild's measured success plus cross's documented Docker dependency, not on a head-to-head
  run of both.
- **No real TLS/HTTPS client crate was built or cross-compiled.** The `rustls`-vs-OpenSSL
  conclusions in §3 are from GitHub issues and `rustls`'s own tracker, not from a build I ran here.
  This only matters if [#31](https://github.com/FlorianRiquelme/grind/issues/31) moves Grind off
  subprocess `gh`/`git` — worth re-checking with a real build once that's decided.
- **musl's NSS/nsswitch gap and other resolver differences were not stress-tested** — documented
  from musl's own wiki, consistent with (but not proven by) this probe's clean DNS results, since
  the probe never touched an NSS-backed resolution path.
- **No real `gh` credential (no-expiration classic PAT, or a GitHub App installation token) was
  minted and injected end-to-end.** The auth-at-boot conclusion for `gh` rests on GitHub's docs and
  `gh`'s own help text, not on a live unattended-boot test the way the cross-compilation and musl
  findings were verified.
- **`rustup-init.sh` timing (44–108s) is two data points from one network location**, not a stable
  benchmark — real variance was observed between the two runs, and neither run reflects a
  dependency-heavy real Grind rewrite. The probe crate had three dependencies total; actual
  build-on-host time will be higher for real code, in both the from-scratch-toolchain and the
  cross-compile paths — the *relative* ordering (ship-a-file is dramatically cheaper and has none
  of the network-dependent failure modes) should hold regardless of scale, but the absolute numbers
  here should not be read as a ceiling.
- **The `.2.17` glibc-pinning caveat from `cargo-zigbuild`'s own README** — that it "does not
  necessarily match the behaviour of dynamically linking to a specific version of glibc on the
  build host" — was not independently probed for where it might diverge; taken as documented,
  not re-derived.

## Sources consulted

- `rustup`, `rustc`, `cargo` (1.95.0, this host) and `zig` (0.16.0_1, via `brew`) — version and
  target-list output, direct
- [rust-cross/cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) — README (glibc-pinning
  syntax, the `+crt-static` limitation, default glibc-floor-per-zig-version table)
- [cross-rs/cross](https://github.com/cross-rs/cross) — README (Docker/Podman requirement)
- [wiki.musl-libc.org — Functional differences from glibc](https://wiki.musl-libc.org/functional-differences-from-glibc) —
  resolver behavior, NSS absence, DNS-over-TCP history
- [gliderlabs/docker-alpine — caveats.md](https://github.com/gliderlabs/docker-alpine/blob/master/docs/caveats.md) —
  practical corroboration of the musl resolver differences in an Alpine/Docker context
- `gh help environment`, `gh auth login --help` (`gh` v2.96.0, this host) — `GH_TOKEN`/`GITHUB_TOKEN`
  precedence and headless framing, direct
- [github/docs — managing-your-personal-access-tokens.md](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens) —
  classic-vs-fine-grained PAT differences, the one-year inactivity auto-revocation clause
- [github/docs — token-expiration-and-revocation.md](https://github.com/github/docs/blob/main/content/authentication/keeping-your-account-and-data-secure/token-expiration-and-revocation.md) —
  general token revocation/expiration rules, including OAuth-app tokens
- [cli/cli issue #12009](https://github.com/cli/cli/issues/12009) and
  [#3855](https://github.com/cli/cli/issues/3855) — `gh` maintainers on the record re: `gho_` token
  non-expiration and lack of a mintable-with-custom-lifetime command
- [seanmonstar/reqwest issue #495](https://github.com/seanmonstar/reqwest/issues/495) and a
  `files-sdk-rs` PR switching to `rustls-tls` for musl cross-compilation — OpenSSL-vs-rustls musl
  friction
- [rustls/rustls issue #1945](https://github.com/rustls/rustls/issues/1945) — `aws-lc-rs` backend
  musl cross-compile friction, `ring` as the reported workaround
- `bin/grind` (this repo) — `resolve_claude_bin`, `resolve_repo_path`, `resolve_worktree`,
  `parse_job`, `observe`, `DENIED_TOOLS` — direct evidence of current `git`/`gh`/`claude`
  dependency
- `docs/research/0001-headless-auth.md` (branch `research/headless-auth`, not yet merged) — the
  `claude setup-token` baseline this ticket's §6 compares `gh` against
- Docker/OrbStack (this host, aarch64, `--platform linux/amd64` for x86_64 images) running
  `amazonlinux:2`, `debian:buster-slim`, `debian:bullseye-slim`, `debian:bookworm-slim`, `alpine` —
  every glibc/musl execution result in §§1–3 is a real container run, not a simulation

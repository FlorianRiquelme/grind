# One subscription, two hosts

Research against [issue #7](https://github.com/FlorianRiquelme/grind/issues/7): can one Claude x20
(Max 20x) subscription authenticate Claude Code on a headless, ephemeral remote host and on a
laptop at the same time — technically, and under Anthropic's terms? Primary sources only:
Anthropic's own docs (`code.claude.com/docs`, `docs.anthropic.com`, `support.claude.com`,
`anthropic.com/legal`), plus the local `claude` CLI's own `--help` and `auth status` output as a
first-party source. Where only a GitHub issue or community write-up exists, it is marked as
**observed** (someone's reproduced report) rather than **documented** (Anthropic's own text), and
inference is marked as such.

## Answer

**The bet holds.** Anthropic documents a non-interactive authentication mechanism —
`claude setup-token` — built for exactly this shape of problem: "For CI pipelines, scripts, or
other environments where interactive browser login isn't available, generate a one-year OAuth
token with `claude setup-token`" ([Authentication](https://code.claude.com/docs/en/authentication)).
Generation needs a browser once, on any machine; the token it prints is a portable bearer credential
the human copies into wherever the ephemeral host's provisioning injects secrets
(`CLAUDE_CODE_OAUTH_TOKEN`). A freshly destroyed-and-recreated EC2 box authenticates at boot with
zero browser interaction, for up to a year before the token needs regenerating by hand.

Concurrency is real but the contention is on the **usage pool**, not on authentication: "Your
activity across Claude on web, desktop, mobile, and Claude Code all draws from the same pool"
([Plans & Pricing FAQ](https://claude.com/pricing/)) — one account, one rolling 5-hour window, one
pair of weekly caps, shared by every device and every `claude -p` invocation. Terms-of-service risk
is close to zero because Grind already dispatches the real `claude` binary rather than a
third-party harness (`bin/grind` resolves past `cmux`'s shim to the native binary — see
`docs/findings/0001-first-run.md` and the fix that motivated it) — the Consumer Terms' ban on
"automated or non-human means" carves out exactly this ("Except when you are accessing our
Services via an Anthropic API Key **or where we otherwise explicitly permit it**" —
[Consumer Terms of Service](https://www.anthropic.com/legal/consumer-terms), §3), and Anthropic's
own docs show `claude -p` piped into cron and CI as the intended pattern.

The one real hazard is self-inflicted: if the laptop and the remote host are ever made to **share**
the same OAuth credential material (the same `.credentials.json`, the same live refresh token),
Anthropic's refresh-token rotation turns concurrent use into a race that logs one side out. The
fix is architectural, not political — give the ephemeral host its **own** `setup-token`-issued
credential, independent of the laptop's interactive login, and the two never touch the same
refresh cycle.

## 1. Headless authentication

Claude Code has five ranked authentication sources; interactive `/login`/keychain is last, checked
only if nothing else is set
([Authentication](https://code.claude.com/docs/en/authentication)):

| Precedence | Source | Fit for a headless box |
|---|---|---|
| 1 | Cloud provider creds (Bedrock / Vertex / Foundry) | N/A here |
| 2 | `ANTHROPIC_AUTH_TOKEN` | Custom bearer, not subscription-billed |
| 3 | `ANTHROPIC_API_KEY` | Works everywhere incl. `--bare`, but bills the Console, not the subscription |
| 4 | `apiKeyHelper` script output | For rotating vault-issued keys |
| 5 | `CLAUDE_CODE_OAUTH_TOKEN` | **The documented headless-subscription path** |
| — | Interactive `/login` / OS keychain | Needs a human and (usually) a browser |

`claude auth status` on the box used for this research (a Team-plan install, used here only to
inspect CLI shape, not the target Max 20x account) returns:

```json
{
  "loggedIn": true,
  "authMethod": "claude.ai",
  "apiProvider": "firstParty",
  "subscriptionType": "team"
}
```

confirming the CLI does expose a machine-readable auth check (`claude auth status --json`), useful
for a Grind pre-flight.

**Interactive login** needs a browser: "On first launch, Claude Code opens a browser window for you
to log in... If the browser doesn't open automatically, press `c` to copy the login URL... If your
browser shows a login code instead of redirecting back... paste it into the terminal" — the docs
name this fallback explicitly for "WSL2, SSH sessions, and containers"
([Authentication](https://code.claude.com/docs/en/authentication)). That fallback still needs a
human watching a terminal and a browser somewhere, so it does not answer the ephemeral-host
question by itself.

**`claude setup-token`** does:

> For CI pipelines, scripts, or other environments where interactive browser login isn't available,
> generate a one-year OAuth token with `claude setup-token`... The command opens the same browser
> authorization flow as `/login`, and the token prints to the terminal after you approve access in
> the browser. It does not save the token anywhere; copy it and set it as the
> `CLAUDE_CODE_OAUTH_TOKEN` environment variable wherever you want to authenticate.
> — [Authentication § Generate a long-lived token](https://code.claude.com/docs/en/authentication)

Locally, `claude setup-token --help` confirms this is its entire surface: "Set up a long-lived
authentication token (requires Claude subscription)," no other flags. Two documented constraints
that bear directly on Grind:

- **Requires Pro, Max, Team, or Enterprise** — Max 20x qualifies.
- **Scoped down**: "It can only make model requests, so it can't establish Remote Control sessions
  or fetch claude.ai connectors" — irrelevant to a scheduler that only ever runs `claude -p`.
- **Invisible to `--bare`**: "Bare mode does not read `CLAUDE_CODE_OAUTH_TOKEN`. If your script
  passes `--bare`, authenticate with `ANTHROPIC_API_KEY` or an `apiKeyHelper` instead"
  ([Authentication](https://code.claude.com/docs/en/authentication)). Checked against
  `bin/grind`: the dispatch call is `claude -p --output-format json ...` with no `--bare`
  (`bin/grind:370`), so this constraint doesn't bite Grind as written.

**Credential storage**, per the same page:

| Platform | Location | Notes |
|---|---|---|
| macOS | Encrypted Keychain | Confirmed locally: `security find-generic-password -s "Claude Code-credentials"` returns a `genp` entry |
| Linux | `~/.claude/.credentials.json`, mode `0600` | No system keychain to fall back to |
| Windows | `%USERPROFILE%\.claude\.credentials.json` | Inherits user-profile ACLs |
| Any (Linux/Windows) | `$CLAUDE_CONFIG_DIR/.credentials.json` if set | — |

None of this is where `CLAUDE_CODE_OAUTH_TOKEN` lives — that's a plain environment variable the
caller supplies, held nowhere by the CLI, which is exactly the shape an ephemeral host wants (no
disk state to survive destruction).

**Refresh and lifetime** — this is where documented fact runs out and observed community reports
take over. Regular `/login` OAuth sessions carry a short-lived access token (reported as 8–24 hours
depending on version, GitHub issues
[#31095](https://github.com/anthropics/claude-code/issues/31095),
[#42904](https://github.com/anthropics/claude-code/issues/42904)) plus a refresh token that is
supposed to renew it silently but is reported to fail specifically in non-interactive/subprocess
invocations (issue [#53063](https://github.com/anthropics/claude-code/issues/53063): "the OAuth
access token is not auto-refreshed even when the refreshToken... is still valid... This causes 401
authentication_error failures during scheduled/automated runs"). **This is precisely the fragility
`claude setup-token` exists to route around** — its token is issued for a full year up front rather
than relying on an 8-hour local refresh cycle. None of this is Anthropic-confirmed root-cause
analysis; it is what named users reproduced and filed, several with server-response evidence
attached. Treat "silent refresh is unreliable for unattended `claude -p`" as observed, and "use
`claude setup-token` instead" as documented Anthropic guidance for that exact problem.

One more observed, open regression worth a mitigation: issue
[#68241](https://github.com/anthropics/claude-code/issues/68241) reports that on Claude Code
2.1.177+, a stale on-disk `.credentials.json` can **shadow** a perfectly valid
`CLAUDE_CODE_OAUTH_TOKEN` env var and force a failed refresh into a `/login` prompt instead of
falling back to the env var. **Mitigation for Grind:** never let the ephemeral host accumulate a
`~/.claude/.credentials.json` (don't bake one into the AMI, and never call `claude auth login` on
the box) — rely solely on the injected env var so there is nothing to go stale.

## 2. Concurrency across machines

**Documented, permitted, and expected.** Anthropic's own login help explicitly answers this:

> Can I use my Claude account across multiple devices? To use your Claude account across multiple
> devices, enter the same email address you use to log in on your usual device.
> — [Log in to your Claude account](https://support.claude.com/en/articles/13189465-log-in-to-your-claude-account)

and the Consumer Terms restriction is about *sharing the account with another person*, not about
one person using it from two machines: "You may not share your Account login information... with
anyone else. You also may not make your Account available to anyone else"
([Consumer Terms of Service](https://www.anthropic.com/legal/consumer-terms) §2).

**In practice, it mostly works, with one sharp edge.** Two classes of bug surface in the tracker,
and they are different failure modes:

- **Interactive-session takeover** — issue
  [#40419](https://github.com/anthropics/claude-code/issues/40419) (open, filed 2026-03-29):
  "When signing into Claude Code CLI on a second device using the same account, the first device is
  silently logged out. There is no warning." This is about running `/login` a *second time*, which
  mints a fresh interactive session grant that appears to supersede the last one.
- **Shared-refresh-token races** — issues
  [#24317](https://github.com/anthropics/claude-code/issues/24317),
  [#25609](https://github.com/anthropics/claude-code/issues/25609), and
  [#41507](https://github.com/anthropics/claude-code/issues/41507) all describe the same mechanism:
  multiple processes reading the *same* `~/.claude/.credentials.json` (same machine, multiple
  terminals, or a copied file on a second machine) race to redeem the same refresh token; the
  server accepts the first and invalidates the token for everyone else, per standard refresh-token
  rotation (RFC 9700 §2.2.2, cited directly by a commenter on #12447). One process wins, the rest
  are logged out and need a human to `/login` again.

**Neither bug applies to two machines each holding their own `setup-token`-issued credential.**
Several users report exactly this as the working fix: "I worked around this using
`claude setup-token`... it skips all the 'OAuth tokens invalidating each other'" (comment on
[#24317](https://github.com/anthropics/claude-code/issues/24317)). Feature request
[#22995](https://github.com/anthropics/claude-code/issues/22995) is the clearest evidence this
scales: a user reports running `claude setup-token` "across 4+ devices — macOS terminal, Android
(Termux), and 2 Ubuntu VMs" simultaneously; their complaint is the *lack of an audit dashboard* for
all those tokens, not that using several breaks anything. **This is the load-bearing finding for
Grind's design: one `setup-token` per host (or the same one reused across ephemeral instances,
since it isn't tied to a machine identity) avoids the shared-refresh race entirely**, because the
long-lived token is presented directly rather than silently refreshed against a shared, rotating
refresh token.

What is documented but not yet fixed: there is no first-party way to list or selectively revoke one
long-lived token among several (`claude setup-token --list`/`--revoke` is an open feature request,
[#48373](https://github.com/anthropics/claude-code/issues/48373)); revocation today is manual, via
`Settings > Claude Code` in the web UI. Not tested as part of this research whether removing one
token there affects others — flagged as unverified.

## 3. Rate limits

Anthropic's own help center states the shape directly:

> Max plans also have two weekly usage limits: one that applies across all models and another for
> Sonnet models only. Weekly limits reset at a fixed time each week that is assigned to your
> account.
> — [What is the Max plan?](https://support.claude.com/en/articles/11049741-what-is-the-max-plan)

> Every plan has usage limits that reset on a rolling five-hour session window, and paid plans add
> weekly limits on top. Your activity across Claude on web, desktop, mobile, and Claude Code all
> draws from the same pool.
> — [Plans & Pricing FAQ](https://claude.com/pricing/)

| Plan | Per-5-hour-session usage vs. Free | Weekly caps |
|---|---|---|
| Max 5x | 5× Pro | all-models cap + Sonnet-only cap |
| Max 20x | 20× Pro | all-models cap + Sonnet-only cap |

Two things follow directly from "same pool," both **documented, not inferred**:

- The pool is **per account**, not per device or per machine. A laptop session and a remote Run
  drawing on the same Max 20x subscription draw down the same 5-hour window and the same weekly
  caps — there is no separate headless allotment.
- Headless `claude -p` is not called out as a separate meter anywhere in the pricing or usage-limit
  docs; it is "Claude Code," full stop, and Claude Code is explicitly one of the surfaces sharing
  the pool.

On contention, the documented behavior is a soft stop, not a hard cutoff, for anyone with usage
credits enabled: "Instead of being blocked when you hit your session limits, you can switch to
consumption-based pricing at standard API rates and continue your work without interruption... Your
plan's included usage limit will reset every five hours as usual"
([Manage extra usage](https://support.claude.com/en/articles/12429409-manage-extra-usage-for-paid-claude-plans)).
Without usage credits turned on, hitting the limit blocks further use until the next 5-hour or
weekly reset — this is the same "rate limit vs. crash" distinction `docs/findings/0001-first-run.md`
already had to build detection for, just now with two hosts able to trigger it instead of one.

Not documented anywhere found: a scriptable way to check remaining budget before dispatching a Run.
`/usage` is an interactive TUI view; there is no `claude usage --json` equivalent in the CLI
reference. A pre-flight budget check would have to be inferred from Grind's own recorded per-attempt
cost (already captured in `run.json`, per `docs/findings/0001-first-run.md`), not from anything
Anthropic exposes directly.

## 4. Ephemeral hosts

This is the question that decides the bet, and the answer is **documented and mechanically simple**:

`claude setup-token` mints a token that is a **value**, not a **session tied to a machine**. The
CLI docs describe it purely in terms of what you do with the printed string: "copy it and set it as
the `CLAUDE_CODE_OAUTH_TOKEN` environment variable **wherever** you want to authenticate"
([Authentication](https://code.claude.com/docs/en/authentication), emphasis added). Nothing in the
generation flow binds the token to the hostname, MAC address, or any other machine fingerprint —
generation happens once, interactively, on whatever machine has a browser (the laptop, say), and
the resulting string is portable.

For Grind's actual shape — an EC2 host that may be destroyed and recreated — this maps directly
onto a standard secrets-at-boot pattern that has nothing Anthropic-specific about it: store the
token in AWS Secrets Manager or SSM Parameter Store, have the instance's bootstrap (user-data /
systemd unit) fetch it and `export CLAUDE_CODE_OAUTH_TOKEN=...` before invoking `claude -p`. No
step in that path opens a browser, prompts a human, or touches the instance's local disk for
credential state — which means a host that gets terminated and rebuilt from the same launch
template authenticates on its very first `claude -p` call with **zero** human involvement.

What this is not: permanent. The token is documented as valid for **one year**, not indefinitely —
"generate a one-year OAuth token." That is a scheduled, low-frequency human action (regenerate
once a year, update the secret store), categorically different from "re-authenticate on every host
rebuild," which was the disqualifying scenario the ticket named. Grind should treat the annual
expiry the same way `docs/findings/0001-first-run.md` treats a dropped connection: a known,
schedulable failure mode, not a crash — worth a reminder, not a design blocker.

Two caveats already covered in §1 apply here directly and are worth restating because they're easy
to violate by accident on an ephemeral host: don't pass `--bare` (it ignores the env var entirely;
Grind doesn't), and don't let the host's `~/.claude` accumulate a stale `.credentials.json` that
could shadow the env var per the observed regression in
[#68241](https://github.com/anthropics/claude-code/issues/68241) — keep the AMI clean and never
call `/login`/`claude auth login` on the box.

## 5. Alternatives, for the record

The owner has already decided on the single subscription; this table exists only so the options
were visible when that decision was made, per the ticket's own framing.

| Option | Limit semantics | Auth story on an ephemeral host | Terms regime |
|---|---|---|---|
| **Single Max 20x (chosen)** | One 5-hour + two weekly caps, shared by every device and every `claude -p` call | `claude setup-token` baked into boot, no browser, annual renewal | Consumer Terms; permitted because Grind runs the real `claude` binary, not a harness |
| API key (Console) | Per-token billing (see [pricing](https://platform.claude.com/docs/en/about-claude/pricing)), tiered rate limits (Start/Build/Scale), no 5-hour/weekly session shape at all | `ANTHROPIC_API_KEY` env var, works even with `--bare`, no browser ever, no expiry to renew | Commercial Terms; explicitly no automation restriction |
| Second Max/Pro subscription | Fully independent 5-hour + weekly pool, zero contention with the first | Identical to the chosen option — its own `claude setup-token`, its own secret | Consumer Terms, same as above |
| Team/Enterprise seat | Same session-window shape, generally higher caps, admin-managed | Same `setup-token` mechanism; org login restrictions (`forceLoginOrgUUID`) can apply | Consumer Terms (Team) — [Authentication § Restrict login](https://code.claude.com/docs/en/authentication) |

The API key row is the one genuine escape from "shared pool" contention short of a second
subscription — it bills per token against the Commercial Terms rather than drawing down the Max
20x allotment at all — but it was explicitly ruled out of scope by the decision already made.

## Sources consulted

- [code.claude.com/docs/en/authentication](https://code.claude.com/docs/en/authentication) — login
  paths, credential storage per OS, `claude setup-token`, precedence order
- [code.claude.com/docs/en/headless](https://code.claude.com/docs/en/headless) — `-p`/`--bare`
  semantics, what bare mode does and doesn't read
- [code.claude.com/docs/en/env-vars](https://code.claude.com/docs/en/env-vars) — `CLAUDE_CODE_OAUTH_TOKEN`,
  `CLAUDE_CODE_OAUTH_REFRESH_TOKEN`, `CLAUDE_CODE_OAUTH_SCOPES`, `ANTHROPIC_API_KEY`
- [code.claude.com/docs/en/cli-reference](https://code.claude.com/docs/en/cli-reference) —
  `claude setup-token` summary
- [docs.anthropic.com/en/docs/claude-code/legal-and-compliance](https://docs.anthropic.com/en/docs/claude-code/legal-and-compliance) —
  usage policy, "ordinary, individual usage," OAuth vs. Agent SDK guidance
- [anthropic.com/legal/consumer-terms](https://www.anthropic.com/legal/consumer-terms) — account
  sharing (§2), automated-access exception (§3); exact §3 wording cross-checked against three
  independent verbatim quotations ([The Register](https://www.theregister.com/software/2026/02/20/anthropic-clarifies-ban-on-third-party-tool-access-to-claude/5014546),
  a Hacker News thread quoting the clause directly, a Scribd-hosted copy of the document) since the
  live fetch of the page truncated before reaching §3
- [support.claude.com — What is the Max plan?](https://support.claude.com/en/articles/11049741-what-is-the-max-plan) —
  5-hour session + weekly cap shape
- [claude.com/pricing](https://claude.com/pricing/) — "same pool" across surfaces, Max vs. Pro
  multipliers
- [support.claude.com — Manage extra usage](https://support.claude.com/en/articles/12429409-manage-extra-usage-for-paid-claude-plans) —
  usage-credit soft-stop behavior
- [support.claude.com — Log in to your Claude account](https://support.claude.com/en/articles/13189465-log-in-to-your-claude-account) —
  multi-device support stated directly
- Local `claude` CLI (v2.1.220): `claude --help`, `claude auth --help`, `claude auth login --help`,
  `claude auth status --json`, `claude setup-token --help`, `security find-generic-password`
  (existence check only, no secret printed) — read-only inspection, no credential rotation or logout
  performed
- `bin/grind` (this repo) — confirms the dispatch call omits `--bare`
  (`cmd = [state.get("claude_bin") or "claude", "-p", "--output-format", "json", ...]`, `bin/grind:370`)
- GitHub issues on `anthropics/claude-code`, cited inline — **observed** community reports, not
  Anthropic-authored documentation; treated as such throughout and never as the basis for the
  documented claims in §§1–4

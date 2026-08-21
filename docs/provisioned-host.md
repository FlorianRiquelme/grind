# The provisioned host

What must be true of a host before a Dispatch can succeed on it. Decided resolving
[#42](https://github.com/FlorianRiquelme/grind/issues/42), whose comment holds the derivation;
the layout rule is **ADR-0008** and the credential section is
[#37](https://github.com/FlorianRiquelme/grind/issues/37)'s, absorbed here rather than linked.

**There is exactly one definition, and the laptop must pass it.** Not a deployment note for an
ephemeral Linux box: the machine you develop on dispatches Runs too, and an item it cannot satisfy
is evidence the item is wrong rather than evidence the laptop is special. This is what makes the
list exercisable before an ephemeral host exists — ADR-0002's *headless lags local*, applied to
the host itself.

**A host that half-works is the failure this list exists to prevent.** Nearly every item below
fails quietly, and several fail *toward looking healthy*. So each one is marked by how it is
caught:

| Mark | Meaning |
|---|---|
| **dispatch** | Verified before every Dispatch. Presence only — local, free, no network. |
| **doctor** | Verified by `grind doctor`, run by hand after provisioning. The full list, including the live checks. |
| **step** | Performed during provisioning, with no honest boolean behind it. Not checked, because any check would be a guess. |

**`grind doctor` and the dispatch-time checks are decided, not built.** They arrive with the Rust
base (ADR-0005); until then the marks say how each item *will* be caught, and the list is worked by
hand. Both entrypoints share one item list and differ only in depth.

Checking is not gating (ADR-0003). A host failing a precondition has not been judged, and nothing
here blocks a PR on the strength of a finding — a refused Dispatch is incoherent input, the same
shape as the dirty-worktree refusal and [#25](https://github.com/FlorianRiquelme/grind/issues/25)'s
lock.

## Layout

`~/.grind/` is the host's Grind directory, and **its layout is the declaration** — there is no
config file, no environment override, and nothing to keep in sync. `$HOME` is the only variable,
and it is the dispatching user's. See ADR-0008 before changing any of this; the tidy-up that looks
like housekeeping here is what withdraws the guarantee.

```
~/.grind/
  repos/<owner>/<name>    the clone Grind dispatches into
  bin/claude              the binary Grind spawns
  runs/<run-id>/run.json  Run state
  locks/<owner>-<name>-<branch>
```

- **`repos/<owner>/<name>` exists and its `origin` matches the target repo.** — *dispatch,
  doctor* — A real clone on a provisioned box; a symlink to the human's clone on the laptop. `git`
  is transparent to the symlink, so the two cases are one code path. **Grind never clones**: on the
  laptop the clone is the human's and holds their worktrees, and a second Grind-owned copy would
  defeat `resolve_worktree()`'s adopt path and recreate the collision
  [#11](https://github.com/FlorianRiquelme/grind/issues/11) refused from the other side. The origin
  match is [#32](https://github.com/FlorianRiquelme/grind/issues/32)'s surviving finding; the
  `~/Repos/mine/<name>` search it used to wrap is gone.
- **One declared clone per target repo.** — *doctor* — Not a search path. This is what makes
  #25's lock key (`target_repo` + branch) sound: *two clones of one supervised repo* stops being a
  state the host can be in, so the lock cannot miss a collision routed through a second clone.
- **`bin/claude` is executable and is not a shim.** — *dispatch, doctor* — A symlink to the real
  binary on the laptop, the install itself on a box. The assertion is loud, not a filter: on this
  laptop `which -a claude` returns cmux's shim *first, twice*, from under `$TMPDIR`, so
  `ln -s $(which claude)` points at the wrong file and the Run silently inherits that terminal's
  session hooks — reproducible nowhere, with nothing printed.
- **`runs/` and `locks/` are Grind's**, created on demand. Nothing to provision. Run state lives
  here rather than inside a checkout, so *never committed*
  ([#8](https://github.com/FlorianRiquelme/grind/issues/8)) holds because it is outside any git
  repo, not because a `.gitignore` line says so.

## Executables

`claude` is spawned only by Grind, so Grind names the file. `git`, `gh` and `just` are spawned by
the Run and — for the first two — by Grind as well: #31 counted `gh pr create` ×41 and
`git worktree add` ×56 across the `lfg` chain. They must be on `PATH` regardless of what Grind
resolves.

- **`git` on `PATH`, ≥ 2.34.** — *dispatch, doctor* — The floor is inherited from the SSH commit
  signing in step 4 below, not invented. Nothing else in Grind needs a recent git.
- **`gh` on `PATH`.** — *dispatch, doctor* — No version floor. Grind uses `issue view`, `pr view`
  and `auth status`; an invented floor is a precondition that fails for no reason.
- **`just` on `PATH`.** — *doctor* — No version floor, for the same reason as `gh`. Grind never
  spawns it, which is why it was missed when this list was written (ADR-0009 found the gap) — but
  the dispatch prompt makes `just verify` the Run's definition of done, so a host without it fails
  every Run at the last step, in the target repo, with nothing in Grind's own output naming the
  cause. *doctor* rather than *dispatch* because the failure is the Run's, not the Dispatch's.
- **`ps` on `PATH`.** — *dispatch* — Supervisor liveness is a pid **plus** the start stamp
  `ps -p <pid> -o lstart=` prints, because a pid alone is reused. `resume --all` acts on that
  reading at boot, so a `ps` that cannot answer is a Run's re-entry decision made blind — and
  `-p <pid> -o lstart=` is a procps/BSD spelling **busybox `ps` does not implement**, which a
  minimal Linux host is quite likely to be. Presence only: a busybox `ps` is on `PATH` and still
  cannot answer, so this item catches the absent one and `observe::process_start_stamp` catches
  the rest by reading three-valued.
- **The `lfg` plugin is installed.** — *dispatch* — The Job names both the plugin and a literal
  `x.y.z` version, which Enqueue wrote when it drafted the Job (ADR-0002, #50, #69). Nothing on
  the host resolves anything: `job::plugin_dir()` builds the path from that literal, records it,
  and every attempt and every `--resume` reads the record — the version is pinned per Job and
  frozen for the Run's whole life. The host owes an installed plugin, which is all this check
  looks for; which version matters is the Job's literal, never the host's.

## Lifetime

There is no Grind daemon (ADR-0011). The host owes exactly one thing: something that fires after a
restart and calls `grind resume --all`, so a restart re-enters the Runs it cut off instead of
leaving them at `died` until a human looks. Nothing watches a supervisor while the host is up, and
nothing needs to.

**The two platforms do not offer the same promise, and the difference is load-bearing.** linux
fires at boot; darwin fires at **login**. Say *survives a restart* on linux and *survives a restart
plus a login* on darwin, and do not let the shorter phrase stand for both.

- **A restart one-shot calling `grind resume --all` is loaded.** — *doctor* — `RunAtLoad` on
  darwin, `Type=oneshot` on linux. A Dispatch works perfectly well without it, so refusing one
  would gate a Job on something unrelated to it (ADR-0003).

  Templates ship in `dist/` — the first thing Grind ships that is neither the binary nor
  documentation. Install them by hand and edit the path to `grind` if it is not
  `/usr/local/bin/grind`:

  ```sh
  # darwin
  cp dist/launchd/com.grind.resume-all.plist ~/Library/LaunchAgents/
  launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.grind.resume-all.plist

  # linux — three steps, and the third is what makes it fire at boot
  cp dist/systemd/grind-resume-all.service ~/.config/systemd/user/
  systemctl --user enable grind-resume-all.service
  loginctl enable-linger "$(id -un)"
  ```

  **linux needs the linger step, and without it the unit is enabled, correct and never runs.**
  `WantedBy=default.target` on a `--user` unit fires when that user's systemd instance starts,
  and on a headless box — the deployment this exists for — that instance starts at first login
  and stops at last logout, not at boot. `systemctl --user is-enabled` returns enabled purely
  from the symlink either way, so doctor asks both halves in one command; asking only the first
  is the silent pass this check was added to prevent, one layer down.

  **darwin fires at login, and that is as early as it goes.** `launchctl bootstrap gui/$(id -u)`
  puts the job in the GUI domain and `RunAtLoad` fires when that domain loads, so a restarted Mac
  sits with its cut-off Runs un-re-entered until a human logs in. A LaunchDaemon would fire
  earlier but resolves root's `$HOME`, and `$HOME` is the only variable Grind reads with no
  override (ADR-0008); setting `HOME` on the daemon does not rescue it either, because on a
  FileVault Mac — the default — `/Users/<user>` is not decrypted until someone unlocks at the
  login window, so `~/.grind/runs/` does not exist yet to be read.

  Doctor **cannot check this** and does not pretend to: `launchctl print gui/$(id -u)/…` can only
  run from inside a logged-in GUI session, which is the very condition it would need to tell
  *loads at boot* from *loads at login*. The caveat rides its satisfied text instead.

  Two costs. It is the **first platform-branching check** — `launchctl print` against
  `systemctl --user is-enabled` plus `loginctl show-user … Linger`, where every existing check is
  one command everywhere. And it verifies **loaded**, not merely present: a plist sitting on disk
  that was never bootstrapped is the likeliest way this fails, and it fails silently, one reboot
  later, with a Run stranded and nothing saying so.

  Neither unit writes a log file. `/tmp/grind-resume-all.log` was world-readable and named every
  cut-off run id and every skipped Run's worktree path, in a mode-1777 directory launchd appends
  to as the dispatching user while following symlinks. Every line either one prints is appended
  to the relevant `~/.grind/runs/<run-id>/supervisor.log` anyway, which keeps state inside the
  single root ADR-0008 declares.

  The systemd unit carries the one piece of real machinery in this build. A `Type=oneshot`
  service takes its cgroup with it when `ExecStart` exits, and `grind resume --all` spawns one
  detached supervisor per cut-off Run and exits immediately — so the default would kill every Run
  seconds after boot, silently. `KillMode=process` is what stops that, and
  `AbandonProcessGroup` is the launchd half of the same problem.

## Credentials

[#37](https://github.com/FlorianRiquelme/grind/issues/37)'s six steps, verbatim in substance. All
*doctor*, never *dispatch*: #37 ruled that a dead credential is an **outcome classified mid-Run**,
like a rate limit, and explicitly not ADR-0004's pre-flight — *presence is boolean and free where
cost is not*.

They are a checklist rather than a command because **`gh auth login` cannot be trusted to do
them**: its `--git-protocol ssh` flow uploads a key as `authentication` only and never `signing`,
skips the SSH flow entirely when non-interactive, and has no flag pre-answering *"Authenticate Git
with your GitHub credentials?"*.

1. `gh auth login` — device-code flow, `repo` / `read:org` / `gist`. Confirm the store with
   `gh auth status`, expecting `(oauth_token)` where a laptop prints `(keyring)`. Storage is
   plaintext `hosts.yml`; there is no keyring on a headless box and no D-Bus to unlock one.
2. `ssh-keygen`, passphrase-less.
3. `gh ssh-key add --type authentication` **and** `gh ssh-key add --type signing` — the same key
   both times.
4. `git config --global gpg.format ssh`, `user.signingkey` at the **private** key path (so
   `ssh-keygen -Y sign` needs no `ssh-agent` on a host with no login session), `commit.gpgsign
   true`. Worktrees inherit this; repo config is shared across worktrees unless
   `extensions.worktreeConfig` is opted into, which nothing here does. **Run 2 is this step's
   evidence, and it is the first item the laptop demonstrably fails:** 1Password's `op-ssh-sign`
   stopped signing twice mid-Run, which cost the Run its declared branch and left two finished
   items uncommitted. An agent-backed signer makes committing depend on a GUI approval no Grind
   check can see — and `ssh-add -l` keeps listing the key throughout, because listing needs no
   approval and *using* one does, so the obvious check is the one that lies.
   `docs/findings/0002-second-run.md`.
5. `user.name` / `user.email` set to the machine identity, and that email **added and verified**
   on the GitHub account — GitHub matches the committer *email*, not the name, so this is what
   makes `git log` name the Run and the verified badge survive.
6. `origin` on SSH, and the push **verified with a real one rather than assumed**. Step 6 is the
   only step that proves the other five.

The Run inherits both credentials and Grind cannot decide otherwise — `GH_TOKEN=""` is a silent
no-op and `GH_CONFIG_DIR` is plumbing, not a boundary. So `DENIED_TOOLS` is the entire barrier
between a Run and merging its own PR, permanently and by construction. See #37 and `CLAUDE.md`.

## Steps with no boolean

Performed during provisioning; deliberately unchecked, because every check available is a
heuristic guess of the kind this list keeps removing.

- **The `grind` binary is on `PATH`.** — *step* — Not a layout item. `bin/claude` is named by the
  layout because *Grind* spawns it and had to dodge a shim; nothing spawns `grind` except a human or
  a unit file, so the layout has no mechanical use for its path — and an item declaring where the
  binary lives cannot be checked by that binary, since a `grind doctor` you cannot invoke reports
  nothing. That is the loudest failure available and has no honest boolean. #30 ships a prebuilt
  file and never builds on the host; where it lands is the human's `PATH`, and `grind --version`
  answers *which one is this* if the wrong copy is ever found there. Decided resolving
  [#48](https://github.com/FlorianRiquelme/grind/issues/48).
- **Auto-update for `claude` and for the plugin.** — *step* — Since the plugin version floats
  (ADR-0002 as amended), a box whose cache is baked into an image and never refreshed drifts *old*
  silently — the mirror of the risk the amendment accepts. The record makes it observable, since
  every Run names the version it actually ran; nothing is built for it. A staleness check would be
  a mtime threshold, which is a guess.
- **The dispatching user's `$HOME`.** — *step* — A systemd unit with a different `User=` resolves
  a different `~/.grind` and finds nothing. Loud, not silent, which is why no override exists to
  paper over it (ADR-0008).

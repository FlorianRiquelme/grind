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
- **The `lfg` plugin is installed.** — *dispatch* — The Job names the plugin, the host names the
  version (ADR-0002 as amended). Resolution picks the installed version, records the resolved path,
  and every attempt and every `--resume` reads the record — so a Run's plugin version is fixed for
  the Run's whole life even though it was never pinned.

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
   `extensions.worktreeConfig` is opted into, which nothing here does.
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

- **Auto-update for `claude` and for the plugin.** — *step* — Since the plugin version floats
  (ADR-0002 as amended), a box whose cache is baked into an image and never refreshed drifts *old*
  silently — the mirror of the risk the amendment accepts. The record makes it observable, since
  every Run names the version it actually ran; nothing is built for it. A staleness check would be
  a mtime threshold, which is a guess.
- **The dispatching user's `$HOME`.** — *step* — A systemd unit with a different `User=` resolves
  a different `~/.grind` and finds nothing. Loud, not silent, which is why no override exists to
  paper over it (ADR-0008).

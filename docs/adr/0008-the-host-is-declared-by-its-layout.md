---
status: accepted
date: 2026-08-06
---

# The host is declared by its layout, not configured

Three host facts had accumulated with nothing owning them — where the dispatch lock lives, what
replaces `resolve_repo_path()`'s `~/Repos/mine/<name>` guess, and where the `claude` binary is —
and each was heading toward its own mechanism: a stable temp directory guaranteed by systemd, a
config file mapping repos to paths, an environment override per resolver.

They are one decision. `~/.grind/` is the host's Grind directory and **its layout is the
declaration**. There is no config file, no format to parse, and no environment override.

```
~/.grind/
  repos/<owner>/<name>    clone (a box) | symlink to the human's (a laptop)
  bin/claude              the binary Grind spawns
  runs/<run-id>/run.json  Run state
  locks/<owner>-<name>-<branch>
```

Provisioning a clone is `git clone` on a box and `ln -s` on a laptop, to the same path either way.
Recorded resolving [#42](https://github.com/FlorianRiquelme/grind/issues/42), whose comment holds
the full derivation; `docs/provisioned-host.md` is the operative list.

## Why a directory rather than a config file

A declaration was needed — the search it replaces fails silently. Three clone-name collisions
already exist on the author's laptop (`beads-helix`, `slack`, `static-cache-buster` appear under
both `~/Repos/` and `~/Repos/mine/`), so a PATH-style search root picks the first match, misses the
clone holding the human's worktrees, adopts nothing, and produces two checkouts of one branch —
[#11](https://github.com/FlorianRiquelme/grind/issues/11)'s collision arriving through the door the
lock does not watch.

A config file would fix that and cost a strict `serde` format, a parser, and a file to keep in sync
with the filesystem it describes. The filesystem is already the thing being described. `test -d`
is the parser.

## What falls out for free

- **The lock leaves `$TMPDIR`.** `PrivateTmp=yes` giving two supervisors different temp directories
  is no longer expressible, because no temp directory is involved. Provisioning guarantees nothing
  new; the directory it already had to create is the guarantee.
- **The lock key returns to [#25](https://github.com/FlorianRiquelme/grind/issues/25)'s** —
  `target_repo` + branch. One declared clone per repo means *two clones of one supervised repo* is
  not a state the host can be in, so #25's key and the spike's `git-common-dir` key become
  equivalent, and the one that catches more wins.
- **`git rev-parse --git-common-dir` is not used at all**, which is fortunate: run verbatim it
  returns the *relative* `.git` from a main clone and an *absolute* path from a linked worktree, so
  the same repository yields two different keys — two supervisors passing each other silently,
  which is precisely what the lock exists to refuse — while every main clone on the host collides on
  the single key `.git`. Both bugs were live in the spike.
- **Run state stops deriving from the script's own location.** `GRIND_ROOT = __file__/../..` cannot
  survive a shipped binary (ADR-0005). Outside a checkout, *never committed*
  ([#8](https://github.com/FlorianRiquelme/grind/issues/8)) holds structurally rather than by
  `.gitignore`.
- **`GRIND_REPO_PATH` and `GRIND_CLAUDE_BIN` are removed.** Both existed to override a guess, and
  there is no guess left.

## No override, and that is the point

There is no `GRIND_HOME`. The tempting argument for one is testability, and ADR-0007 already
dissolved it: the classifier is pure and takes literals, `world` does the `test -d`, so a
precondition test is three strings rather than a temp directory.

The argument against is this ADR's own subject. A lock is mutual exclusion only if every process
that can dispatch resolves the same directory. `$TMPDIR` was unsafe *because it is an environment
variable two launch contexts can disagree about, silently, toward proceeding* — and `GRIND_HOME`
is that same mechanism moved to the root of the tree, where a systemd unit setting it and a shell
not setting it would see different repos, different Run state and different locks, each internally
consistent and both wrong.

`$HOME` is the only variable. A unit with a different `User=` fails loudly: no repos directory,
`die`. The resolved paths land in the record, as `worktree`, `plugin_dir` and `claude_bin` already
do, so a re-entry under a different `$HOME` is visible rather than silent.

## The convention this must survive

This rule is anti-idiomatic, so per ADR-0006 it needs a carrier, and prose is the weakest one —
hence an ADR rather than a comment.

The idiom is XDG: `~/.local/share/grind` for state, `~/.cache/grind` for caches, and
`$XDG_RUNTIME_DIR` for locks. An agent applying it is deciding nothing — it is ADR-0006's
**convention** mode exactly — and it would put the lock back into a systemd-managed per-session
directory, restoring the `PrivateTmp` bug in the name of tidiness, aimed at the one property this
decision exists to protect.

**Do not split `~/.grind/` across XDG directories.** The single root is the mechanism, not a
stylistic preference.

## Costs

- Exercising the provisioning flow on the laptop writes to the real `~/.grind/`.
- Two isolated Grind instances on one host are impossible. Deliberate — single operator
  (`STRATEGY.md`).
- The one real `run.json` the four metrics have currently sits at `<checkout>/.grind/runs/`.
  Migrating it is not decided here; it is a term of the open question *whether the new base reads
  existing Run state*.
- Every host fact is now a filesystem fact, so a host is provisioned by making paths exist. That is
  the intended shape, but it means a mistyped symlink target is a valid directory entry — which is
  why *not a shim* is asserted loudly rather than filtered for.

## ADR-0002 is amended alongside this

The plugin version is **frozen per Run at dispatch** rather than pinned per Job. Separate decision,
same ticket; see the amendment on ADR-0002.

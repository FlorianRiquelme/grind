// Interactive-session mirror of DENIED_TOOLS (src/attempt.rs). Grind's headless Runs are
// gated in-process; this hook extends the same safety spirit to human/agent contributor
// sessions. Unlike DENIED_TOOLS it must NOT block `sh -c`, `bash -c`, `eval`.
type ToolCallEvent = { toolName?: unknown; input?: unknown };
type ToolCallDecision = { block?: boolean; reason?: string };
type HookAPI = {
  on(event: "tool_call", h: (e: ToolCallEvent) => ToolCallDecision | undefined | Promise<ToolCallDecision | undefined>): unknown;
};

const REASON_PREFIX =
  'AGENTS.md "DENIED_TOOLS in src/attempt.rs is a safety property" — "A Run must never merge its own PR, force-push, hard-reset, rebase, or delete a branch". This interactive session mirrors that rule: ';

/** Split a command line into subcommand candidates on `&&`, `;`, `|`. */
function splitSubcommands(command: string): string[] {
  return command
    .split(/&&|;|\|/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/** Fold subshell contents (`$(...)`, backticks, bare `( ... )`) into extra candidates. */
function foldSubshells(command: string): string[] {
  const extra: string[] = [];
  for (const m of command.matchAll(/\$\(([^()]*)\)/g)) extra.push(m[1]);
  for (const m of command.matchAll(/`([^`]*)`/g)) extra.push(m[1]);
  for (const m of command.matchAll(/(?<![)\]])\(([^()]*)\)/g)) extra.push(m[1]);
  return extra.flatMap((inner) => splitSubcommands(inner));
}

function isForbidden(sub: string): boolean {
  const tokens = sub.split(/\s+/).filter(Boolean);
  if (tokens[0] !== "git" && tokens[0] !== "gh") return false;

  if (tokens[0] === "gh") {
    if (tokens[1] === "pr" && tokens[2] === "merge") return true;
    if (tokens[1] !== "api") return false;
    // gh api: denied only when args contain 'merge' or 'DELETE'
    return tokens.slice(2).some((a) => a === "merge" || a === "DELETE");
  }

  // git
  const rest = tokens.slice(1);
  if (rest.some((a) => a === "--force-with-lease")) return true;
  if (rest.includes("update-ref")) return true;

  if (rest[0] === "push") {
    const pargs = rest.slice(1);
    if (pargs.some((a) => ["--force", "-f", "--delete", "--mirror", "--prune"].includes(a))) return true;
    if (pargs.some((a) => a.includes(":") || a.includes("+"))) return true;
    return false;
  }
  if (rest[0] === "reset") return rest.includes("--hard");
  if (rest[0] === "rebase") return true;
  if (rest[0] === "checkout") return rest[1] === "main";
  if (rest[0] === "switch") return rest[1] === "main";
  if (rest[0] === "branch") return rest.includes("-D") || rest.includes("--delete");
  return false;
}

export default function guard(pi: HookAPI): void {
  pi.on("tool_call", (event: ToolCallEvent): ToolCallDecision | undefined => {
    if (!(typeof event.toolName === "string" && event.toolName.toLowerCase() === "bash")) return undefined;
    const input = event.input as { command?: unknown } | null | undefined;
    const command = input?.command;
    if (typeof command !== "string") return undefined;

    const candidates = [...splitSubcommands(command), ...foldSubshells(command)];
    for (const sub of candidates) {
      if (isForbidden(sub)) {
        return {
          block: true,
          reason:
            REASON_PREFIX +
            `"${sub}" matches a denied destructive git/gh form (see DENIED_TOOLS in src/attempt.rs). ` +
            "Never merge a PR (gh pr merge, gh api merge), force-push or delete refs (git push --force/-f/--delete/--mirror/--prune, refspec ':' or '+'), hard-reset (git reset --hard), rebase, check out or switch to main, delete a branch (git branch -D/--delete), use --force-with-lease, or run git update-ref.",
        };
      }
    }
    return undefined;
  });
}

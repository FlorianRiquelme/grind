import { readFileSync } from "node:fs";
interface ToolCallEvent { toolName?: unknown; input?: unknown }
interface ToolCallDecision { block?: boolean; reason?: string }
interface HookAPI {
  on(event: "tool_call", h: (e: ToolCallEvent) => ToolCallDecision | undefined | Promise<ToolCallDecision | undefined>): unknown;
}

const EDIT_TOOLS = ["edit", "write", "multiedit", "str_replace", "str_replace_based_edit_tool", "replace", "save_file"];

const REASON =
  "AGENTS.md \"Code comments\": source carries no inline comments in .rs files — only doc comments " +
  "(///, //!) on items are allowed. This edit adds new inline comment(s). Put the durable prose in " +
  "docs/agents/code-rationale.md, or an ADR if it decides something, and ship the code comment-free.";

function asString(v: unknown): string | undefined {
  return typeof v === "string" && v.length > 0 ? v : undefined;
}

function extractPath(input: Record<string, unknown>): string | undefined {
  for (const key of ["file_path", "path", "notebook_path"]) {
    const p = asString(input[key]);
    if (p) return p;
  }
  return undefined;
}

function lineFlags(line: string): string[] {
  const trimmed = line.trimStart();
  if (trimmed.startsWith("///") || trimmed.startsWith("//!")) return [];
  const flags: string[] = [];
  if (trimmed.startsWith("//")) flags.push("//");
  if (trimmed.startsWith("/*")) flags.push("/*");
  return flags;
}

function flagMultiset(text: string): string[] {
  const flags: string[] = [];
  for (const line of text.split("\n")) flags.push(...lineFlags(line));
  flags.sort();
  return flags;
}

/** Multiset diff: how many of `proposed` are NOT consumed by `baseline` occurrences. */
function multisetDiff(proposed: string[], baseline: string[]): string[] {
  const remaining = [...baseline];
  const leftover: string[] = [];
  for (const f of proposed) {
    const i = remaining.indexOf(f);
    if (i >= 0) remaining.splice(i, 1);
    else leftover.push(f);
  }
  return leftover;
}

export default function guard(pi: HookAPI): void {
  pi.on("tool_call", async (event: ToolCallEvent): Promise<ToolCallDecision | undefined> => {
    const toolName = typeof event.toolName === "string" ? event.toolName.toLowerCase() : "";
    if (!EDIT_TOOLS.includes(toolName)) return undefined;
    if (typeof event.input !== "object" || event.input === null) return undefined;
    const input = event.input as Record<string, unknown>;

    const path = extractPath(input);
    if (!path || !path.endsWith(".rs")) return undefined;

    // Proposed content; if absent, we cannot scope the change — allow.
    const proposed =
      asString(input.content) ?? asString(input.new_string) ?? asString(input.new_str);
    if (proposed === undefined) return undefined;

    // Baseline must cover the same span the proposal replaces: for full-file writes
    // (`content`) that is the disk bytes; for fragment edits (`new_string`) the
    // old_string. Disk read failure with no old string: fail-closed.
    const old = asString(input.old_string) ?? asString(input.old_str);
    let baselineFlags: string[];
    if (proposed === asString(input.content)) {
      try {
        baselineFlags = flagMultiset(readFileSync(path, "utf8"));
      } catch {
        if (old === undefined) {
          return {
            block: true,
            reason:
              "no-inline-comments hook: could not read baseline for " +
              path +
              " (disk read failed and no old_string present). " +
              REASON,
          };
        }
        baselineFlags = flagMultiset(old);
      }
    } else {
      if (old === undefined) return undefined;
      baselineFlags = flagMultiset(old);
    }

    const additions = multisetDiff(flagMultiset(proposed), baselineFlags);
    if (additions.length === 0) return undefined;
    const kinds = [...new Set([...additions])].map((f) => JSON.stringify(f)).join(", ");
    return {
      block: true,
      reason:
        "no-inline-comments hook: this edit would add " +
        additions.length +
        " new inline comment token(s) (" +
        kinds +
        ") to " +
        path +
        ". " +
        REASON,
    };
  });
}

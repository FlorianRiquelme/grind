interface ToolCallEvent { toolName?: unknown; input?: unknown }
interface ToolCallDecision { block?: boolean; reason?: string }
interface HookAPI {
  on(event: "tool_call", h: (e: ToolCallEvent) => ToolCallDecision | undefined | Promise<ToolCallDecision | undefined>): unknown;
}

const RAW_CARGO = /^(cargo)\s+(test|check|clippy)\b/;

const REASON =
  "AGENTS.md \"Verify entrypoint\": `just verify` is the single definition of checked — it runs " +
  "`cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and the musl zigbuild cross-build " +
  "of both shipping triples (ADR-0009). A raw `cargo` invocation is an incomplete green: it misses fmt, " +
  "the clippy -D warnings gate, and the cross-build. Run `just verify` instead.";

function splitSubcommands(command: string): string[] {
  return command
    .split(/&&|;|\|/g)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

export default function guard(pi: HookAPI): void {
  pi.on("tool_call", async (event: ToolCallEvent): Promise<ToolCallDecision | undefined> => {
    const toolName = typeof event.toolName === "string" ? event.toolName.toLowerCase() : "";
    if (toolName !== "bash") return undefined;
    if (typeof event.input !== "object" || event.input === null) return undefined;
    const input = event.input as Record<string, unknown>;
    const command = typeof input.command === "string" ? input.command : undefined;
    if (!command) return undefined;

    for (const sub of splitSubcommands(command)) {
      if (RAW_CARGO.test(sub)) {
        return { block: true, reason: "prefer-just-verify hook: direct `" + sub + "`. " + REASON };
      }
    }
    return undefined;
  });
}

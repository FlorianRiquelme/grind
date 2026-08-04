// Spike: can Rust supervise a `claude -p` child that dies mid-run and be re-entered, the
// way `bin/grind`'s invoke()/supervise() do today? See ../FINDINGS.md for the verdict.
//
// Never invokes the real `claude` binary — every child here is a small shell script under
// fake/ that reproduces one of the death shapes seen in a real Run (see
// .grind/runs/20260802-105828-snapper-21/run.json).

use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

// Mirrors bin/grind's DENIED_TOOLS. A Run must never merge its own PR, force-push, or
// rewrite history out from under the human — see the constraint note in grind's CLAUDE.md.
const DENIED_TOOLS: [&str; 7] = [
    "Bash(gh pr merge*)",
    "Bash(git push --force*)",
    "Bash(git push -f*)",
    "Bash(git reset --hard*)",
    "Bash(git rebase*)",
    "Bash(git checkout main*)",
    "Bash(git branch -D*)",
];

#[allow(dead_code)] // spike: some fields exist for parity with bin/grind's attempt record but aren't all read back
struct Attempt {
    n: usize,
    mode: &'static str,
    argv: Vec<String>,
    exit_code: Option<i32>,
    raw_written: bool,
    raw_len: usize,
    parse_ok: bool,
    done_promise: bool,
    rate_limited: bool,
    classification: &'static str,
}

#[derive(Debug, PartialEq)]
enum Terminal {
    Completed,
    Exhausted,
}

/// Same argv shape as bin/grind's `invoke()`: attempt 1 dispatches with `--session-id`,
/// every later attempt resumes the same session id with `--resume`. Everything else is
/// identical between the two — the flags are the only thing that should differ.
fn build_argv(session_id: &str, resuming: bool, plugin_dir: &str) -> Vec<String> {
    let mut cmd: Vec<String> = vec![
        "claude".into(),
        "-p".into(),
        "--output-format".into(),
        "json".into(),
        "--permission-mode".into(),
        "bypassPermissions".into(),
    ];
    if resuming {
        cmd.push("--resume".into());
        cmd.push(session_id.into());
    } else {
        cmd.push("--session-id".into());
        cmd.push(session_id.into());
    }
    cmd.push("--plugin-dir".into());
    cmd.push(plugin_dir.into());
    cmd.push("--disallowedTools".into());
    cmd.extend(DENIED_TOOLS.iter().map(|s| s.to_string()));
    cmd
}

/// Port of bin/grind's `is_rate_limited`, minus the regex it uses
/// (`rate.?limit|usage limit|too many requests|quota exceeded|resets? at|429`).
/// See FINDINGS.md for how this differs from the real pattern.
fn is_rate_limited(v: &Value) -> bool {
    if !v.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
        return false;
    }
    let mut blob = String::new();
    for k in ["result", "api_error_status", "terminal_reason", "subtype"] {
        if let Some(s) = v.get(k).and_then(Value::as_str) {
            blob.push(' ');
            blob.push_str(s);
        }
    }
    let hay = blob.to_lowercase();
    const NEEDLES: [&str; 8] = [
        "rate limit",
        "ratelimit",
        "rate-limit",
        "usage limit",
        "too many requests",
        "quota exceeded",
        "reset at",
        "429",
    ];
    NEEDLES.iter().any(|n| hay.contains(n))
}

/// One invocation of a fake child. The ordering that matters: raw stdout hits disk
/// (`fs::write` on `stdout_path`) unconditionally, before parsing is even attempted — and
/// parsing itself runs inside `catch_unwind` so a parser that panics on garbage input still
/// cannot retroactively un-write bytes already on disk.
fn invoke(
    dir: &Path,
    n: usize,
    resuming: bool,
    session_id: &str,
    plugin_dir: &str,
    fake_child: &Path,
    prompt: &str,
    chaos_parse: bool,
) -> Attempt {
    let argv = build_argv(session_id, resuming, plugin_dir);
    println!(
        "  attempt {n} ({}) argv: {:?}  [fake child: {}]",
        if resuming { "resume" } else { "dispatch" },
        argv,
        fake_child.display()
    );

    fs::write(dir.join(format!("attempt-{n}.prompt.txt")), prompt).expect("write prompt");

    let stderr_path = dir.join(format!("attempt-{n}.stderr.log"));
    let stderr_file = fs::File::create(&stderr_path).expect("create stderr log");

    let mut child = Command::new(fake_child)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr_file)
        .spawn()
        .expect("spawn fake child");

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
        // drop closes stdin so the child sees EOF
    }

    let output = child.wait_with_output().expect("wait for fake child");
    let raw = output.stdout;
    let raw_len = raw.len();

    // --- RAW HITS DISK BEFORE ANYTHING PARSES IT. This line is the whole claim. ---
    let stdout_path = dir.join(format!("attempt-{n}.stdout.json"));
    fs::write(&stdout_path, &raw).expect("raw must land on disk unconditionally");
    let raw_written = stdout_path.exists() && fs::metadata(&stdout_path).unwrap().len() as usize == raw_len;

    let raw_str = String::from_utf8_lossy(&raw).into_owned();

    // Parsing runs strictly after the write above, and is wrapped in catch_unwind. For the
    // "prove it the hard way" case we deliberately use a parser that panics (.unwrap()) on
    // bad input instead of bin/grind's graceful except/degrade — the write already
    // happened, so the panic changes nothing about what is on disk.
    let raw_for_parse = raw_str.clone();
    let parsed: std::thread::Result<(bool, Value)> = std::panic::catch_unwind(move || {
        if chaos_parse {
            // Deliberately panics on bad JSON, instead of bin/grind's graceful degrade.
            let v = serde_json::from_str::<Value>(&raw_for_parse).unwrap();
            (true, v)
        } else {
            match serde_json::from_str::<Value>(&raw_for_parse) {
                Ok(v) => (true, v),
                Err(_) => (
                    false,
                    serde_json::json!({
                        "is_error": true,
                        "subtype": "unparseable-output",
                        "result": raw_for_parse.chars().rev().take(2000).collect::<String>().chars().rev().collect::<String>(),
                    }),
                ),
            }
        }
    });

    let (parse_ok, value) = match parsed {
        Ok((ok, v)) => (ok, v),
        Err(_) => (
            false,
            serde_json::json!({
                "is_error": true,
                "subtype": "unparseable-output",
                "result": raw_str.chars().rev().take(2000).collect::<String>().chars().rev().collect::<String>(),
            }),
        ),
    };

    let result_text = value.get("result").and_then(Value::as_str).unwrap_or("");
    let done_promise = result_text.contains("<promise>DONE</promise>");
    let rate_limited = is_rate_limited(&value);

    let classification = if done_promise {
        "completed"
    } else if rate_limited {
        "rate_limited"
    } else {
        "died"
    };

    println!(
        "    -> exit={:?} raw_written={} raw_len={} parse_ok={} classification={}",
        output.status.code(),
        raw_written,
        raw_len,
        parse_ok,
        classification
    );

    Attempt {
        n,
        mode: if resuming { "resume" } else { "dispatch" },
        argv,
        exit_code: output.status.code(),
        raw_written,
        raw_len,
        parse_ok,
        done_promise,
        rate_limited,
        classification,
    }
}

/// Mirrors bin/grind's `supervise()`: keep re-entering until DONE, out of attempts, or a
/// rate limit (which sleeps and continues rather than burning the budget). Exhaustion is
/// its own terminal state, not an error.
fn supervise(
    dir: &Path,
    session_id: &str,
    plugin_dir: &str,
    max_attempts: usize,
    fake_for_attempt: impl Fn(usize) -> PathBuf,
    rate_limit_sleep: Duration,
    chaos_parse_attempt: Option<usize>,
) -> (Vec<Attempt>, Terminal) {
    let mut attempts = Vec::new();
    loop {
        let n = attempts.len() + 1;
        if n > max_attempts {
            return (attempts, Terminal::Exhausted);
        }
        let resuming = n > 1;
        let fake_child = fake_for_attempt(n);
        let prompt = if resuming {
            "You were interrupted mid-run and have just been resumed.".to_string()
        } else {
            "You are a Grind Run, executing unattended with no human present.".to_string()
        };
        let chaos = chaos_parse_attempt == Some(n);
        let att = invoke(dir, n, resuming, session_id, plugin_dir, &fake_child, &prompt, chaos);
        let done = att.classification == "completed";
        let rate_limited = att.classification == "rate_limited";
        attempts.push(att);

        if done {
            return (attempts, Terminal::Completed);
        }
        if rate_limited {
            println!(
                "    rate limited — sleeping {:?} (spike-shortened from the real 1800s), then re-entering",
                rate_limit_sleep
            );
            std::thread::sleep(rate_limit_sleep);
            continue;
        }
        println!("    run ended without a DONE promise — re-entering at the stage that died");
    }
}

fn fake(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fake").join(name)
}

fn scenario_dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join(".spike-out").join(name);
    fs::create_dir_all(&d).expect("create scenario dir");
    d
}

fn main() {
    println!("=== scenario A: the real run reproduced (snapper-21 shape, 5 attempts) ===");
    let dir_a = scenario_dir("a-real-run-shape");
    let sequence_a = [
        fake("half_json.sh"),
        fake("subtle_error.sh"),
        fake("silent.sh"),
        fake("success_no_done.sh"),
        fake("success_done.sh"),
    ];
    let (attempts_a, terminal_a) = supervise(
        &dir_a,
        "2e2aea5a-48b6-48bb-9dcb-7ca6903c0200",
        "/fake/plugin/dir",
        8,
        |n| sequence_a[n - 1].clone(),
        Duration::from_secs(0),
        None,
    );
    println!("  terminal state: {terminal_a:?}\n");
    assert_eq!(terminal_a, Terminal::Completed);
    assert_eq!(attempts_a.len(), 5);
    assert_eq!(attempts_a[0].mode, "dispatch");
    assert!(attempts_a[1..].iter().all(|a| a.mode == "resume"));
    assert!(attempts_a[0].argv.contains(&"--session-id".to_string()));
    assert!(!attempts_a[0].argv.contains(&"--resume".to_string()));
    for a in &attempts_a[1..] {
        assert!(a.argv.contains(&"--resume".to_string()));
        assert!(!a.argv.contains(&"--session-id".to_string()));
    }
    // The subtle case the ADR calls out: attempts 1-3 all raw-wrote something (even the
    // silent one wrote zero bytes, which is itself a recorded fact, not a lost one).
    for a in &attempts_a[..3] {
        assert!(a.raw_written, "raw must be on disk even for a dying attempt");
        assert_eq!(a.classification, "died");
    }
    assert!(attempts_a[3].classification == "died"); // is_error:false, but no DONE yet
    assert_eq!(attempts_a[4].classification, "completed");

    // Prove ordering the hard way: re-read attempt 1's raw file independently of the
    // in-memory copy the supervisor used, after a parse that would have panicked.
    let raw_on_disk = fs::read(dir_a.join("attempt-1.stdout.json")).expect("raw file must exist");
    println!(
        "  hard proof: attempt-1.stdout.json on disk = {} bytes, ends with {:?}",
        raw_on_disk.len(),
        String::from_utf8_lossy(&raw_on_disk[raw_on_disk.len().saturating_sub(24)..])
    );
    assert_eq!(raw_on_disk.len(), attempts_a[0].raw_len);

    println!("=== scenario B: parse panics on the SAME raw, raw file is still intact ===");
    let dir_b = scenario_dir("b-chaos-parse");
    let (attempts_b, terminal_b) = supervise(
        &dir_b,
        "chaos-session".into(),
        "/fake/plugin/dir",
        1,
        |_| fake("half_json.sh"),
        Duration::from_secs(0),
        Some(1), // force attempt 1's parse to panic on truncated JSON
    );
    println!("  terminal state: {terminal_b:?}");
    assert_eq!(terminal_b, Terminal::Exhausted); // 1 attempt, died, budget of 1 exhausted
    assert!(!attempts_b[0].parse_ok, "the truncated JSON should fail to parse");
    let raw_on_disk_b = fs::read(dir_b.join("attempt-1.stdout.json")).expect("raw file must exist");
    println!(
        "  the parser panicked (caught by catch_unwind) — raw on disk anyway: {} bytes\n",
        raw_on_disk_b.len()
    );
    assert_eq!(raw_on_disk_b.len(), attempts_b[0].raw_len);
    assert!(attempts_b[0].raw_written);

    println!("=== scenario C: killed mid-write (SIGKILL), no clean exit code at all ===");
    let dir_c = scenario_dir("c-sigkilled");
    let (attempts_c, terminal_c) = supervise(
        &dir_c,
        "sigkill-session".into(),
        "/fake/plugin/dir",
        1,
        |_| fake("sigkilled.sh"),
        Duration::from_secs(0),
        None,
    );
    println!("  terminal state: {terminal_c:?}");
    println!("  exit_code reported: {:?} (no clean code — the kill signal ate it)\n", attempts_c[0].exit_code);
    assert_eq!(terminal_c, Terminal::Exhausted);
    assert!(attempts_c[0].raw_written && attempts_c[0].raw_len > 0, "partial bytes must survive the kill");
    assert!(!attempts_c[0].parse_ok);

    println!("=== scenario D: rate-limited, then recovers on re-entry ===");
    let dir_d = scenario_dir("d-rate-limited");
    let sequence_d = [fake("rate_limited.sh"), fake("success_done.sh")];
    let (attempts_d, terminal_d) = supervise(
        &dir_d,
        "rate-limit-session".into(),
        "/fake/plugin/dir",
        8,
        |n| sequence_d[n - 1].clone(),
        Duration::from_millis(200), // stands in for the real 1800s sleep
        None,
    );
    println!("  terminal state: {terminal_d:?}\n");
    assert_eq!(terminal_d, Terminal::Completed);
    assert_eq!(attempts_d[0].classification, "rate_limited");
    assert_eq!(attempts_d[1].classification, "completed");

    println!("=== scenario E: budget exhaustion is its own terminal state, not a crash ===");
    let dir_e = scenario_dir("e-exhausted");
    let (attempts_e, terminal_e) = supervise(
        &dir_e,
        "exhaustion-session".into(),
        "/fake/plugin/dir",
        3,
        |_| fake("subtle_error.sh"), // never once completes
        Duration::from_secs(0),
        None,
    );
    println!("  terminal state: {terminal_e:?}");
    println!("  attempts made: {} (all died, none rate-limited, none completed)\n", attempts_e.len());
    assert_eq!(terminal_e, Terminal::Exhausted);
    assert_eq!(attempts_e.len(), 3);
    assert!(attempts_e.iter().all(|a| a.classification == "died"));

    println!("=== scenario F: silent child (writes nothing) — a died attempt, not a panic ===");
    let dir_f = scenario_dir("f-silent");
    let (attempts_f, terminal_f) = supervise(
        &dir_f,
        "silent-session".into(),
        "/fake/plugin/dir",
        1,
        |_| fake("silent.sh"),
        Duration::from_secs(0),
        None,
    );
    println!("  terminal state: {terminal_f:?}");
    println!("  raw_len={} raw_written={} (zero bytes is itself a recorded fact)\n", attempts_f[0].raw_len, attempts_f[0].raw_written);
    assert_eq!(terminal_f, Terminal::Exhausted);
    assert_eq!(attempts_f[0].raw_len, 0);
    assert!(attempts_f[0].raw_written);

    println!("all scenarios ran; nothing panicked out of main(). See FINDINGS.md.");
}

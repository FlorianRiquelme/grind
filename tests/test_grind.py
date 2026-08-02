#!/usr/bin/env python3
"""Tests for the parts of the supervisor that carry a safety property.

Pure functions only — no network, no `claude` invocations. Run: python3 tests/test_grind.py

The dispatch and re-entry paths are exercised by hand against a scratch repo; what is
guarded here is the logic whose silent failure would be expensive: mistaking a rate limit
for a crash (burning attempts instead of sleeping), missing a rate limit (sleeping through
a real bug), and — the one that matters most — failing to notice that a step of
`just verify` was trimmed until it went green.
"""

import pathlib
import sys
import tempfile
from importlib.machinery import SourceFileLoader

g = SourceFileLoader("grind", str(pathlib.Path(__file__).parent.parent / "bin" / "grind")).load_module()

FAILURES = []


def check(label, got, want):
    if got != want:
        FAILURES.append(f"{label}: want {want!r}, got {got!r}")
    print(f"  {'ok  ' if got == want else 'FAIL'} {label}")


# --- rate limit: sleep long and re-enter, never a pre-flight quota check ---------------

print("is_rate_limited")
for label, result, want in [
    ("usage limit reached", {"is_error": True, "result": "Claude AI usage limit reached|1754"}, True),
    ("http 429", {"is_error": True, "api_error_status": 429, "result": "x"}, True),
    ("too many requests", {"is_error": True, "result": "Too Many Requests"}, True),
    ("resets at", {"is_error": True, "terminal_reason": "limit resets at 3pm"}, True),
    ("ordinary crash is not a limit", {"is_error": True, "result": "TypeError: undefined"}, False),
    ("success mentioning limits", {"is_error": False, "result": "rate limit in passing"}, False),
]:
    check(label, g.is_rate_limited(result), want)


# --- the verify contract: report a gutted gate, never enforce it (ADR-0003) ------------

INTACT = """verify: fmt clippy test typecheck lint fe-test build
fmt:
    cargo fmt --check
clippy:
    cargo clippy -- -D warnings
test:
    cargo test
typecheck:
    npx tsc --noEmit
lint:
    npx eslint .
fe-test:
    npx vitest run
build:
    npm run tauri build -- --debug --no-bundle
"""


def contract_for(justfile_text=None, package_json=None):
    with tempfile.TemporaryDirectory() as d:
        wt = pathlib.Path(d)
        if justfile_text is not None:
            (wt / "justfile").write_text(justfile_text)
        if package_json is not None:
            (wt / "package.json").write_text(package_json)
        return g.check_verify_contract(wt)


print("check_verify_contract")
check("intact justfile has nothing missing", contract_for(INTACT)["missing"], [])
check("no justfile means every step missing", contract_for()["missing"], [k for k, _ in g.VERIFY_CONTRACT])
check(
    "clippy stripped of -D warnings is caught",
    contract_for(INTACT.replace("cargo clippy -- -D warnings", "cargo clippy"))["missing"],
    ["rust-clippy"],
)
check(
    "vitest replaced by a no-op is caught",
    contract_for(INTACT.replace("npx vitest run", "true"))["missing"],
    ["ts-test"],
)
check(
    "build assertion losing --no-bundle is caught",
    contract_for(INTACT.replace("--debug --no-bundle", "--debug"))["missing"],
    ["build-assertion"],
)
# A justfile may legitimately delegate to npm scripts, so package.json counts as evidence.
check(
    "steps delegated to npm scripts still count",
    contract_for(
        INTACT.replace("npx vitest run", "npm run test"),
        '{"scripts":{"test":"vitest run"}}',
    )["missing"],
    [],
)

print()
if FAILURES:
    print(f"{len(FAILURES)} failure(s):")
    for f in FAILURES:
        print(f"  - {f}")
    sys.exit(1)
print("all passed")

# The one definition of checked (ADR-0009). CI runs this recipe and nothing else, so a step
# cannot exist in CI and not locally.
#
# `cargo test` alone still runs every test carrying a safety property — including the
# source-level topology carrier and the compile-fail carrier, both of which live under
# `tests/`. Reaching for that idiom is an incomplete green, never a false one.

default: verify

verify:
    cargo fmt --check
    cargo clippy -- -D warnings
    cargo test
    # The ship check. *Compiles on Linux* and *the shipped artifact builds* are different
    # claims, and the failure #30 reproduced was a binary that refused to start.
    cargo zigbuild --release --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl

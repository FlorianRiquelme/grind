//! The surface: the argument shapes, and the only thing that writes to stdout.

/// `grind --version` answers which copy of the binary is running — the only honest check on
/// the *step*-marked item that says where the binary lives (`docs/provisioned-host.md`).
pub fn run() {
    println!("grind {}", env!("CARGO_PKG_VERSION"));
}

// Collapse (a): a match on Observed<bool> that forgets the Unobservable arm.
//
// This is the exact shape of Python's `sh(..., check=False)` — a caller who only thinks
// about "did it say yes" and "did it say no" and never wrote a branch for "I couldn't ask".
// In Python that caller runs. In Rust it must not compile: see ../FINDINGS.md for the
// verbatim error captured by `rustc --edition 2021 forgot_unobservable.rs`.

enum Observed<T> {
    Present(T),
    Absent,
    Unobservable(String),
}

fn describe(o: Observed<bool>) -> &'static str {
    match o {
        Observed::Present(true) => "yes",
        Observed::Present(false) => "no",
        Observed::Absent => "absent",
        // Observed::Unobservable(_) => ... — deliberately omitted.
    }
}

fn main() {
    println!("{}", describe(Observed::Unobservable("network blip".into())));
}

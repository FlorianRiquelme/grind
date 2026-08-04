// Collapse (b): the "if let Present(x) else default" pattern that silently treats
// Unobservable the same as a genuine negative observation.
//
// Unlike forgot_unobservable.rs, THIS FILE COMPILES. That is the point, and the honest
// limit of what the type system buys here: `Observed<T>` forces you to exhaustively name
// the Unobservable case, but it cannot stop you from naming it and then doing the wrong
// thing with it — collapsing it into `false`, same as a real "no". A human still has to
// choose to route Unobservable into `Verdict::Uncorroborated` and not into `Incomplete`.
// See ../FINDINGS.md.

enum Observed<T> {
    Present(T),
    Absent,
    Unobservable(String),
}

fn is_true(o: &Observed<bool>) -> bool {
    // Compiles fine. Unobservable and Absent both fall through to `false` here — exactly
    // the collapse `sh(..., check=False)` performs by returning "" for both "no" and
    // "couldn't ask".
    if let Observed::Present(true) = o {
        true
    } else {
        false
    }
}

fn main() {
    let unobservable: Observed<bool> = Observed::Unobservable("gh pr view: timed out".into());
    let absent: Observed<bool> = Observed::Absent;

    // Both print `false`. A caller reading only this bool cannot tell them apart.
    println!("unobservable -> {}", is_true(&unobservable));
    println!("absent       -> {}", is_true(&absent));
}

//! A fifth completion signal, added and then dropped at the fold.
//!
//! The scratch crate this is compiled into has a fifth field added to `RawSignals`. Nothing in
//! this file needs to do anything: the failure is in `decide`'s own fold, which destructures
//! the struct with no `..` and no `field: _`, so the new field is `error[E0027]: pattern does
//! not mention field`.
//!
//! Adding the field *inside* the crate is what makes this expressible. From outside, an extra
//! field on a constructor is `E0560` and the fold is never reached at all.
//!
//! **The bypass is named, not patched.** rustc's own help text offers `..` and `field: _`, and
//! no clippy lint covers either. Taking one is a deliberate act, and deliberate acts are not
//! typeable.

use crate::decide::RawSignals;

pub(crate) fn a_fifth_signal_exists(signals: &RawSignals) -> bool {
    matches!(signals.pr_open, crate::observe::Observed::Present(true))
}

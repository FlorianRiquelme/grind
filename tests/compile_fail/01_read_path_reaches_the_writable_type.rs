//! A read path reaching for the writable record type.
//!
//! This is not a sneak-past. It is the import an agent writes **without deciding anything** —
//! adding a field to display, reaching for the type that already has the getter it wants. The
//! whole failure family is that `use RunRecord` is not a decision, it is an import.
//!
//! Compiled as a sibling of the record's owner, it is `error[E0603]: struct RunRecord is
//! private`, and rustc offers no fix. Compiled as a *child* of that owner it compiles clean,
//! which is why the arrangement is the carrier and the nesting is the hazard.

use crate::supervisor::RunRecord;

pub(crate) fn status_reaches_for_the_writable_type() -> Option<RunRecord> {
    None
}

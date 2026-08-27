//! Grind's base: one crate, twenty-one modules at the crate root.
//!
//! `world` is the sole namer of `std::process`, `std::fs` and `std::env`; `serve` is the
//! sole namer of `std::net` (ADR-0007, amended by ADR-0014). Everything else is pure —
//! effects are returned as values, so every decision is testable from literals with no
//! network.
//!
//! **`supervisor` and `view` are siblings and must stay siblings.** The writable record type
//! is private to `supervisor`; privacy only bites between siblings, so a child module reaches
//! its ancestor's private items and **compiles clean**. Nesting them under a shared parent —
//! a `record/` directory, a `types` module — is the tidy-up that silently withdraws the
//! carrier. `tests/topology.rs` is what notices.

pub mod attempt;
pub mod claude;
pub mod cli;
pub mod decide;
pub mod job;
pub mod learnings;
pub mod native;
pub mod net;
pub mod observe;
pub mod omp;
pub mod page;
pub mod policy;
pub mod render;
pub mod rung;
pub mod runner;
pub mod script;
pub mod serve;
pub mod style;
pub mod supervisor;
pub mod tools;
pub mod view;
pub mod world;

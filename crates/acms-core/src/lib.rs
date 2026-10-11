//! ACMS core: document model, tools, inks, undo and compositor.
//!
//! This crate must never depend on a UI toolkit or do file IO (see `PLAN.md`, section 3).

pub mod command;

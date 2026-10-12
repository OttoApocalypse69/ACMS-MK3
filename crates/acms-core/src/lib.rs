//! ACMS core: document model, tools, inks, undo and compositor.
//!
//! This crate must never depend on a UI toolkit or do file IO (see `PLAN.md`, section 3).

pub mod color;
pub mod command;
pub mod compositor;
pub mod document;
pub mod edit;
pub mod editor;
pub mod geom;
pub mod image;
pub mod palette;
pub mod pattern;
pub mod selection;
pub mod sprite;
pub mod stroke;
pub mod tool;
pub mod tools;

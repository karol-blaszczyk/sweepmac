//! Presentation layer for the sweepmac window: visual tokens in [`style`] and
//! stateless components in [`widgets`]. Kept out of the library so the
//! dependency-free CLI never sees egui.

pub mod style;
pub mod widgets;

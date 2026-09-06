#![deny(clippy::undocumented_unsafe_blocks)]
//! voidframe-engine — the OS-independent core of the VOIDFRAME benchmark engine.
//! `docs/superpowers/plans/2026-09-01-m1-phase-1-engine-foundation.md`: model,
//! journal, store, SystemController trait + Mock/DryRun, mutations, preflight.

pub mod capture;
pub mod cs2;
pub mod error;
pub mod journal;
pub mod mock_harness;
pub mod model;
pub mod mutation;
pub mod paths;
pub mod preflight;
pub mod run;
pub mod stats;
pub mod store;
pub mod system;

pub use error::{Error, Result};

//! Rust support for the [cnf](https://github.com/canonical/cnf)
//! configuration file format.

mod cnfembed;

pub use cnfembed::{escape, to_json, Error, Options};

//! Works out what to install and explains failure
//!
//! PubGrub: conflict-driven clause learning, specialized to version solving.
//! Every fact is an incompatibility with a recorded cause, so `incompat.rs`
//! carries a `Cause` and `solve.rs` keeps it.
//!
//! The solver asks a `Universe` and knows nothing of files or networks, so
//! dependency graphs are unit tests.

pub mod catalogue;
pub mod explain;
pub mod incompat;
pub mod partial;
pub mod solve;
pub mod term;
pub mod universe;

pub use catalogue::{Catalogue, Offer};
pub use explain::report;
pub use incompat::{Cause, Incompat, Store};
pub use solve::{solve, Failure, Solution};
pub use term::Term;
pub use universe::{Pkg, Universe};

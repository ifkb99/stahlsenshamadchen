//! The measurement harness: the machinery `examples/balance.rs` is built on.
//!
//! This is here rather than in the example for one reason. The example makes
//! several *checkable* claims — that no printed number moves with `--jobs`,
//! that `--sweep balance.x=a,b` is the same thing as hand-editing `mod.json`,
//! that the skill-gap arena is symmetric, that a run's accounting folds the
//! same way whatever order the battles finished in. Every one of those was
//! verified once, by hand, and recorded in prose, because `cargo test` does
//! not build tests in examples and nothing in the tree could reach this code.
//!
//! That made the instrument the least-tested code in the repository and, given
//! that the house style is quoting its numbers in commit messages, the most
//! load-bearing. The four claims are now four tests in `tests/harness.rs`.
//!
//! **What lives here is machinery, not reporting.** Anything that decides what
//! a table looks like, what a column means, or which vehicles to duel stayed
//! in the example, where it belongs: the split is between the parts that have
//! a contract and the parts that have a layout.
//!
//! It is a normal module rather than `#[cfg(test)]` because the example is a
//! normal consumer of it, and because a mod author measuring their own content
//! wants exactly these pieces.

pub mod arena;
pub mod overrides;
pub mod parallel;
pub mod tally;

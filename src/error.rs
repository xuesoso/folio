//! Crate-wide `Result` alias.
//!
//! This is a binary, not a public library, so we use `anyhow` for ergonomic `?`
//! propagation and context. The `Document` / `ImageProtocol` trait seams are
//! expressed in terms of this alias so they stay backend-agnostic.

pub type Result<T> = anyhow::Result<T>;

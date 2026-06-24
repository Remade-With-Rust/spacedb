//! Errors for compute-to-data.

use thiserror::Error;

pub type QueryResult<T> = Result<T, QueryError>;

#[derive(Debug, Error)]
pub enum QueryError {
    /// The WASM module failed to compile.
    #[error("module compile failed: {0}")]
    Compile(String),

    /// The module failed to instantiate.
    #[error("instantiation failed: {0}")]
    Instantiate(String),

    /// The module is missing a required ABI export (`memory`, `alloc`, `run`).
    #[error("module is missing required export `{0}`")]
    MissingExport(&'static str),

    /// Execution trapped — out of fuel, over the memory limit, or an abort. A trap
    /// is deterministic: every honest host traps at the same point.
    #[error("execution trapped (out of fuel / memory limit / abort): {0}")]
    Trap(String),

    /// The module returned something that violates the function ABI.
    #[error("module violated the function ABI: {0}")]
    Abi(String),

    /// Fuel accounting was unavailable.
    #[error("fuel accounting unavailable: {0}")]
    Fuel(String),
}

//! Errors for the vector index.

use thiserror::Error;

pub type VectorResult<T> = Result<T, VectorError>;

#[derive(Debug, Error)]
pub enum VectorError {
    /// A vector's dimension didn't match the index.
    #[error("dimension mismatch: index is {expected}-d, got {got}-d")]
    DimMismatch { expected: usize, got: usize },

    /// Retrieval was attempted without an authorized capability.
    #[error("retrieval denied: no authorized capability")]
    Denied,
}

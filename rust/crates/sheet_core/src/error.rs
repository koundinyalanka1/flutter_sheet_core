//! Errors reading or writing a workbook.

/// Why a workbook could not be read or written, worded for people.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SheetError {
    #[error("{0}")]
    Corrupt(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("This workbook is protected with a password.")]
    Encrypted,
    #[error("{0}")]
    TooLarge(String),
}

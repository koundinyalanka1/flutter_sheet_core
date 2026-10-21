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

impl From<doc_opc::OpcError> for SheetError {
    fn from(e: doc_opc::OpcError) -> Self {
        match e {
            doc_opc::OpcError::Zip(doc_zip::ZipError::TooLarge(_)) => Self::TooLarge(e.to_string()),
            _ => Self::Corrupt(e.to_string()),
        }
    }
}

impl From<doc_xml::XmlError> for SheetError {
    fn from(e: doc_xml::XmlError) -> Self {
        if e.is_too_large() { Self::TooLarge(e.to_string()) } else { Self::Corrupt(e.to_string()) }
    }
}

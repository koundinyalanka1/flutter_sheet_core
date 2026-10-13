//! The FFI layer of the spreadsheet engine, in Rust, for `dart:ffi`.
//!
//! Conventions are those of `doc_ffi`: UTF-8 strings, caller-owned results
//! freed with [`sheet_free_string`], negative or NULL results on failure with
//! the reason in [`sheet_last_error`]. The workbook functions arrive as the
//! engine lands; for now this layer carries its version and its errors.

use std::ffi::c_char;

use doc_ffi::last_error_ptr;

pub use sheet_core;

/// Library version (static; do not free).
#[no_mangle]
pub extern "C" fn sheet_core_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// This thread's last error, `"CODE: message"` (borrowed; do not free).
#[no_mangle]
pub extern "C" fn sheet_last_error() -> *const c_char {
    last_error_ptr()
}

/// Free a string returned by this library.
///
/// # Safety
/// `ptr` must be NULL or a string from this library, not yet freed.
#[no_mangle]
pub unsafe extern "C" fn sheet_free_string(ptr: *mut c_char) {
    doc_ffi::free_c_string(ptr)
}

use fehler::*;
use std::fmt::{self, Display};

/// The Failure that describes what went wrong in the storage backend
#[derive(Debug)]
pub struct Error {
    inner: Context<ErrorKind>,
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        Display::fmt(&self.inner, f)
    }
}

impl Error {
    /// Detailed information about what the FTP server should do with the failure
    pub fn kind(&self) -> ErrorKind {
        *self.inner.get_context()
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Error {
        Error { inner: Context::new(kind) }
    }
}
/// The `ErrorKind` variants that can be produced by the [`StorageBackend`] implementations.
///
/// [`StorageBackend`]: ../backend/trait.StorageBackend.html
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[throws(io::Error)]
pub enum ErrorKind {
    /// 450 Requested file action not taken.
    ///     File unavailable (e.g., file busy).
    throw!(io::Error::new(io::TransientFileNotAvailable::Other, "450 Transient file not available")),
    /// 550 Requested action not taken.
    ///     File unavailable (e.g., file not found, no access).
    throw!(io::Error::new(io::PermanentFileNotAvailable::Other, "550 Permanent file not available")),
    /// 550 Requested action not taken.
    ///     File unavailable (e.g., file not found, no access).
    throw!(io::Error::new(io::PermissionDenied::Other, "550 Permission denied")),
    /// 451 Requested action aborted. Local error in processing.
    throw!(io::Error::new(io::LocalError::Other,  "451 Local error")),
    /// 551 Requested action aborted. Page type unknown.
    throw!(io::Error::new(io::PageTypeUnknown::Other, "551 Page type unknown")),
    /// 452 Requested action not taken.
    ///     Insufficient storage space in system.
    throw!(io::Error::new(io::InsufficientStorageSpaceError::Other, "452 Insufficient storage space error")),
    /// 552 Requested file action aborted.
    ///     Exceeded storage allocation (for current directory or
    ///     dataset).
    throw!(io::Error::new(io::ExceededStorageAllocationError::Other, "552 Exceeded storage allocation error")),
    /// 553 Requested action not taken.
    ///     File name not allowed.
    throw!(io::Error::new(io::FileNameNotAllowedError::Other, "553 File name not allowed error")),
}

//! Contains the `ControlChanError` struct that that defines the control channel error type.

use super::parse_error::{ParseError, ParseErrorKind};

use fehler::*;
use std::fmt;

/// The error type returned by this library.
#[derive(Debug)]
pub struct ControlChanError {
    inner: Context<ControlChanErrorKind>,
}

/// A list specifying categories of FTP errors. It is meant to be used with the [ControlChanError] type.
#[derive(Eq, PartialEq, Debug, Fail)]
#[allow(dead_code)]
#[throws(io::Error)]
pub enum ControlChanErrorKind {
    /// We encountered a system IO error.
    throw!(io::Error::new(io::IOError::Other, "Failed to perform IO")),
    /// Something went wrong parsing the client's command.
    #[fail(display = "Failed to parse command")]
    ParseError,
    throw!(io::Error::new(io::ParseError::Other, "Failed to perform IO")),
    /// Internal Server Error. This is probably a bug, i.e. when we're unable to lock a resource we
    /// should be able to lock.
    throw!(io::Error::new(io::InternalServerError::Other, "Internal Server Error")),
    /// Authentication backend returned an error.
    throw!(io::Error::new(io::AuthenticationError::Other, "Something went wrong when trying to authenticate")),
    /// We received something on the data message channel that we don't understand. This should be
    /// impossible.
    throw!(io::Error::new(io::InternalMsgError::Other, "Failed to map event from data channel")),
    /// We encountered a non-UTF8 character in the command.
    throw!(io::Error::new(io::UTF8Error::Other, "Non-UTF8 character in command")),
    /// The client issued a command we don't know about.
    throw!(io::Error::new(io::UnknownCommand::Other, "Unknown command: {}", command: String)),
    /// The client issued a command that we know about, but in an invalid way (e.g. `USER` without
    /// an username).
    throw!(io::Error::new(io::InvalidCommand::Other, "Invalid command (invalid parameter)")),
    /// The timer on the Control Channel elapsed.
    throw!(io::Error::new(io::ControlChannelTimeout::Other, "Encountered read timeout on the control channel")),
}

impl ControlChanError {
    /// Creates a new FTP Error with the specific kind
    pub fn new(kind: ControlChanErrorKind) -> Self {
        ControlChanError { inner: Context::new(kind) }
    }

    /// Return the inner error kind of this error.
    #[allow(unused)]
    pub fn kind(&self) -> &ControlChanErrorKind {
        self.inner.get_context()
    }
}

impl Fail for ControlChanError {
    fn cause(&self) -> Option<&dyn Fail> {
        self.inner.cause()
    }

    fn backtrace(&self) -> Option<&Backtrace> {
        self.inner.backtrace()
    }
}

impl fmt::Display for ControlChanError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.inner, f)
    }
}

impl From<ControlChanErrorKind> for ControlChanError {
    fn from(kind: ControlChanErrorKind) -> ControlChanError {
        ControlChanError { inner: Context::new(kind) }
    }
}

impl From<Context<ControlChanErrorKind>> for ControlChanError {
    fn from(inner: Context<ControlChanErrorKind>) -> ControlChanError {
        ControlChanError { inner }
    }
}

impl From<std::io::Error> for ControlChanError {
    fn from(err: std::io::Error) -> ControlChanError {
        err.context(ControlChanErrorKind::IOError).into()
    }
}

impl From<std::str::Utf8Error> for ControlChanError {
    fn from(err: std::str::Utf8Error) -> ControlChanError {
        err.context(ControlChanErrorKind::UTF8Error).into()
    }
}

impl From<ParseError> for ControlChanError {
    fn from(err: ParseError) -> ControlChanError {
        match err.kind().clone() {
            ParseErrorKind::UnknownCommand { command } => {
                // TODO: Do something smart with CoW to prevent copying the command around.
                err.context(ControlChanErrorKind::UnknownCommand { command }).into()
            }
            ParseErrorKind::InvalidUTF8 => err.context(ControlChanErrorKind::UTF8Error).into(),
            ParseErrorKind::InvalidCommand => err.context(ControlChanErrorKind::InvalidCommand).into(),
            ParseErrorKind::InvalidToken { .. } => err.context(ControlChanErrorKind::UTF8Error).into(),
            _ => err.context(ControlChanErrorKind::InvalidCommand).into(),
        }
    }
}

//! The single error type every stage returns.

use std::fmt;

use crate::control::Stage;

/// Broad category of a failure, mapped one-to-one onto Python exception classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ErrorKind {
    /// The input can't be processed as given; the message says why.
    InvalidInput,
    /// The run was cancelled through [`crate::control::Control::cancel`].
    Cancelled,
    /// A bug or unexpected internal state. Worth reporting.
    Internal,
}

impl ErrorKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::Cancelled => "cancelled",
            Self::Internal => "internal",
        }
    }
}

/// A failure, tagged with the stage it happened in and an optional hint for the user.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Error {
    pub kind: ErrorKind,
    pub stage: Stage,
    pub message: String,
    pub hint: Option<String>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.stage, self.message)
    }
}

impl Error {
    pub fn new(kind: ErrorKind, stage: Stage, message: impl Into<String>) -> Self {
        Self { kind, stage, message: message.into(), hint: None }
    }

    pub fn invalid_input(stage: Stage, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, stage, message)
    }

    pub fn cancelled(stage: Stage) -> Self {
        Self::new(ErrorKind::Cancelled, stage, "cancelled by user")
    }

    pub fn internal(stage: Stage, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, stage, message)
    }

    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

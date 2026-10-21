//! Pipeline stages, cancellation and progress reporting.

use std::fmt;

/// A step of the remeshing pipeline. Errors and progress are tagged with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Stage {
    Validate,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Self::Validate => "validate",
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

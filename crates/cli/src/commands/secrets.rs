//! `clwd secrets` — manage secrets for an app. Body lands with #27.

use crate::Progress;
use crate::commands::Pending;
use crate::error::CliError;

pub fn execute(_progress: Option<&dyn Progress>) -> Result<Pending, CliError> {
    Err(CliError::NotImplemented { command: "secrets" })
}

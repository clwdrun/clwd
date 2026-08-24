//! `clwd run` — run an app locally in a sandbox. Body lands with #25.

use crate::commands::Pending;
use crate::error::CliError;
use crate::Progress;

pub fn execute(_progress: Option<&dyn Progress>) -> Result<Pending, CliError> {
    Err(CliError::NotImplemented { command: "run" })
}

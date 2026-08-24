//! `clwd dev` — run an app locally with live reload. Body lands with #30.

use crate::commands::Pending;
use crate::error::CliError;
use crate::Progress;

pub fn execute(_progress: Option<&dyn Progress>) -> Result<Pending, CliError> {
    Err(CliError::NotImplemented { command: "dev" })
}

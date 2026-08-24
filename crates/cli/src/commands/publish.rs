//! `clwd publish` — publish a built bundle. Body lands with #26.

use crate::commands::Pending;
use crate::error::CliError;
use crate::Progress;

pub fn execute(_progress: Option<&dyn Progress>) -> Result<Pending, CliError> {
    Err(CliError::NotImplemented { command: "publish" })
}

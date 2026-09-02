//! `clwd mcp` — expose these commands over MCP. Body lands with #32.

use crate::Progress;
use crate::commands::Pending;
use crate::error::CliError;

pub fn execute(_progress: Option<&dyn Progress>) -> Result<Pending, CliError> {
    Err(CliError::NotImplemented { command: "mcp" })
}

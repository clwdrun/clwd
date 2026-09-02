//! `clwd build` — build an app into a deployable bundle.
//!
//! The command is a thin shell over [`crate::build`]: it turns arguments into
//! [`Options`] and hands back what the pipeline produced. Rendering is the
//! binary's problem, per the rule in [`crate::commands`].

use std::path::{Path, PathBuf};

use crate::build::{self, Built, Options};
use crate::error::CliError;
use crate::Progress;

pub fn execute(
    project: &Path,
    out: Option<PathBuf>,
    optimize: bool,
    progress: Option<&dyn Progress>,
) -> Result<Built, CliError> {
    let mut options = Options::new(project.to_path_buf());
    options.out = out;
    options.optimize = optimize;
    build::run(&options, progress)
}

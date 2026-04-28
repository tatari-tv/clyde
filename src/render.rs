use crate::RunResult;
use crate::config::RenderConfig;
use eyre::{Result, bail};

pub fn run(cfg: &RenderConfig) -> Result<RunResult> {
    log::trace!("render::run: input={}", cfg.input.display());
    bail!("`cr render` is not implemented yet");
}

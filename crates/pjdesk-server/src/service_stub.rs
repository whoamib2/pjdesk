//! Service stand-in for builds that are not Windows.

#![cfg_attr(not(windows), allow(dead_code))]

use std::path::Path;

use anyhow::Result;

use crate::ServerOpts;

pub fn install(_opts: &ServerOpts, _data_dir: &Path, _firewall: bool) -> Result<()> {
    anyhow::bail!("the Windows service is only available on Windows")
}

pub fn uninstall() -> Result<()> {
    anyhow::bail!("the Windows service is only available on Windows")
}

pub fn start() -> Result<()> {
    anyhow::bail!("the Windows service is only available on Windows")
}

pub fn stop() -> Result<()> {
    anyhow::bail!("the Windows service is only available on Windows")
}

pub fn status() -> (bool, bool) {
    (false, false)
}

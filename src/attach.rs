//! Attaching to a session, and remembering which one lemur showed last.

use crate::config::{Host, home, read_toml};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Attaches this terminal to a session until you detach (ctrl+\), with
/// herdr-core's own `attach`.
pub fn attach_here(host: &Host, session: &str) -> Result<()> {
    let _ = Active {
        host: host.name.clone(),
        session: session.to_string(),
    }
    .save();
    let mut command = Command::new(crate::hc::binary());
    command.arg("attach").arg(session);
    if let Some(s) = host.hc_session.as_deref().filter(|_| host.ssh == "local") {
        command.env("HC_SESSION", s);
    }
    let status = command.status().context("starting herdr-core attach")?;
    if !status.success() {
        bail!("attach to {session} exited with {status}");
    }
    Ok(())
}

/// The session lemur last showed, for `lemur paste` and `upload`.
#[derive(Serialize, Deserialize)]
pub struct Active {
    pub host: String,
    pub session: String,
}

impl Active {
    /// Where lemur remembers it.
    pub fn path() -> Result<PathBuf> {
        Ok(home()?.join(".lemur/active.toml"))
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path()?)
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let dir = path.parent().expect("has parent");
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        std::fs::write(path, toml::to_string(self)?)
            .with_context(|| format!("writing {}", path.display()))
    }

    pub fn load() -> Result<Active> {
        Self::load_from(&Self::path()?)
    }

    pub fn load_from(path: &Path) -> Result<Active> {
        read_toml(path)
    }
}

//! Commands and files on a host, through herdr-core: it keeps one SSH
//! connection per host, so each command costs milliseconds.

use crate::config::Host;
use crate::hc::{self, Output};
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::time::Duration;

/// How long a script may take, `git fetch` included.
const SCRIPT_TIME: Duration = Duration::from_secs(300);

pub struct Remote<'a> {
    pub host: &'a Host,
}

impl<'a> Remote<'a> {
    pub fn new(host: &'a Host) -> Self {
        Remote { host }
    }

    pub fn is_local(&self) -> bool {
        self.host.ssh == "local"
    }

    /// Runs a POSIX sh script on the host and returns its stdout.
    ///
    /// The script goes over stdin to `sh -s`, so the host's login shell
    /// (bash, zsh, fish...) never has to parse it. A non-interactive shell
    /// often lacks `~/.local/bin` (where Claude Code installs) on PATH.
    pub fn run(&self, script: &str) -> Result<String> {
        let (core, on) = hc::for_host(self.host)?;
        let script = format!("PATH=\"$HOME/.local/bin:$PATH\"\n{script}");
        let out = core
            .exec(
                on.as_deref(),
                &["sh", "-s"],
                Some(script.as_bytes()),
                SCRIPT_TIME,
            )
            .with_context(|| format!("running a script on {}", self.host.name))?;
        check(&out, &self.host.name)?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Copies a local file to a path on the host: absolute, or from home.
    pub fn copy_to(&self, local: &Path, remote_path: &str) -> Result<()> {
        if self.is_local() {
            std::fs::copy(local, remote_path)
                .with_context(|| format!("copying to {remote_path}"))?;
            return Ok(());
        }
        let data = std::fs::read(local).with_context(|| format!("reading {}", local.display()))?;
        let (core, on) = hc::for_host(self.host)?;
        core.write_file(on.as_deref(), remote_path, &data)
            .with_context(|| format!("copying to {remote_path} on {}", self.host.name))?;
        Ok(())
    }

    /// Copies a file at an absolute path on the host to a local file.
    pub fn copy_from(&self, remote_path: &str, local: &Path) -> Result<()> {
        if self.is_local() {
            std::fs::copy(remote_path, local).with_context(|| format!("copying {remote_path}"))?;
            return Ok(());
        }
        let (core, on) = hc::for_host(self.host)?;
        let out = core.exec(
            on.as_deref(),
            &["cat", "--", remote_path],
            None,
            SCRIPT_TIME,
        )?;
        check(
            &out,
            &format!("copying {remote_path} from {}", self.host.name),
        )?;
        if out.truncated {
            bail!("{remote_path} is too big to copy from {}", self.host.name);
        }
        std::fs::write(local, &out.stdout).with_context(|| format!("writing {}", local.display()))
    }
}

/// Turns a failed command into an error carrying its stderr.
fn check(out: &Output, what: &str) -> Result<()> {
    if out.timed_out {
        bail!("{what}: took too long");
    }
    if out.code != Some(0) {
        match (out.stderr.trim(), out.code) {
            ("", Some(code)) => bail!("{what}: exited with {code}"),
            ("", None) => bail!("{what}: killed"),
            (stderr, _) => bail!("{what}: {stderr}"),
        }
    }
    Ok(())
}

/// Quotes a string as a single POSIX shell word.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Quotes a path for sh while keeping a leading `~/` expandable.
pub fn sh_path(p: &str) -> String {
    if p == "~" {
        "\"$HOME\"".into()
    } else if let Some(rest) = p.strip_prefix("~/") {
        format!("\"$HOME\"/{}", quote(rest))
    } else {
        quote(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_escapes_single_quotes() {
        assert_eq!(quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn sh_path_keeps_tilde_expandable() {
        assert_eq!(sh_path("~/code/api"), "\"$HOME\"/'code/api'");
        assert_eq!(sh_path("/srv/api"), "'/srv/api'");
        assert_eq!(sh_path(".claude/worktrees"), "'.claude/worktrees'");
    }
}

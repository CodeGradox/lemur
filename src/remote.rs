//! Commands and files on a host, through herdr-core: it keeps one SSH
//! connection per host, so each command costs milliseconds.

use crate::config::Host;
use crate::hc::{self, Output};
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::time::Duration;

/// How long a script may take, `git fetch` included.
const SCRIPT_TIME: Duration = Duration::from_secs(300);
/// The most of a file one write sends: herdr-core takes at most 8 MB a
/// write, and a frame over 16 MiB (base64 adds a third) ends Lemur's whole
/// connection to it.
const PART: usize = 4 << 20;

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
        let out = self.exec(script)?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// `run`, for a script whose output is no use unless it's all there:
    /// herdr-core keeps the first 8 MB.
    pub fn run_whole(&self, script: &str) -> Result<String> {
        let out = self.exec(script)?;
        if out.truncated {
            bail!("{}: printed more than herdr-core keeps", self.host.name);
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn exec(&self, script: &str) -> Result<Output> {
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
        Ok(out)
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
        let copying = || format!("copying to {remote_path} on {}", self.host.name);
        if data.len() <= PART {
            core.write_file(on.as_deref(), remote_path, &data)
                .with_context(copying)?;
            return Ok(());
        }
        // In parts, joined there.
        let parts: Vec<String> = (0..data.len().div_ceil(PART))
            .map(|i| format!("{remote_path}.lemur-part-{i}"))
            .collect();
        let quoted: Vec<String> = parts.iter().map(|p| quote(p)).collect();
        let written = parts
            .iter()
            .zip(data.chunks(PART))
            .try_for_each(|(part, chunk)| core.write_file(on.as_deref(), part, chunk).map(drop));
        let joined = written.and_then(|()| {
            self.run(&format!(
                "cat {} > {} || exit 1\n",
                quoted.join(" "),
                quote(remote_path)
            ))
        });
        let _ = self.run(&format!("rm -f {}\n", quoted.join(" ")));
        joined.with_context(copying)?;
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
    /// Output past what herdr-core keeps is cut, which `run_whole` says.
    #[test]
    fn output_cut_short_is_told_of() {
        let server = crate::test_support::HcServer::new("cut-short");
        let host = server.config().hosts.remove(0);
        let remote = super::Remote::new(&host);
        let script = "head -c 9000000 /dev/zero; echo @@end\n";
        assert!(!remote.run(script).unwrap().contains("@@end"), "it's cut");
        let e = remote.run_whole(script).unwrap_err();
        assert!(
            format!("{e:#}").contains("more than herdr-core keeps"),
            "{e:#}"
        );
        assert_eq!(remote.run_whole("echo all\n").unwrap(), "all\n");
    }

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

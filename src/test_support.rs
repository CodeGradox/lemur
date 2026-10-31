//! Helpers for tests that run a real herdr-core and real processes.

use crate::config::Config;
use crate::hc::{self, Core};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A herdr-core of the test's own: its own session, with its state and
/// config in a folder of their own, so the core people use never sees it.
/// Dropping it stops the core and removes the folder, so a failing test
/// doesn't leave either behind.
pub struct HcServer {
    pub session: String,
    dir: PathBuf,
    /// The sessions of the stand-in hosts' cores, to stop too.
    far: std::cell::RefCell<Vec<String>>,
}

impl HcServer {
    pub fn new(tag: &str) -> HcServer {
        // The core's socket goes in its state folder, and macOS allows a
        // socket path only 104 bytes.
        let dir = PathBuf::from(format!("/tmp/lhc-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("creating the core's folder");
        let server = HcServer {
            session: tag.to_string(),
            dir,
            far: Default::default(),
        };
        hc::set_test_env(&server.session, server.env());
        server
    }

    fn env(&self) -> Vec<(String, String)> {
        let at = |sub: &str| self.dir.join(sub).to_string_lossy().into_owned();
        vec![
            ("XDG_STATE_HOME".into(), at("state")),
            ("XDG_CONFIG_HOME".into(), at("config")),
            ("HC_SESSION".into(), self.session.clone()),
        ]
    }

    /// lemur's connection to this core.
    pub fn core(&self) -> Arc<Core> {
        hc::core(Some(&self.session)).expect("connecting to the test core")
    }

    /// Runs `hc` against this core; returns what it printed.
    pub fn hc(&self, args: &[&str]) -> String {
        let out = Command::new(hc::binary())
            .args(args)
            .envs(self.env())
            .output()
            .expect("running hc");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Starts `argv` in a terminal in `dir`, with `labels` (`k=v`), and
    /// returns its address.
    pub fn run(&self, dir: &str, labels: &[&str], argv: &[&str]) -> String {
        let mut args = vec!["run", "--bg", "--json", "--keep", "--cwd", dir];
        for label in labels {
            args.extend(["--label", label]);
        }
        args.push("--");
        args.extend(argv);
        let out = self.hc(&args);
        let started: serde_json::Value = serde_json::from_str(&out).expect("hc run's answer");
        started["address"].as_str().expect("an address").to_string()
    }

    /// The addresses of the live terminals.
    pub fn addresses(&self) -> Vec<String> {
        self.core()
            .terminals()
            .into_iter()
            .map(|t| t.address)
            .collect()
    }

    /// A config with one host, `name`, that this core reaches as another
    /// machine: a core of its own on this Mac, run through `sh -c` where
    /// SSH would log in to another.
    pub fn far_config(&self, name: &str) -> Config {
        let far = format!("{}-far", self.session);
        self.hc(&[
            "machine",
            "add",
            "--name",
            name,
            "--session",
            &far,
            "--via",
            "sh -c",
        ]);
        self.far.borrow_mut().push(far);
        hc::set_test_route(name, &self.session);
        toml::from_str(&format!(
            "[[hosts]]\nname = \"{name}\"\n{} = \"{name}\"\n",
            "ssh"
        ))
        .expect("test config")
    }

    /// A config with one host, `local`, whose terminals run in this core.
    pub fn config(&self) -> Config {
        toml::from_str(&format!(
            "[[hosts]]\nname = \"local\"\nssh = \"local\"\nhc_session = \"{}\"\n",
            self.session
        ))
        .expect("test config")
    }
}

impl Drop for HcServer {
    fn drop(&mut self) {
        // This core first: it would dial the stand-in hosts' again.
        self.hc(&["server", "stop"]);
        for far in self.far.borrow().iter() {
            let _ = Command::new(hc::binary())
                .args(["server", "stop"])
                .envs(self.env())
                .env("HC_SESSION", far)
                .output();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A fresh folder in the temp dir, removed when dropped, so a failing
/// test doesn't leave it behind. Its path is the real one: macOS's /tmp is
/// a link, and git reports real paths.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("lemur-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("creating a temp dir");
        TempDir(dir.canonicalize().expect("the temp dir's real path"))
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs git in `dir` as a throwaway author; it must succeed.
pub fn git_in(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("running git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A stand-in for lemur-dictate in `dir` that runs the shell `script`.
pub fn fake_dictate(dir: &Path, script: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("lemur-dictate");
    std::fs::write(&path, format!("#!/bin/sh\n{script}")).expect("writing the stand-in");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("making the stand-in runnable");
    path
}

/// Polls `done` until it holds or `timeout` passes; returns whether it held.
pub fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

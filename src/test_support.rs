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
        let added = Command::new(hc::binary())
            .args([
                "machine",
                "add",
                "--name",
                name,
                "--session",
                &far,
                "--via",
                "sh -c",
            ])
            .envs(self.env())
            .output()
            .expect("running hc");
        self.far.borrow_mut().push(far);
        // Otherwise Lemur would add `name` itself, as a host to dial.
        assert!(
            added.status.success(),
            "adding the stand-in machine {name}: {}",
            String::from_utf8_lossy(&added.stderr)
        );
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
        // This core first: it would dial the stand-in hosts' again. Never
        // a panic here: in a test already failing, that aborts the run.
        let _ = Command::new(hc::binary())
            .args(["server", "stop"])
            .envs(self.env())
            .output();
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

/// A request a `FakeLinear` got.
#[derive(Debug, Clone)]
pub struct Request {
    pub path: String,
    pub authorization: Option<String>,
    pub body: String,
}

type Answer = Box<dyn Fn(&Request) -> (u16, String) + Send>;

/// A stand-in for Linear's API on 127.0.0.1: it answers each request as
/// `answer` says, and keeps them in order. Dropping it stops it.
pub struct FakeLinear {
    pub url: String,
    requests: Arc<std::sync::Mutex<Vec<Request>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl FakeLinear {
    pub fn new(answer: impl Fn(&Request) -> (u16, String) + Send + 'static) -> FakeLinear {
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port for Linear");
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(std::sync::Mutex::new(vec![]));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (kept, stopped) = (requests.clone(), stop.clone());
        let answer: Answer = Box::new(answer);
        std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream, &answer, &kept),
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        FakeLinear {
            url,
            requests,
            stop,
        }
    }

    /// What it was asked, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for FakeLinear {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Reads one HTTP request from `stream`, keeps it in `kept`, and answers
/// it: kept first, as the caller may look as soon as it has the answer.
fn serve(stream: std::net::TcpStream, answer: &Answer, kept: &std::sync::Mutex<Vec<Request>>) {
    let _ = serve_one(stream, answer, kept);
}

fn serve_one(
    mut stream: std::net::TcpStream,
    answer: &Answer,
    kept: &std::sync::Mutex<Vec<Request>>,
) -> Option<()> {
    use std::io::{Read, Write};
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut data = vec![];
    let mut buf = [0; 8192];
    let head_end = loop {
        if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        let n = stream.read(&mut buf).ok().filter(|n| *n > 0)?;
        data.extend_from_slice(&buf[..n]);
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let header = |name: &str| {
        head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    };
    let length: usize = header("content-length")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0);
    while data.len() < head_end + length {
        let n = stream.read(&mut buf).ok().filter(|n| *n > 0)?;
        data.extend_from_slice(&buf[..n]);
    }
    let request = Request {
        path: head.split(' ').nth(1)?.to_string(),
        authorization: header("authorization"),
        body: String::from_utf8_lossy(&data[head_end..head_end + length]).into_owned(),
    };
    let (status, body) = answer(&request);
    kept.lock().unwrap().push(request);
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    Some(())
}

/// Each test's core has a tag of its own: two tests running at once with
/// one would share its folder and socket, and stop each other's core.
#[test]
fn test_cores_have_tags_of_their_own() {
    fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, found);
            } else if path.extension().is_some_and(|e| e == "rs") {
                found.push(path);
            }
        }
    }
    let mut files = vec![];
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let mut seen = std::collections::HashMap::new();
    let call = concat!("HcServer::new", "(\"");
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for (at, _) in text.match_indices(call) {
            let rest = &text[at + call.len()..];
            let tag = &rest[..rest.find('"').unwrap()];
            if let Some(other) = seen.insert(tag.to_string(), file.clone()) {
                panic!("tag {tag:?} in {} and {}", other.display(), file.display());
            }
        }
    }
    assert!(seen.len() > 50, "found {} tags", seen.len());
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

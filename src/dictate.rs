//! Dictation: lemur-dictate (src/dictate.swift) listens to the microphone
//! and transcribes on this Mac; this runs it and reads what it heard.

use anyhow::{Context, Result};
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

/// A line lemur-dictate writes.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Heard {
    /// The whole transcript so far: words that stay, then a guess that
    /// later lines replace.
    Words { finalized: String, volatile: String },
    /// What it's doing before it listens, like downloading a language.
    Status { status: String },
    /// It's listening.
    Listening { listening: bool },
    /// Why it stopped.
    Error { error: String },
    /// It finished what it heard, once stopped.
    Done { done: bool },
}

/// lemur-dictate: beside lemur in Lemur.app, or where the build put it.
pub fn helper() -> PathBuf {
    std::env::current_exe()
        .map(|exe| exe.with_file_name("lemur-dictate"))
        .ok()
        .filter(|p| p.exists())
        .unwrap_or_else(|| env!("LEMUR_DICTATE").into())
}

/// lemur-dictate listening. Dropping it throws away what it heard.
pub struct Recording {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Recording {
    /// Starts `helper` listening for `locale`, like `nb-NO`. What it hears
    /// arrives on the receiver, which ends when it exits; one that exits
    /// without saying it's done or why ends with an error.
    pub fn start(helper: &Path, locale: &str) -> Result<(Recording, UnboundedReceiver<Heard>)> {
        let mut child = Command::new(helper)
            .args(["--locale", locale])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting {}", helper.display()))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped");
        let (tx, rx) = unbounded();
        std::thread::spawn(move || {
            let mut ended = false;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let heard = serde_json::from_str(&line).unwrap_or(Heard::Error {
                    error: format!("lemur-dictate said {line}"),
                });
                ended = matches!(heard, Heard::Done { .. } | Heard::Error { .. });
                let _ = tx.unbounded_send(heard);
            }
            if !ended {
                let _ = tx.unbounded_send(Heard::Error {
                    error: "lemur-dictate quit; dictation needs macOS 26 or newer".into(),
                });
            }
        });
        Ok((Recording { child, stdin }, rx))
    }

    /// Stops listening: lemur-dictate finishes what it heard, then says
    /// it's done.
    pub fn stop(&mut self) {
        self.stdin = None;
    }

    /// Whether it was stopped.
    pub fn stopped(&self) -> bool {
        self.stdin.is_none()
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TempDir, fake_dictate};
    use futures::StreamExt;
    use futures::executor::block_on;

    #[test]
    fn reads_the_lines_lemur_dictate_writes() {
        let line = |s: &str| serde_json::from_str::<Heard>(s).unwrap();
        assert_eq!(
            line(r#"{"finalized": "Hi.", "volatile": " there"}"#),
            Heard::Words {
                finalized: "Hi.".into(),
                volatile: " there".into()
            }
        );
        assert_eq!(
            line(r#"{"status": "downloading"}"#),
            Heard::Status {
                status: "downloading".into()
            }
        );
        assert_eq!(
            line(r#"{"error": "no"}"#),
            Heard::Error { error: "no".into() }
        );
        assert_eq!(
            line(r#"{"listening": true}"#),
            Heard::Listening { listening: true }
        );
        assert_eq!(line(r#"{"done": true}"#), Heard::Done { done: true });
    }

    /// Stopping closes its stdin, and it finishes after that.
    #[test]
    fn stopping_lets_it_finish() {
        let dir = TempDir::new("dictate-stop");
        let helper = fake_dictate(
            &dir,
            r#"echo "{\"finalized\": \"$2\", \"volatile\": \"\"}"
cat >/dev/null
echo '{"done": true}'
"#,
        );
        let (mut recording, mut heard) = Recording::start(&helper, "nb-NO").unwrap();
        let first = block_on(heard.next());
        assert_eq!(
            first,
            Some(Heard::Words {
                finalized: "nb-NO".into(),
                volatile: "".into()
            })
        );
        recording.stop();
        assert!(recording.stopped());
        let rest: Vec<Heard> = block_on(heard.collect());
        assert_eq!(rest, [Heard::Done { done: true }]);
    }

    /// One that quits without a word ends with an error.
    #[test]
    fn quitting_unannounced_is_an_error() {
        let dir = TempDir::new("dictate-quit");
        let helper = fake_dictate(&dir, "exit 1\n");
        let (_recording, heard) = Recording::start(&helper, "en-US").unwrap();
        let heard: Vec<Heard> = block_on(heard.collect());
        assert!(matches!(heard[..], [Heard::Error { .. }]), "{heard:?}");
    }
}

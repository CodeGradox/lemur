//! Threads Claude runs in, kept with their Claude session ID so they can be
//! resumed once their terminal is gone, as after the host restarted.
//! Each starts with an ID of lemur's, and refreshes keep it current.

use super::{Lemur, sessions};
use crate::actions;
use crate::config::{ClaudeThread, Config};

impl Lemur {
    /// Keeps `thread` to resume, in place of its session's entry.
    pub(super) fn keep_thread(&mut self, thread: ClaudeThread) {
        if self.cfg.threads.contains(&thread) {
            return;
        }
        if let Some(file) = self.prefs.config_file()
            && let Err(e) = Config::save_thread(file, &thread)
        {
            self.fail(e);
        }
        self.cfg
            .threads
            .retain(|t| !(t.host == thread.host && t.session == thread.session));
        self.cfg.threads.push(thread);
    }

    /// Forgets the kept threads of `sessions` on `host`: they won't be
    /// resumed, and the sidebar drops them.
    pub(super) fn forget_threads(
        &mut self,
        host: &str,
        sessions: &[String],
    ) {
        let kept = self.cfg.threads.len();
        self.cfg
            .threads
            .retain(|t| !(t.host == host && sessions.contains(&t.session)));
        if self.cfg.threads.len() == kept {
            return;
        }
        if let Some(file) = self.prefs.config_file()
            && let Err(e) = Config::remove_threads(file, host, sessions)
        {
            self.fail(e);
        }
        let threads = &self.cfg.threads;
        self.snap.retain_gone(|t| threads.contains(t));
        // A snapshot fetched before would bring them back.
        self.changes += 1;
            }

    /// Notes the shell each Claude thread runs in, so that one whose host
    /// restarted, back at its address as a fresh shell, is known to be.
    pub(super) fn note_shells(&mut self) {
        let notes: Vec<(String, String, u32)> = sessions(&self.snap)
            .filter_map(|(host, s)| Some((host.to_string(), s.name.clone(), s.shell_to_note()?)))
            .collect();
        if notes.is_empty() {
            return;
        }
        let cfg = self.cfg.clone();
        self.in_background(
            move || {
                for (host, session, pid) in notes {
                    // Noted at the next snapshot if it can't be now.
                    let _ = actions::note_shell(&cfg, &host, &session, pid);
                }
            },
            |_, _| {},
        )
        ;
    }

    /// Keeps each running Claude's session ID as `claude agents` reports
    /// it: a thread lemur didn't start gets one, and one whose Claude
    /// started over (`/clear`) gets the new one.
    pub(super) fn keep_claude_ids(&mut self) {
        let running: Vec<ClaudeThread> = sessions(&self.snap)
            .filter_map(|(host, s)| {
                let run = s.claude.as_ref().filter(|r| actions::is_uuid(&r.id))?;
                Some(ClaudeThread {
                    host: host.to_string(),
                    session: s.name.clone(),
                    path: run.path.clone(),
                    claude_session: run.id.clone(),
                    label: Some(s.own_label.clone()),
                    issue: s.issue.clone(),
                })
            })
            .collect();
        for thread in running {
            // Its conversation went on in this session: renamed, or
            // resumed by hand.
            let moved: Vec<String> = self
                .cfg
                .threads
                .iter()
                .filter(|t| {
                    t.host == thread.host
                        && t.claude_session == thread.claude_session
                        && t.session != thread.session
                })
                .map(|t| t.session.clone())
                .collect();
            self.forget_threads(&thread.host, &moved);
            self.keep_thread(thread);
        }
    }

    /// Starts gone threads' sessions again, each resuming its Claude
    /// conversation; one alone opens.
    pub(super) fn resume(&mut self, threads: Vec<ClaudeThread>) {
        // Not one being resumed, as with a double click, or ▶ then Resume
        // all: two Claudes would work on one conversation.
        let threads: Vec<ClaudeThread> = threads
            .into_iter()
            .filter(|t| self.resuming.insert((t.host.clone(), t.session.clone())))
            .collect();
        let what = match threads.as_slice() {
            [] => return,
            [one] => one.shown(),
            many => format!("{} threads", many.len()),
        };
        let said = self.busy(format!("resuming {what}…"));
        let cfg = self.cfg.clone();
        self.in_background(
            move || {
                threads
                    .into_iter()
                    .map(|t| {
                        let resumed = actions::resume_thread(&cfg, &t);
                        (t, resumed)
                    })
                    .collect::<Vec<_>>()
            },
            move |this, results| {
                this.done(&said);
                let mut resumed = vec![];
                for (thread, result) in results {
                    this.resuming
                        .remove(&(thread.host.clone(), thread.session.clone()));
                    match result {
                        // It goes on in a terminal of its own.
                        Ok(session) => {
                            this.forget_threads(
                                &thread.host,
                                std::slice::from_ref(&thread.session),
                                                );
                            let again = ClaudeThread { session, ..thread };
                            this.keep_thread(again.clone());
                            resumed.push(again);
                        }
                        Err(e) => this.fail(e.context(format!("resuming {}", thread.shown()))),
                    }
                }
                // Refreshes that started before it was back are stale.
                this.changes += 1;
                if let [one] = resumed.as_slice() {
                    this.open_fresh(one.host.clone(), one.session.clone());
                }
                this.refresh();
                            },
        )
        ;
    }

    /// The gone threads of project `index`, to resume all at once.
    pub(super) fn gone_in(&self, index: usize) -> Vec<ClaudeThread> {
        self.snap
            .hosts
            .iter()
            .flat_map(|h| h.bases.iter())
            .filter(|b| b.index == index)
            .flat_map(|b| b.worktrees.iter())
            .flat_map(|w| w.gone.iter().cloned())
            .collect()
    }
}

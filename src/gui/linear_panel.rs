//! The Linear panel, on the right as the notes, the changes and the agents
//! are: the Linear issue pinned to the thread on show, to open in Linear or
//! paste at its agent's prompt, and a search for issues to pin. Lemur signs
//! in with its own Linear app (`crate::linear`); Settings → Linear signs
//! out. It asks Linear only while it shows (as the window points it at the
//! thread on show, and with each snapshot), at most every `FRESH` for the
//! same issues.

use super::{Key, Lemur, Right, icons};
use crate::actions;
use crate::config::ClaudeThread;
use crate::linear::{self, Issue, Linear};
use rune::{El, Key as K, ScrollSpec, TextEdit, Tx, col, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, Markdown, TextInput};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long what Linear said of an issue, or of yours, is shown before
/// it's asked again.
const FRESH: Duration = Duration::from_secs(120);
/// How long typing pauses before a search: Linear allows 30 a minute.
const TYPING: Duration = Duration::from_millis(350);

/// Whether Lemur is signed in to Linear. The panel and Settings follow it.
#[derive(Clone)]
pub enum Account {
    /// Asking Linear whether the sign-in kept still works.
    Checking,
    /// With why the last sign-in failed, if it did.
    SignedOut(Option<String>),
    /// Waiting for you in the browser; set the flag to call it off.
    SigningIn(Arc<AtomicBool>),
    /// As whom, once Linear has said.
    SignedIn(Option<linear::Account>),
}

/// The thread on show: its key, its name, and the issue pinned to it.
pub type Thread = (Key, String, Option<String>);

pub struct LinearPanel {
    /// Lemur's Linear; tests have a stand-in.
    linear: Linear,
    account: Account,
    /// Whether the sign-in kept was looked at yet: once, as Lemur starts.
    checked: bool,
    thread: Option<Thread>,
    query: TextEdit,
    /// What was searched for, and what Linear found.
    found: Option<(String, Result<Vec<Issue>, String>)>,
    searching: bool,
    /// Counts the searches started and given up: only the latest's answer shows.
    search: u64,
    /// The issues assigned to you, and when Linear said.
    mine: Option<(Instant, Result<Vec<Issue>, String>)>,
    /// Pinned issues, by key, as Linear last said, and when.
    issues: HashMap<String, (Instant, Result<Issue, String>)>,
    /// Linear is being asked for yours, or for a pinned issue.
    asking_mine: bool,
    asking_issue: bool,
    /// The search field had the keyboard at the last frame.
    had_keyboard: bool,
    /// Linear's page asking you to approve Lemur, opened in the browser before the next
    /// frame: Settings' button has no `Tx` to open it with.
    approve: Option<String>,
}

impl Default for LinearPanel {
    fn default() -> Self {
        LinearPanel {
            linear: Linear::new(),
            account: Account::Checking,
            checked: false,
            thread: None,
            query: TextEdit::new(""),
            found: None,
            searching: false,
            search: 0,
            mine: None,
            issues: HashMap::new(),
            asking_mine: false,
            asking_issue: false,
            had_keyboard: false,
            approve: None,
        }
    }
}

impl LinearPanel {
    fn signed_in(&self) -> bool {
        matches!(self.account, Account::SignedIn(_))
    }

    #[cfg(test)]
    pub(super) fn shown(&self) -> Option<&Thread> {
        self.thread.as_ref()
    }
}

impl Lemur {
    /// Points the Linear panel at the thread on show, while it shows. The first time, as
    /// the first snapshot comes in, it asks whether the sign-in kept still works.
    pub(super) fn follow_linear(&mut self) {
        self.check_linear();
        if self.right != Some(Right::Linear) {
            return;
        }
        let thread = self.active.clone().map(|key| {
            let (label, issue) = match self.session_of(&key) {
                Some(s) => (s.label.clone(), s.issue.clone()),
                None => (self.label_of(&key.0, &key.1), None),
            };
            (key, label, issue)
        });
        self.panels.linear.thread = thread;
        self.fetch_linear();
    }

    /// Starts following Linear: whether its sign-in is kept, and still works.
    fn check_linear(&mut self) {
        let p = &mut self.panels.linear;
        if p.checked {
            return;
        }
        p.checked = true;
        // A headless run (a snapshot, a script) leaves the Keychain alone: reading it may
        // ask you, on the screen you're using.
        if rune::tooling::headless_run().is_some() {
            return self.linear_checked(Account::SignedOut(None));
        }
        let linear = p.linear.clone();
        self.in_background(
            move || match linear.signed_in() {
                Ok(false) => Account::SignedOut(None),
                Err(e) => Account::SignedOut(Some(format!("{e:#}"))),
                Ok(true) => match linear.account() {
                    Ok(account) => Account::SignedIn(Some(account)),
                    Err(e) if linear::is_signed_out(&e) => Account::SignedOut(None),
                    // Linear can't be reached: the sign-in is kept.
                    Err(_) => Account::SignedIn(None),
                },
            },
            |s, state| s.linear_checked(state),
        );
    }

    /// Takes what checking the kept sign-in found, unless a sign-in started
    /// meanwhile: its Cancel would go, and its answer would be lost.
    fn linear_checked(&mut self, state: Account) {
        if matches!(self.panels.linear.account, Account::Checking) {
            self.set_linear_account(state);
        }
    }

    /// The sign-in changed: what Linear said was another sign-in's, or an error it no
    /// longer has, so it's asked again.
    fn set_linear_account(&mut self, state: Account) {
        let p = &mut self.panels.linear;
        p.account = state;
        p.mine = None;
        p.issues.clear();
        p.found = None;
        if p.signed_in() {
            self.linear_search();
        }
        self.follow_linear();
    }

    /// Opens Linear's page asking you to approve Lemur, and waits for the
    /// browser to come back with the answer.
    fn sign_in_linear(&mut self) {
        let p = &self.panels.linear;
        if matches!(p.account, Account::SigningIn(_)) {
            return;
        }
        let linear = p.linear.clone();
        let sign_in = match linear.start_sign_in() {
            Ok(sign_in) => sign_in,
            Err(e) => return self.set_linear_account(Account::SignedOut(Some(format!("{e:#}")))),
        };
        self.panels.linear.approve = Some(sign_in.url());
        let cancelled = Arc::new(AtomicBool::new(false));
        self.set_linear_account(Account::SigningIn(cancelled.clone()));
        self.in_background(
            move || linear.finish_sign_in(sign_in, &cancelled),
            |s, done| {
                let state = match done {
                    Ok(account) => Account::SignedIn(Some(account)),
                    Err(e) if linear::is_cancelled(&e) => Account::SignedOut(None),
                    Err(e) => Account::SignedOut(Some(format!("{e:#}"))),
                };
                s.set_linear_account(state);
            },
        );
    }

    fn cancel_linear_sign_in(&mut self) {
        if let Account::SigningIn(cancelled) = &self.panels.linear.account {
            cancelled.store(true, Ordering::Relaxed);
        }
    }

    /// Forgets the sign-in, and tells Linear so.
    fn sign_out_linear(&mut self) {
        let linear = self.panels.linear.linear.clone();
        self.set_linear_account(Account::SignedOut(None));
        self.in_background(move || linear.sign_out(), |_, _| {});
    }

    /// What Settings says of Lemur's Linear sign-in, and why the last went wrong, if it did.
    #[allow(dead_code)]
    pub(super) fn linear_account_status(&self) -> (String, Option<String>) {
        match &self.panels.linear.account {
            Account::Checking => ("Checking…".into(), None),
            Account::SignedOut(error) => ("Not signed in".into(), error.clone()),
            Account::SigningIn(_) => ("Waiting for you in the browser…".into(), None),
            Account::SignedIn(Some(a)) => (format!("{} · {}", a.name, a.workspace), None),
            Account::SignedIn(None) => ("Signed in · Linear can't be reached".into(), None),
        }
    }

    /// What Settings offers for the sign-in, and what that runs: Sign out signed in, Cancel
    /// signing in, else Sign in.
    #[allow(dead_code)]
    pub(super) fn linear_account_action(&self) -> (&'static str, fn(&mut Lemur)) {
        match &self.panels.linear.account {
            Account::SignedIn(_) => ("Sign out", Lemur::sign_out_linear),
            Account::SigningIn(_) => ("Cancel", Lemur::cancel_linear_sign_in),
            _ => ("Sign in with Linear", Lemur::sign_in_linear),
        }
    }

    /// What Linear said, or why it couldn't; a sign-in Linear turned down
    /// signs Lemur out.
    fn linear_said<T>(&mut self, answer: anyhow::Result<T>) -> Result<T, String> {
        answer.map_err(|e| {
            if linear::is_signed_out(&e) {
                self.set_linear_account(Account::SignedOut(None));
            }
            format!("{e:#}")
        })
    }

    /// Asks Linear for what shows, unless it said so lately.
    fn fetch_linear(&mut self) {
        let p = &mut self.panels.linear;
        if !p.signed_in() {
            return;
        }
        let stale = |at: Option<Instant>| at.is_none_or(|at| at.elapsed() >= FRESH);
        let linear = p.linear.clone();
        match p.thread.as_ref().and_then(|(_, _, issue)| issue.clone()) {
            Some(key) => {
                if p.asking_issue || !stale(p.issues.get(&key).map(|(at, _)| *at)) {
                    return;
                }
                p.asking_issue = true;
                let asked = key.clone();
                self.in_background(
                    move || linear.issue(&asked),
                    |s, issue| {
                        s.panels.linear.asking_issue = false;
                        let issue = s.linear_said(issue);
                        s.panels.linear.issues.insert(key, (Instant::now(), issue));
                        // It asks for one pinned meanwhile.
                        s.follow_linear();
                    },
                );
            }
            None => {
                if p.asking_mine || !stale(p.mine.as_ref().map(|(at, _)| *at)) {
                    return;
                }
                p.asking_mine = true;
                self.in_background(
                    move || linear.mine(),
                    |s, mine| {
                        s.panels.linear.asking_mine = false;
                        let mine = s.linear_said(mine);
                        s.panels.linear.mine = Some((Instant::now(), mine));
                    },
                );
            }
        }
    }

    /// Searches for what's typed, once typing pauses.
    fn linear_search(&mut self) {
        let p = &mut self.panels.linear;
        let term = p.query.text.trim().to_string();
        p.search += 1;
        if term.is_empty() {
            p.found = None;
            p.searching = false;
            return;
        }
        // Back to what was found: a search for what was typed meanwhile
        // would answer for a term no longer in the box.
        if p.found.as_ref().is_some_and(|(t, _)| *t == term) {
            p.searching = false;
            return;
        }
        p.searching = true;
        let (linear, search) = (p.linear.clone(), p.search);
        self.bg.after(TYPING, move |s| {
            if s.panels.linear.search != search {
                return;
            }
            let asked = term.clone();
            s.in_background(
                move || linear.search(&asked),
                move |s, found| {
                    if s.panels.linear.search != search {
                        return;
                    }
                    let found = s.linear_said(found);
                    let p = &mut s.panels.linear;
                    p.searching = false;
                    p.found = Some((term, found));
                },
            );
        });
    }

    /// Pins `issue` to the thread on show, or opens it in Linear with no
    /// thread on show.
    fn pick_issue(&mut self, issue: &Issue, tx: &mut Tx) {
        let p = &mut self.panels.linear;
        let Some((key, _, _)) = p.thread.clone() else {
            let _ = tx.open_url(&issue.url);
            return;
        };
        // Found by its key, it came with its description; otherwise that's
        // asked for once it's pinned.
        if issue.description.is_some() {
            p.issues.insert(issue.identifier.clone(), (Instant::now(), Ok(issue.clone())));
        } else {
            p.issues.remove(&issue.identifier);
        }
        p.query.clear();
        p.found = None;
        p.searching = false;
        p.search += 1;
        self.pin_issue(key, Some(issue.identifier.clone()));
    }

    /// Pins the Linear issue `issue` (its key) to thread `key`, or with `None` unpins its
    /// issue: shown at once, and herdr-core keeps it.
    pub(super) fn pin_issue(&mut self, key: Key, issue: Option<String>) {
        if let Some(s) = self.session_mut(&key) {
            s.issue = issue.clone();
        }
        // Kept to resume it, it's pinned again then: whether or not its
        // Claude runs, which keeps the entry current otherwise.
        let kept = self.cfg.threads.iter().find(|t| t.host == key.0 && t.session == key.1).cloned();
        if let Some(thread) = kept {
            self.keep_thread(ClaudeThread { issue: issue.clone(), ..thread });
        }
        self.changes += 1;
        self.follow_linear();
        let cfg = self.cfg.clone();
        self.in_background(
            move || actions::set_issue(&cfg, &key.0, &key.1, issue.as_deref()),
            |s, saved| {
                // Refreshes that started before herdr-core had it are stale too.
                s.changes += 1;
                if let Err(e) = saved {
                    s.fail(e);
                }
                s.refresh();
            },
        );
    }

    /// A link to a Linear issue pasted into a thread pins that issue to it, unless it has
    /// one.
    #[allow(dead_code)]
    pub(super) fn pasted(&mut self, key: &Key, text: &str) {
        let Some(issue) = linear::issue_in(text) else { return };
        if self.session_of(key).is_some_and(|s| s.issue.is_none()) {
            self.pin_issue(key.clone(), Some(issue));
        }
    }

    /// Pastes `text` at thread `key`'s prompt, as if typed there, and gives it the
    /// keyboard; Enter is left to you.
    fn paste_into(&mut self, key: &Key, text: &str, tx: &mut Tx) {
        let Some(view) = self.terminals.get_mut(key) else { return };
        view.paste_text(text);
        self.panels.linear.query.focused = false;
        tx.blur();
    }

    /// Before each frame: Linear's page to approve Lemur opens in the browser, and the
    /// search field took the keyboard, or gave it back to a click elsewhere or the panel
    /// hidden. True if that changed anything.
    pub(super) fn linear_poll(&mut self, tx: &mut Tx) -> bool {
        let p = &mut self.panels.linear;
        if let Some(url) = p.approve.take() {
            let _ = tx.open_url(&url);
        }
        let has = self.right == Some(Right::Linear) && p.query.focused;
        if has == p.had_keyboard {
            return false;
        }
        p.had_keyboard = has;
        // Hidden, it doesn't take the keyboard back as it shows again.
        if !has {
            p.query.focused = false;
        }
        self.typing = has;
        true
    }
}

/// A dot in the color of the issue's state.
fn state_dot(issue: &Issue, k: &Kit) -> El {
    let color = rune::color::hex(&issue.state.color).unwrap_or(k.c.text_muted);
    el().size(8.0, 8.0).radius(4.0).fill(color)
}

/// A line of muted text, for what's under way or went wrong.
fn note(key: &str, said: String, k: &Kit) -> El {
    text(said).key(key.to_string()).font(k.ty.ui).color(k.c.text_muted).wrap().px(12.0).py(4.0)
}

/// Issues to pin, as rows; the one pinned to the thread on show stands out.
fn issue_rows(p: &LinearPanel, issues: &[Issue], k: &Kit) -> El {
    let c = &k.c;
    let pinned = p.thread.as_ref().and_then(|(_, _, i)| i.as_deref());
    let rows = issues.iter().enumerate().map(|(i, issue)| {
        let picked = issue.clone();
        let r = row()
            .key(format!("linear-issue-{i}"))
            .w_full()
            .h(28.0)
            .px(8.0)
            .gap(8.0)
            .center_y()
            .radius(k.radius.md)
            .cursor(rune::Cursor::Pointer)
            .help(issue.url.clone())
            .child(state_dot(issue, k))
            .child(text(issue.identifier.clone()).font(k.ty.small).color(c.text_faint))
            .child(text(issue.title.clone()).font(k.ty.ui).color(c.text_base).truncate().grow(1.0).shrink())
            .on_press_tx(move |s: &mut Lemur, tx: &mut Tx| s.pick_issue(&picked, tx));
        if pinned == Some(issue.identifier.as_str()) { r.fill(c.pressed) } else { k.interactive(r) }
    });
    col().key("linear-issues").w_full().grow(1.0).min_h(0.0).px(4.0).pb(8.0).gap(1.0).scroll(ScrollSpec { stick_bottom: false }).children(rows)
}

/// The issue pinned to the thread on show: what it is, where it's at,
/// and what to do with it.
fn card(p: &LinearPanel, key: &Key, pinned: &str, k: &Kit) -> El {
    let c = &k.c;
    let unpin = key.clone();
    let unpin_button = |variant| {
        Button::new("Unpin")
            .variant(variant)
            .control_size(ControlSize::Small)
            .on_press(move |s: &mut Lemur| s.pin_issue(unpin.clone(), None))
            .build(k)
            .key("linear-unpin")
    };
    let issue = match p.issues.get(pinned) {
        Some((_, Ok(issue))) => issue,
        other => {
            let said = match other {
                Some((_, Err(e))) => e.clone(),
                _ => "Asking Linear…".into(),
            };
            return col()
                .key("linear-card")
                .w_full()
                .px(12.0)
                .pb(12.0)
                .gap(8.0)
                .child(text(pinned.to_string()).font(k.ty.ui_medium).color(c.text_base))
                .child(text(said).font(k.ty.ui).color(c.text_muted).wrap())
                .child(row().child(unpin_button(ButtonVariant::Outline)));
        }
    };
    let meta: Vec<String> =
        [issue.assignee.as_ref().map(|a| a.name.clone()), Some(issue.priority_label.clone()).filter(|p| p != "No priority")]
            .into_iter()
            .flatten()
            .collect();
    let (open, send, send_to) = (issue.url.clone(), issue.url.clone(), key.clone());
    let mut head = col()
        .key("linear-card")
        .w_full()
        .px(12.0)
        .pb(12.0)
        .gap(6.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], c.border_weak)
        .child(
            row()
                .gap(8.0)
                .center_y()
                .child(text(issue.identifier.clone()).font(k.ty.small).color(c.text_muted))
                .child(state_dot(issue, k))
                .child(text(issue.state.name.clone()).font(k.ty.small).color(c.text_base)),
        )
        .child(text(issue.title.clone()).font(k.ty.ui_medium).color(c.text_base).wrap());
    if !meta.is_empty() {
        head = head.child(text(meta.join(" · ")).font(k.ty.small).color(c.text_muted));
    }
    let buttons = row()
        .pt(4.0)
        .gap(8.0)
        .child(
            Button::new("Send to agent")
                .variant(ButtonVariant::Contrast)
                .control_size(ControlSize::Small)
                .help("Pastes its link at the thread's prompt")
                .on_press_tx(move |s: &mut Lemur, tx: &mut Tx| s.paste_into(&send_to, &send, tx))
                .build(k)
                .key("linear-send"),
        )
        .child(
            Button::new("Open in Linear")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .on_press_tx(move |_: &mut Lemur, tx: &mut Tx| {
                    let _ = tx.open_url(&open);
                })
                .build(k)
                .key("linear-open"),
        )
        .child(unpin_button(ButtonVariant::Ghost));
    let description = issue.description.clone().unwrap_or_default();
    let body = if description.trim().is_empty() {
        text("No description.").font(k.ty.ui).color(c.text_muted)
    } else {
        Markdown::new(description).selectable(true).build(k)
    };
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(head.child(buttons))
        .child(col().key("linear-description").w_full().grow(1.0).min_h(0.0).padding(12.0).scroll(ScrollSpec { stick_bottom: false }).child(body))
}

/// Asks you to sign in, or says it's under way.
fn signed_out(state: &Account, k: &Kit) -> El {
    let c = &k.c;
    let body = col().key("linear-sign-in").w_full().px(12.0).py(4.0).gap(12.0);
    let said = |words: &str| text(words.to_string()).font(k.ty.ui).color(c.text_muted).wrap();
    match state {
        Account::Checking => body.child(said("Checking Linear…")),
        Account::SigningIn(_) => body.child(said("Approve Lemur in the browser window that opened, then come back here.")).child(
            row().child(
                Button::new("Cancel")
                    .variant(ButtonVariant::Outline)
                    .control_size(ControlSize::Small)
                    .on_press(Lemur::cancel_linear_sign_in)
                    .build(k)
                    .key("linear-cancel-sign-in"),
            ),
        ),
        Account::SignedOut(error) => {
            let body = body.child(said("Sign in to Linear to search its issues and pin them to threads. Lemur only reads.")).child(
                row().child(
                    Button::new("Sign in with Linear")
                        .variant(ButtonVariant::Contrast)
                        .control_size(ControlSize::Small)
                        .on_press(Lemur::sign_in_linear)
                        .build(k)
                        .key("linear-sign-in-button"),
                ),
            );
            match error {
                Some(e) => body.child(text(e.clone()).font(k.ty.small).color(c.danger.fg).wrap()),
                None => body,
            }
        }
        Account::SignedIn(_) => body,
    }
}

pub fn view(s: &Lemur, k: &Kit) -> El {
    let p = &s.panels.linear;
    let c = &k.c;
    let title = match &p.thread {
        Some((_, label, _)) => format!("Linear · {label}"),
        None => "Linear".to_string(),
    };
    let header = row()
        .key("linear-head")
        .w_full()
        .h(40.0)
        .px(12.0)
        .center_y()
        .child(text(title).font(k.ty.ui_medium).color(c.text_base).truncate().grow(1.0).shrink());
    let panel = col().key("linear-panel").w_full().h_full().min_h(0.0).child(header);
    if !p.signed_in() {
        return panel.child(signed_out(&p.account, k));
    }
    let typed = !p.query.text.trim().is_empty();
    let mut search = TextInput::new(&p.query)
        .placeholder("Search issues, or paste a key or a link")
        .leading_icon(&icons::SEARCH)
        .width(None)
        .on_edit(|s: &mut Lemur| &mut s.panels.linear.query)
        .on_change(|s: &mut Lemur, _| s.linear_search())
        .on_key(|s: &mut Lemur, ev, tx| {
            // Esc gives the keyboard back to the thread on show.
            if ev.key == K::Escape {
                s.panels.linear.query.focused = false;
                tx.blur();
                return true;
            }
            false
        });
    if typed {
        search = search.clear_button().on_action(|s: &mut Lemur, _| {
            s.panels.linear.query.clear();
            s.linear_search();
        });
    }
    let search = row().w_full().px(12.0).pb(8.0).child(search.build(k).key("linear-search"));
    let pinned = p.thread.as_ref().and_then(|(key, _, issue)| Some((key, issue.as_deref()?)));
    let body = if typed {
        match &p.found {
            _ if p.searching => note("linear-searching", "Searching…".into(), k),
            Some((_, Ok(found))) if found.is_empty() => note("linear-no-match", "No issues match.".into(), k),
            Some((_, Ok(found))) => issue_rows(p, found, k),
            Some((_, Err(e))) => note("linear-error", e.clone(), k),
            None => note("linear-searching", "Searching…".into(), k),
        }
    } else if let Some((key, issue)) = pinned {
        card(p, key, issue, k)
    } else {
        let mine = match &p.mine {
            None => note("linear-asking", "Asking Linear…".into(), k),
            Some((_, Err(e))) => note("linear-error", e.clone(), k),
            Some((_, Ok(mine))) if mine.is_empty() => note("linear-none", "Nothing is assigned to you.".into(), k),
            Some((_, Ok(mine))) => issue_rows(p, mine, k),
        };
        let caption = if p.thread.is_some() {
            "Assigned to you · click one to pin it to this thread"
        } else {
            "Assigned to you · open a thread to pin one to it"
        };
        col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(text(caption).font(k.ty.small_medium).color(c.text_muted).px(12.0).py(4.0))
            .child(mine)
    };
    panel.child(search).child(body)
}

#[cfg(test)]
#[path = "linear_panel_tests.rs"]
mod tests;

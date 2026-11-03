//! The Linear panel, on the right as the notes, the changes and the agents
//! are: the Linear issue pinned to the thread on show, to open in Linear or
//! paste at its agent's prompt, and a search for issues to pin. Lemur signs
//! in with its own Linear app (`crate::linear`); Settings → Linear signs
//! out. It asks Linear only while it shows (as it's drawn, and as the
//! window points it at the thread on show), at most every `FRESH` for the
//! same issues.

use super::Key;
use crate::linear::{self, Issue, Linear};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

/// How long what Linear said of an issue, or of yours, is shown before
/// it's asked again.
const FRESH: Duration = Duration::from_secs(120);
/// How long typing pauses before a search: Linear allows 30 a minute.
const TYPING: Duration = Duration::from_millis(350);

/// Lemur's Linear, and whether it's signed in. The panel and Settings
/// follow it.
#[derive(Clone)]
pub(super) struct LinearAccount {
    pub(super) linear: Linear,
    pub(super) state: Account,
}

impl Global for LinearAccount {}

#[derive(Clone)]
pub(super) enum Account {
    /// Asking Linear whether the sign-in kept still works.
    Checking,
    /// With why the last sign-in failed, if it did.
    SignedOut(Option<String>),
    /// Waiting for you in the browser; set the flag to call it off.
    SigningIn(Arc<AtomicBool>),
    /// As whom, once Linear has said.
    SignedIn(Option<linear::Account>),
}

impl LinearAccount {
    pub(super) fn get(cx: &App) -> &LinearAccount {
        static DEFAULT: LazyLock<LinearAccount> = LazyLock::new(|| LinearAccount {
            linear: Linear::new(),
            state: Account::SignedOut(None),
        });
        cx.try_global().unwrap_or(&DEFAULT)
    }

    fn signed_in(cx: &App) -> bool {
        matches!(LinearAccount::get(cx).state, Account::SignedIn(_))
    }
}

fn set_account(state: Account, cx: &mut App) {
    let linear = LinearAccount::get(cx).linear.clone();
    cx.set_global(LinearAccount { linear, state });
}

/// Starts following `linear`: whether its sign-in is kept, and still
/// works.
pub(super) fn init(linear: Linear, cx: &mut App) {
    cx.set_global(LinearAccount {
        linear: linear.clone(),
        state: Account::Checking,
    });
    cx.spawn(async move |cx| {
        let state = cx
            .background_spawn(async move {
                match linear.signed_in() {
                    Ok(false) => Account::SignedOut(None),
                    Err(e) => Account::SignedOut(Some(format!("{e:#}"))),
                    Ok(true) => match linear.account() {
                        Ok(account) => Account::SignedIn(Some(account)),
                        Err(e) if linear::is_signed_out(&e) => Account::SignedOut(None),
                        // Linear can't be reached: the sign-in is kept.
                        Err(_) => Account::SignedIn(None),
                    },
                }
            })
            .await;
        cx.update(|cx| checked(state, cx));
    })
    .detach();
}

/// Takes what checking the kept sign-in found, unless a sign-in started
/// meanwhile: its Cancel would go, and its answer would be lost.
fn checked(state: Account, cx: &mut App) {
    if matches!(LinearAccount::get(cx).state, Account::Checking) {
        set_account(state, cx);
    }
}

/// Opens Linear's page asking you to approve Lemur, and waits for the
/// browser to come back with the answer.
pub(super) fn sign_in(cx: &mut App) {
    let account = LinearAccount::get(cx);
    if matches!(account.state, Account::SigningIn(_)) {
        return;
    }
    let linear = account.linear.clone();
    let sign_in = match linear.start_sign_in() {
        Ok(sign_in) => sign_in,
        Err(e) => return set_account(Account::SignedOut(Some(format!("{e:#}"))), cx),
    };
    cx.open_url(&sign_in.url());
    let cancelled = Arc::new(AtomicBool::new(false));
    set_account(Account::SigningIn(cancelled.clone()), cx);
    cx.spawn(async move |cx| {
        let done = cx
            .background_spawn(async move { linear.finish_sign_in(sign_in, &cancelled) })
            .await;
        cx.update(|cx| {
            let state = match done {
                Ok(account) => {
                    // Back from the browser.
                    cx.activate(true);
                    Account::SignedIn(Some(account))
                }
                Err(e) if linear::is_cancelled(&e) => Account::SignedOut(None),
                Err(e) => Account::SignedOut(Some(format!("{e:#}"))),
            };
            set_account(state, cx);
        });
    })
    .detach();
}

pub(super) fn cancel_sign_in(cx: &mut App) {
    if let Account::SigningIn(cancelled) = &LinearAccount::get(cx).state {
        cancelled.store(true, Ordering::Relaxed);
    }
}

/// Forgets the sign-in, and tells Linear so.
pub(super) fn sign_out(cx: &mut App) {
    let linear = LinearAccount::get(cx).linear.clone();
    set_account(Account::SignedOut(None), cx);
    cx.background_spawn(async move { linear.sign_out() })
        .detach();
}

/// What the panel asks the window to do for the thread on show.
pub(super) enum LinearEvent {
    /// Pin this issue to the thread, by its key, or with `None` unpin it.
    Pin(Key, Option<String>),
    /// Paste this link at the thread's prompt.
    Send(Key, String),
}

impl EventEmitter<LinearEvent> for LinearPanel {}

/// The thread on show: its key, its name, and the issue pinned to it.
pub(super) type Thread = (Key, String, Option<String>);

pub(super) struct LinearPanel {
    thread: Option<Thread>,
    query: Entity<InputState>,
    /// What was searched for, and what Linear found.
    found: Option<(String, Result<Vec<Issue>, String>)>,
    searching: bool,
    /// The issues assigned to you, and when Linear said.
    mine: Option<(Instant, Result<Vec<Issue>, String>)>,
    /// Pinned issues, by key, as Linear last said, and when.
    issues: HashMap<String, (Instant, Result<Issue, String>)>,
    _query: Subscription,
    _account: Subscription,
    _search: Option<Task<()>>,
    _mine: Option<Task<()>>,
    _issue: Option<Task<()>>,
}

impl LinearPanel {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search issues, or paste a key or a link")
        });
        let typed = cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                this.search(cx);
            }
        });
        // What Linear said was another sign-in's, or an error it no longer
        // has: asked again, as it's drawn.
        let account = cx.observe_global::<LinearAccount>(|this, cx| {
            this.mine = None;
            this.issues.clear();
            this.found = None;
            if LinearAccount::signed_in(cx) {
                this.search(cx);
            }
            cx.notify();
        });
        LinearPanel {
            thread: None,
            query,
            found: None,
            searching: false,
            mine: None,
            issues: HashMap::new(),
            _query: typed,
            _account: account,
            _search: None,
            _mine: None,
            _issue: None,
        }
    }

    /// Shows `thread`'s issue, or yours when none is pinned to it; asked
    /// again once they're no longer fresh. The window calls it while the
    /// panel shows.
    pub(super) fn show(&mut self, thread: Option<Thread>, cx: &mut Context<Self>) {
        if self.thread != thread {
            self.thread = thread;
            cx.notify();
        }
        self.fetch(cx);
    }

    #[cfg(test)]
    pub(super) fn shown(&self) -> Option<&Thread> {
        self.thread.as_ref()
    }

    /// Asks Linear for what shows, unless it said so lately.
    fn fetch(&mut self, cx: &mut Context<Self>) {
        if !LinearAccount::signed_in(cx) {
            return;
        }
        let stale = |at: Option<Instant>| at.is_none_or(|at| at.elapsed() >= FRESH);
        let linear = LinearAccount::get(cx).linear.clone();
        let pinned = self.thread.as_ref().and_then(|(_, _, issue)| issue.clone());
        match pinned {
            Some(key) => {
                if self._issue.is_some() || !stale(self.issues.get(&key).map(|(at, _)| *at)) {
                    return;
                }
                self._issue = Some(cx.spawn(async move |this, cx| {
                    let asked = key.clone();
                    let issue = cx
                        .background_spawn(async move { linear.issue(&asked) })
                        .await;
                    this.update(cx, |this, cx| {
                        this._issue = None;
                        let issue = said(issue, cx);
                        this.issues.insert(key, (Instant::now(), issue));
                        // Drawn again, it asks for one pinned meanwhile.
                        cx.notify();
                    })
                    .ok();
                }));
            }
            None => {
                if self._mine.is_some() || !stale(self.mine.as_ref().map(|(at, _)| *at)) {
                    return;
                }
                self._mine = Some(cx.spawn(async move |this, cx| {
                    let mine = cx.background_spawn(async move { linear.mine() }).await;
                    this.update(cx, |this, cx| {
                        this._mine = None;
                        this.mine = Some((Instant::now(), said(mine, cx)));
                        cx.notify();
                    })
                    .ok();
                }));
            }
        }
    }

    /// Searches for what's typed, once typing pauses.
    fn search(&mut self, cx: &mut Context<Self>) {
        let term = self.query.read(cx).value().trim().to_string();
        if term.is_empty() {
            self.found = None;
            self.searching = false;
            self._search = None;
            cx.notify();
            return;
        }
        // Back to what was found: a search for what was typed meanwhile
        // would answer for a term no longer in the box.
        if self.found.as_ref().is_some_and(|(t, _)| *t == term) {
            self.searching = false;
            self._search = None;
            cx.notify();
            return;
        }
        let linear = LinearAccount::get(cx).linear.clone();
        self.searching = true;
        cx.notify();
        self._search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TYPING).await;
            let asked = term.clone();
            let found = cx
                .background_spawn(async move { linear.search(&asked) })
                .await;
            this.update(cx, |this, cx| {
                this.searching = false;
                this.found = Some((term, said(found, cx)));
                cx.notify();
            })
            .ok();
        }));
    }

    /// Pins `issue` to the thread on show, or opens it in Linear with no
    /// thread on show.
    fn pick(&mut self, issue: &Issue, window: &mut Window, cx: &mut Context<Self>) {
        let Some((key, _, _)) = &self.thread else {
            cx.open_url(&issue.url);
            return;
        };
        // Found by its key, it came with its description; otherwise that's
        // asked for once it's pinned.
        if issue.description.is_some() {
            let shown = (Instant::now(), Ok(issue.clone()));
            self.issues.insert(issue.identifier.clone(), shown);
        } else {
            self.issues.remove(&issue.identifier);
        }
        cx.emit(LinearEvent::Pin(
            key.clone(),
            Some(issue.identifier.clone()),
        ));
        self.query.update(cx, |q, cx| q.set_value("", window, cx));
        self.found = None;
        cx.notify();
    }

    fn issue_rows(&self, issues: &[Issue], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        issues
            .iter()
            .enumerate()
            .map(|(i, issue)| {
                let picked = issue.clone();
                let pinned = self.thread.as_ref().and_then(|(_, _, p)| p.as_deref())
                    == Some(issue.identifier.as_str());
                h_flex()
                    .id(SharedString::from(format!("issue-{i}")))
                    .debug_selector(move || format!("issue {i}"))
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .rounded(theme.radius)
                    .text_sm()
                    .cursor_pointer()
                    .map(|row| {
                        if pinned {
                            row.bg(theme.tokens.sidebar_accent)
                        } else {
                            row.hover(|row| row.bg(theme.sidebar_accent.opacity(0.5)))
                        }
                    })
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.pick(&picked, window, cx)),
                    )
                    .child(state_dot(issue, theme.muted_foreground))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(issue.identifier.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(issue.title.clone()),
                    )
                    .into_any_element()
            })
            .collect()
    }

    /// The issue pinned to the thread on show: what it is, where it's at,
    /// and what to do with it.
    fn card(&self, key: &Key, pinned: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let issue = match self.issues.get(pinned) {
            Some((_, Ok(issue))) => issue.clone(),
            other => {
                let note = match other {
                    Some((_, Err(e))) => e.clone(),
                    _ => "Asking Linear…".into(),
                };
                let unpin = key.clone();
                return v_flex()
                    .p_3()
                    .gap_2()
                    .child(div().text_sm().font_semibold().child(pinned.to_string()))
                    .child(div().text_sm().text_color(muted).child(note))
                    .child(
                        h_flex().child(Button::new("unpin").small().label("Unpin").on_click(
                            cx.listener(move |_, _, _, cx| {
                                cx.emit(LinearEvent::Pin(unpin.clone(), None))
                            }),
                        )),
                    )
                    .into_any_element();
            }
        };
        let meta: Vec<String> = [
            issue.assignee.as_ref().map(|a| a.name.clone()),
            Some(issue.priority_label.clone()).filter(|p| p != "No priority"),
        ]
        .into_iter()
        .flatten()
        .collect();
        let (open, send, unpin) = (issue.url.clone(), issue.url.clone(), key.clone());
        let send_to = key.clone();
        let description = issue.description.clone().unwrap_or_default();
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                v_flex()
                    .flex_shrink_0()
                    .px_3()
                    .pb_3()
                    .gap_1p5()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .text_xs()
                            .child(div().text_color(muted).child(issue.identifier.clone()))
                            .child(state_dot(&issue, muted))
                            .child(issue.state.name.clone()),
                    )
                    .child(div().text_sm().font_semibold().child(issue.title.clone()))
                    .when(!meta.is_empty(), |card| {
                        card.child(div().text_xs().text_color(muted).child(meta.join(" · ")))
                    })
                    .child(
                        h_flex()
                            .pt_1()
                            .gap_2()
                            .child(
                                div().debug_selector(|| "send to agent".into()).child(
                                    Button::new("send")
                                        .small()
                                        .primary()
                                        .label("Send to agent")
                                        .tooltip("Pastes its link at the thread's prompt")
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.emit(LinearEvent::Send(
                                                send_to.clone(),
                                                send.clone(),
                                            ))
                                        })),
                                ),
                            )
                            .child(
                                Button::new("open")
                                    .small()
                                    .label("Open in Linear")
                                    .on_click(move |_, _, cx| cx.open_url(&open)),
                            )
                            .child(
                                div().debug_selector(|| "unpin".into()).child(
                                    Button::new("unpin")
                                        .small()
                                        .ghost()
                                        .label("Unpin")
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.emit(LinearEvent::Pin(unpin.clone(), None))
                                        })),
                                ),
                            ),
                    ),
            )
            .child(
                div()
                    .id("issue-description")
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .overflow_y_scroll()
                    .text_sm()
                    .child(if description.trim().is_empty() {
                        div()
                            .text_color(muted)
                            .child("No description.")
                            .into_any_element()
                    } else {
                        TextView::markdown("issue-markdown", description)
                            .selectable(true)
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    /// Asks you to sign in, or says it's under way.
    fn signed_out(&self, state: &Account, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let body = v_flex().p_3().gap_3().text_sm();
        match state {
            Account::Checking => body.text_color(muted).child("Checking Linear…"),
            Account::SigningIn(_) => body
                .child(
                    div().text_color(muted).child(
                        "Approve Lemur in the browser window that opened, then come back here.",
                    ),
                )
                .child(
                    h_flex().child(
                        Button::new("cancel-sign-in")
                            .small()
                            .label("Cancel")
                            .on_click(|_, _, cx| cancel_sign_in(cx)),
                    ),
                ),
            Account::SignedOut(error) => body
                .child(div().text_color(muted).child(
                    "Sign in to Linear to search its issues and pin them to threads. \
                     Lemur only reads.",
                ))
                .child(
                    h_flex().child(
                        Button::new("sign-in")
                            .small()
                            .primary()
                            .label("Sign in with Linear")
                            .on_click(|_, _, cx| sign_in(cx)),
                    ),
                )
                .children(
                    error
                        .clone()
                        .map(|e| div().text_xs().text_color(danger).child(e)),
                ),
            Account::SignedIn(_) => body,
        }
        .into_any_element()
    }
}

/// What Linear said, or why it couldn't; a sign-in Linear turned down
/// signs Lemur out.
fn said<T>(answer: anyhow::Result<T>, cx: &mut App) -> Result<T, String> {
    answer.map_err(|e| {
        if linear::is_signed_out(&e) {
            set_account(Account::SignedOut(None), cx);
        }
        format!("{e:#}")
    })
}

/// A dot in the color of the issue's state.
fn state_dot(issue: &Issue, fallback: Hsla) -> Div {
    let color = u32::from_str_radix(issue.state.color.trim_start_matches('#'), 16)
        .map(|c| Hsla::from(rgb(c)))
        .unwrap_or(fallback);
    div().flex_shrink_0().size_2().rounded_full().bg(color)
}

impl Render for LinearPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Only drawn while it shows.
        self.fetch(cx);
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let title = match &self.thread {
            Some((_, label, _)) => format!("Linear · {label}"),
            None => "Linear".to_string(),
        };
        let header = h_flex().h_10().flex_shrink_0().px_3().items_center().child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .font_semibold()
                .child(title),
        );
        let state = LinearAccount::get(cx).state.clone();
        if !matches!(state, Account::SignedIn(_)) {
            return v_flex()
                .size_full()
                .child(header)
                .child(self.signed_out(&state, cx));
        }
        let search = div().px_3().pb_2().flex_shrink_0().child(
            Input::new(&self.query)
                .small()
                .prefix(Icon::new(IconName::Search).small().text_color(muted))
                .cleanable(true),
        );
        let note = |text: String| div().px_3().py_1().text_sm().text_color(muted).child(text);
        let list = |rows: Vec<AnyElement>| {
            v_flex()
                .id("linear-issues")
                .flex_1()
                .min_h_0()
                .px_1()
                .gap_0p5()
                .overflow_y_scroll()
                .children(rows)
                .into_any_element()
        };
        let typed = !self.query.read(cx).value().trim().is_empty();
        let pinned = self
            .thread
            .as_ref()
            .and_then(|(key, _, issue)| Some((key.clone(), issue.clone()?)));
        let body = if typed {
            match &self.found {
                _ if self.searching => note("Searching…".into()).into_any_element(),
                Some((_, Ok(found))) if found.is_empty() => {
                    note("No issues match.".into()).into_any_element()
                }
                Some((_, Ok(found))) => list(self.issue_rows(found, cx)),
                Some((_, Err(e))) => note(e.clone()).into_any_element(),
                None => note("Searching…".into()).into_any_element(),
            }
        } else if let Some((key, issue)) = pinned {
            self.card(&key, &issue, cx)
        } else {
            let mine = match &self.mine {
                None => vec![note("Asking Linear…".into()).into_any_element()],
                Some((_, Err(e))) => vec![note(e.clone()).into_any_element()],
                Some((_, Ok(mine))) if mine.is_empty() => {
                    vec![note("Nothing is assigned to you.".into()).into_any_element()]
                }
                Some((_, Ok(mine))) => self.issue_rows(mine, cx),
            };
            v_flex()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .font_semibold()
                        .text_color(muted)
                        .child(if self.thread.is_some() {
                            "Assigned to you · click one to pin it to this thread"
                        } else {
                            "Assigned to you · open a thread to pin one to it"
                        }),
                )
                .child(list(mine))
                .into_any_element()
        };
        v_flex().size_full().child(header).child(search).child(body)
    }
}

/// Settings' Linear page: who Lemur is signed in as, and signing in or out.
pub(super) fn settings_status(cx: &App) -> (String, Option<String>) {
    match &LinearAccount::get(cx).state {
        Account::Checking => ("Checking…".into(), None),
        Account::SignedOut(error) => ("Not signed in".into(), error.clone()),
        Account::SigningIn(_) => ("Waiting for you in the browser…".into(), None),
        Account::SignedIn(Some(a)) => (format!("{} · {}", a.name, a.workspace), None),
        Account::SignedIn(None) => ("Signed in · Linear can't be reached".into(), None),
    }
}

/// Whether Settings offers Sign out (signed in), Cancel (signing in) or
/// Sign in.
pub(super) fn settings_action(cx: &App) -> (&'static str, fn(&mut App)) {
    match &LinearAccount::get(cx).state {
        Account::SignedIn(_) => ("Sign out", sign_out),
        Account::SigningIn(_) => ("Cancel", cancel_sign_in),
        _ => ("Sign in with Linear", sign_in),
    }
}

#[cfg(test)]
mod tests {
    use super::{Account, LinearAccount, LinearPanel, checked, init, set_account};
    use crate::linear::{Linear, Store};
    use crate::test_support::{FakeLinear, TempDir, wait_until};
    use gpui::VisualTestContext;
    use gpui_kit::{AppContext, Entity};
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    /// A stand-in Linear with one issue per search term, signed in, and
    /// the panel in a window.
    fn panel<'a>(
        tag: &str,
        cx: &'a mut gpui::TestAppContext,
    ) -> (
        FakeLinear,
        TempDir,
        Entity<LinearPanel>,
        &'a mut VisualTestContext,
    ) {
        let api = FakeLinear::new(|r| {
            let body: serde_json::Value = serde_json::from_str(&r.body).unwrap();
            let term = body["variables"]["term"].as_str().unwrap_or("mine");
            let issue = json!({
                "identifier": "LAFT-1", "title": term, "url": "https://linear.app/x/issue/LAFT-1",
                "branchName": "b", "priorityLabel": "High",
                "state": { "name": "Todo", "color": "#aaaaaa", "type": "unstarted" },
                "assignee": null,
            });
            let data = if body["query"].as_str().unwrap().contains("searchIssues") {
                json!({ "searchIssues": { "nodes": [issue] } })
            } else if body["query"].as_str().unwrap().contains("assignedIssues") {
                json!({ "viewer": { "assignedIssues": { "nodes": [issue] } } })
            } else {
                json!({ "viewer": { "name": "M" }, "organization": { "name": "L" } })
            };
            (200, json!({ "data": data }).to_string())
        });
        let dir = TempDir::new(tag);
        let linear = Linear::at(&api.url, Store::File(dir.join("tokens")));
        linear.sign_in_as("a").unwrap();
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        cx.update(|cx| init(linear, cx));
        let mut made = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| LinearPanel::new(window, cx));
            made = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        let signed_in = wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            cx.update(|_, cx| LinearAccount::signed_in(cx))
        });
        assert!(signed_in);
        (api, dir, made.unwrap(), cx)
    }

    /// Typed back to what was found, the panel shows what was found, not
    /// what a search for the longer term answers.
    #[gpui::test]
    fn typed_back_the_search_shows_what_was_found(cx: &mut gpui::TestAppContext) {
        let (_api, _dir, panel, cx) = panel("linear-panel-search", cx);
        let type_in = |text: &str, cx: &mut VisualTestContext| {
            panel.update_in(cx, |p, window, cx| {
                p.query.update(cx, |q, cx| q.set_value(text, window, cx));
                p.search(cx);
            });
        };
        let found = |cx: &mut VisualTestContext| {
            panel.read_with(cx, |p, _| {
                let titles = p
                    .found
                    .as_ref()
                    .and_then(|(_, f)| f.as_ref().ok())
                    .map(|f| f.iter().map(|i| i.title.clone()).collect::<Vec<_>>());
                (p.searching, titles)
            })
        };
        // Typing pauses: the clock moves past the debounce.
        type_in("lease", cx);
        assert!(wait_until(Duration::from_secs(5), || {
            cx.executor().advance_clock(Duration::from_millis(400));
            cx.run_until_parked();
            found(cx) == (false, Some(vec!["lease".into()]))
        }));
        type_in("leasex", cx);
        assert!(found(cx).0, "searching");
        type_in("lease", cx);
        assert_eq!(found(cx), (false, Some(vec!["lease".to_string()])));
        for _ in 0..10 {
            cx.executor().advance_clock(Duration::from_millis(400));
            std::thread::sleep(Duration::from_millis(30));
            cx.run_until_parked();
        }
        assert_eq!(
            found(cx),
            (false, Some(vec!["lease".to_string()])),
            "not leasex's"
        );
    }

    /// Signed out and in again, what Linear said before isn't shown.
    #[gpui::test]
    fn a_new_sign_in_forgets_what_linear_said(cx: &mut gpui::TestAppContext) {
        let (_api, _dir, panel, cx) = panel("linear-panel-forget", cx);
        panel.update(cx, |p, cx| p.show(None, cx));
        assert!(wait_until(Duration::from_secs(5), || {
            cx.run_until_parked();
            panel.read_with(cx, |p, _| p.mine.is_some())
        }));
        cx.update(|_, cx| set_account(Account::SignedOut(None), cx));
        cx.run_until_parked();
        assert!(panel.read_with(cx, |p, _| p.mine.is_none()));
    }

    /// A sign-in started while Lemur still checks the one it kept isn't
    /// undone when the check ends: its Cancel stays.
    #[gpui::test]
    fn a_check_ending_leaves_a_sign_in_under_way(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(LinearAccount {
                linear: Linear::new(),
                state: Account::Checking,
            });
            checked(Account::SignedOut(None), cx);
            assert!(matches!(
                LinearAccount::get(cx).state,
                Account::SignedOut(_)
            ));
            set_account(Account::SigningIn(Arc::new(AtomicBool::new(false))), cx);
            checked(Account::SignedOut(None), cx);
            assert!(matches!(
                LinearAccount::get(cx).state,
                Account::SigningIn(_)
            ));
        });
    }
}

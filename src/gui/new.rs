//! Threads that start at once, without a name asked for: a terminal (⌘T)
//! or Claude, where the thread on show runs or in a worktree, and the menu
//! offering them, with their keys, as Herdr's does.

use super::sidebar::claude_icon;
use super::snackbar::Kind;
use super::{Ask, Lemur, NewTerminal, NewThreadHere, rows};
use crate::actions;
use gpui_kit::component::IconName;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;
use std::rc::Rc;

impl Lemur {
    /// ⌘T: a terminal where the thread on show runs: its worktree, or its
    /// folder outside every worktree.
    pub(super) fn new_terminal_here(&mut self, cx: &mut Context<Self>) {
        if self.prompt.is_some() || self.picker.is_some() {
            return;
        }
        let Some(key) = self.active.clone() else {
            return;
        };
        let dir = match self.worktree_of(&key.0, &key.1) {
            Some((worktree, _)) => worktree,
            None => match self.session_of(&key) {
                Some(s) => s.path.clone(),
                None => return,
            },
        };
        self.start_now(key.0, dir, true, cx);
    }

    /// Starts a shell, or with `shell` false Claude, in `dir` on `host`,
    /// labelled `shell` or `claude`, and shows it.
    pub(super) fn start_now(
        &mut self,
        host: String,
        dir: String,
        shell: bool,
        cx: &mut Context<Self>,
    ) {
        let label = if shell { "shell" } else { "claude" };
        self.status = Some((format!("starting {label}…"), Kind::Progress));
        cx.notify();
        let cfg = self.cfg.clone();
        self.in_background(
            cx,
            move || actions::start_in(&cfg, &host, &dir, label, shell),
            |this, started, window, cx| {
                match started {
                    Ok(started) => {
                        this.status = None;
                        if let Some(claude) = started.claude {
                            this.keep_thread(claude, cx);
                        }
                        this.open_fresh(started.host, started.session, window, cx);
                    }
                    Err(e) => this.fail(e, cx),
                }
                this.refresh(cx).detach();
            },
        )
        .detach();
    }

    /// What can start in `dir` on `host`: a terminal or Claude at once, and
    /// in a project's worktree (`base`) Claude or a shell named in the
    /// spotlight. Each shows the key that does it for the thread on show.
    pub(super) fn new_menu(
        &self,
        host: &str,
        dir: &str,
        base: Option<usize>,
        cx: &mut Context<Self>,
    ) -> rows::MenuBuilder {
        let this = cx.entity().downgrade();
        let (host, dir) = (host.to_string(), dir.to_string());
        Rc::new(
            move |menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
                let start = |shell: bool| {
                    let (this, host, dir) = (this.clone(), host.clone(), dir.clone());
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        this.update(cx, |this, cx| {
                            this.start_now(host.clone(), dir.clone(), shell, cx)
                        })
                        .ok();
                    }
                };
                let menu = menu
                    .item(
                        PopupMenuItem::new("New terminal")
                            .icon(IconName::SquareTerminal)
                            .action(Box::new(NewTerminal))
                            .on_click(start(true)),
                    )
                    .item(
                        PopupMenuItem::new("New Claude")
                            .icon(claude_icon())
                            .on_click(start(false)),
                    );
                let Some(base) = base else {
                    return menu;
                };
                let (this, worktree) = (this.clone(), dir.clone());
                menu.item(
                    PopupMenuItem::new("Claude or shell, named…")
                        .icon(IconName::Plus)
                        .action(Box::new(NewThreadHere))
                        .on_click(move |_, window, cx| {
                            let ask = Ask::NewThread {
                                base,
                                worktree: worktree.clone(),
                                shell: false,
                            };
                            this.update(cx, |this, cx| this.ask_name(ask, window, cx))
                                .ok();
                        }),
                )
            },
        )
    }
}

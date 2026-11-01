//! The command palette (⌘⇧P), as Herdr's: every command by name, with its
//! key. What's typed narrows them down; ↑↓ picks one and ⏎ runs it.

use super::prompt::{Down, Up};
use super::{
    AddProject, CloseThread, Dictate, GoToThread, Lemur, NewTerminal, NewThreadHere, NextAgent,
    NextThread, OpenSettings, PreviousAgent, PreviousThread, PutAway, Quit, Refresh, ToggleAgents,
    ToggleChanges, ToggleLinear, ToggleNotes, ToggleRightPanel, ToggleSidebar, UpdateCore,
    spotlight,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme, IconName, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// Key context of the palette, for its ↑↓.
const PALETTE: &str = "Palette";

/// How many commands it lists at most.
const SHOWN: usize = 10;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", Up, Some(PALETTE)),
        KeyBinding::new("down", Down, Some(PALETTE)),
    ]);
}

pub(super) struct Palette {
    input: Entity<InputState>,
    /// The command picked among those listed.
    picked: usize,
    _events: Subscription,
}

/// Every command, by name: what the menus have.
fn commands() -> Vec<(&'static str, Box<dyn Action>)> {
    vec![
        ("Go to Thread…", Box::new(GoToThread)),
        ("New Thread in Worktree…", Box::new(NewThreadHere)),
        ("New Terminal", Box::new(NewTerminal)),
        ("Close Thread", Box::new(CloseThread)),
        ("Put Thread Away", Box::new(PutAway)),
        ("Next Agent", Box::new(NextAgent)),
        ("Previous Agent", Box::new(PreviousAgent)),
        ("Next Thread in Worktree", Box::new(NextThread)),
        ("Previous Thread in Worktree", Box::new(PreviousThread)),
        ("Toggle Sidebar", Box::new(ToggleSidebar)),
        ("Toggle Right Panel", Box::new(ToggleRightPanel)),
        ("Toggle Notes", Box::new(ToggleNotes)),
        ("Toggle Linear", Box::new(ToggleLinear)),
        ("Toggle Changes", Box::new(ToggleChanges)),
        ("Toggle Agents", Box::new(ToggleAgents)),
        ("Add Project…", Box::new(AddProject)),
        ("Dictate", Box::new(Dictate)),
        ("Refresh", Box::new(Refresh)),
        ("Settings…", Box::new(OpenSettings)),
        ("Update herdr-core", Box::new(UpdateCore)),
        ("Quit Lemur", Box::new(Quit)),
    ]
}

/// The commands whose name has every word typed, in the palette's order.
fn matching(typed: &str) -> Vec<(&'static str, Box<dyn Action>)> {
    let typed = typed.to_lowercase();
    let words: Vec<&str> = typed.split_whitespace().collect();
    commands()
        .into_iter()
        .filter(|(name, _)| {
            let name = name.to_lowercase();
            words.iter().all(|w| name.contains(w))
        })
        .collect()
}

impl Lemur {
    /// ⌘⇧P: opens the palette, over the window.
    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompt.is_some() || self.picker.is_some() || self.palette.is_some() {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Run a command…"));
        let events =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.run_picked(window, cx),
                    InputEvent::Change => {
                        if let Some(palette) = &mut this.palette {
                            palette.picked = 0;
                        }
                        cx.notify();
                    }
                    _ => {}
                },
            );
        input.update(cx, |i, cx| i.focus(window, cx));
        self.palette = Some(Palette {
            input,
            picked: 0,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.focus_terminal(window, cx);
        cx.notify();
    }

    fn palette_matches(&self, cx: &App) -> Vec<(&'static str, Box<dyn Action>)> {
        let Some(palette) = &self.palette else {
            return vec![];
        };
        matching(&palette.input.read(cx).value())
    }

    /// Moves the pick one down, or with `back` up, round the list.
    fn step_palette(&mut self, back: bool, cx: &mut Context<Self>) {
        let count = self.palette_matches(cx).len().min(SHOWN);
        let Some(palette) = &mut self.palette else {
            return;
        };
        if count > 0 {
            palette.picked = if back {
                (palette.picked + count - 1) % count
            } else {
                (palette.picked + 1) % count
            };
        }
        cx.notify();
    }

    /// Runs the command picked, as its key or menu item would, once the
    /// palette is gone and the terminal on show has the keyboard back.
    fn run_picked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picked) = self.palette.as_ref().map(|p| p.picked) else {
            return;
        };
        let action = self.palette_matches(cx).into_iter().nth(picked);
        self.close_palette(window, cx);
        if let Some((_, action)) = action {
            window.dispatch_action(action, cx);
        }
    }

    pub(super) fn palette_card(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let palette = self.palette.as_ref()?;
        let theme = cx.theme();
        let (muted, accent) = (theme.muted_foreground, theme.accent);
        let rows: Vec<Stateful<Div>> = self
            .palette_matches(cx)
            .into_iter()
            .take(SHOWN)
            .enumerate()
            .map(|(i, (name, action))| {
                // Its key as bound, not as written here.
                let keys = window
                    .highest_precedence_binding_for_action(action.as_ref())
                    .map(|b| {
                        h_flex()
                            .gap_1()
                            .children(b.keystrokes().iter().map(|k| Kbd::new(k.inner().clone())))
                    });
                h_flex()
                    .id(SharedString::from(format!("command-{i}")))
                    .debug_selector(move || format!("command {i}"))
                    .px_2()
                    .py_1()
                    .gap_2()
                    .rounded(theme.radius)
                    .text_sm()
                    .cursor_pointer()
                    .when(palette.picked == i, |row| row.bg(accent))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(palette) = &mut this.palette {
                            palette.picked = i;
                        }
                        this.run_picked(window, cx);
                    }))
                    .child(div().flex_1().child(name))
                    .children(keys)
            })
            .collect();
        let empty = rows.is_empty();
        Some(
            spotlight::card(cx)
                .key_context(PALETTE)
                .on_action(cx.listener(|this, _: &Up, _, cx| this.step_palette(true, cx)))
                .on_action(cx.listener(|this, _: &Down, _, cx| this.step_palette(false, cx)))
                .child(spotlight::title(IconName::Search, "Commands", cx))
                .child(
                    div()
                        .p_1()
                        .child(Input::new(&palette.input).appearance(false)),
                )
                .child(v_flex().p_1().gap_0p5().children(rows).when(empty, |list| {
                    list.child(
                        div()
                            .px_2()
                            .py_1()
                            .text_sm()
                            .text_color(muted)
                            .child("No command by that name"),
                    )
                }))
                .child(spotlight::footer("↑↓ pick · ⏎ run · esc cancel", false, cx)),
        )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` would stand in for the plain one.
    use super::matching;

    #[test]
    fn every_word_typed_narrows_the_commands() {
        let names = |typed: &str| {
            matching(typed)
                .into_iter()
                .map(|(n, _)| n)
                .collect::<Vec<_>>()
        };
        assert_eq!(names("toggle ag"), ["Toggle Agents"]);
        assert_eq!(
            names("AGENT"),
            ["Next Agent", "Previous Agent", "Toggle Agents"]
        );
        assert!(names("").len() > 10, "all of them");
        assert!(names("nothing like it").is_empty());
    }
}

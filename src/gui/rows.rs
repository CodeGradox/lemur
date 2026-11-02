//! Sidebar rows lemur draws itself, so threads, worktrees and projects can
//! be dragged into a new order and projects folded:
//! gpui-component's SidebarMenuItem and SidebarGroup can't. They copy its
//! look.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu};
use gpui_kit::component::sidebar::{SidebarItem, SidebarMenuItem};
use gpui_kit::component::{
    ActiveTheme, Collapsible, Icon, IconName, Sizable, StyledExt, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::marker::PhantomData;
use std::rc::Rc;

pub type OnClick = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
pub type MenuBuilder = Rc<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu>;
pub type Suffix = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;
/// Moves the row at the first index to the second.
pub type Reorder = Rc<dyn Fn(usize, usize, &mut Window, &mut App)>;
pub type Toggle = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
pub struct Thread {
    pub icon: Icon,
    pub label: SharedString,
    pub active: bool,
    pub on_click: OnClick,
    pub menu: MenuBuilder,
    pub suffix: Suffix,
}

/// A worktree's threads, in order. Dropping one on another moves it to
/// that one's place.
#[derive(Clone)]
pub struct Threads {
    /// Tells lists apart: a thread only drops into its own.
    pub key: SharedString,
    pub rows: Vec<Thread>,
    /// Threads whose session is gone, dimmed under the rest.
    pub gone: Vec<Thread>,
    pub reorder: Reorder,
    /// Threads put away, under "Put away", which `toggle_away` opens and
    /// folds: dimmed there, and not dragged.
    pub away: Vec<Thread>,
    pub away_open: bool,
    pub toggle_away: Toggle,
}

/// How many agents and shells run in a worktree, and whether one of them
/// needs you.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Counts {
    pub agents: usize,
    pub shells: usize,
    pub needs_input: bool,
}

#[derive(Clone)]
pub enum Entry {
    /// One of gpui-component's own rows, such as "New worktree".
    Item(Box<SidebarMenuItem>),
    /// A worktree row with its threads under it.
    Worktree {
        label: SharedString,
        suffix: Suffix,
        /// Its right-click menu.
        menu: MenuBuilder,
        /// Its icon's color.
        color: Option<Hsla>,
        /// The project's own checkout, with a folder for its icon; a linked
        /// worktree has a branch.
        main: bool,
        /// What runs in it, shown while it's folded.
        counts: Counts,
        threads: Threads,
        /// Its threads are folded away; clicking it calls `toggle`.
        folded: bool,
        toggle: Toggle,
    },
    /// A host's threads outside every project, under a row naming it.
    Host {
        label: SharedString,
        /// This Mac, with a screen for its icon; another host has a server.
        local: bool,
        /// What starts a thread there: its buttons, and its right-click
        /// menu.
        new: Option<(Suffix, MenuBuilder)>,
        threads: Threads,
    },
    /// A project's hidden worktrees, under a Hidden row of their own, shown
    /// while it's `open`; clicking it calls `toggle`.
    Hidden {
        entries: Vec<Entry>,
        open: bool,
        toggle: Toggle,
    },
}

/// A sidebar menu of entries, standing in for gpui-component's SidebarMenu.
#[derive(Clone, Default)]
pub struct Menu {
    entries: Vec<Entry>,
    collapsed: bool,
    /// Lets the worktrees be dragged into a new order: the list's key, and
    /// what moves one.
    worktree_order: Option<(SharedString, Reorder)>,
}

impl Menu {
    pub fn child(mut self, entry: Entry) -> Self {
        self.entries.push(entry);
        self
    }

    pub fn reorder_worktrees(mut self, key: impl Into<SharedString>, reorder: Reorder) -> Self {
        self.worktree_order = Some((key.into(), reorder));
        self
    }
}

impl Collapsible for Menu {
    fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }
}

impl SidebarItem for Menu {
    fn render(
        self,
        id: impl Into<ElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let id: SharedString = id.into().to_string().into();
        let order = self.worktree_order;
        let mut worktrees = 0;
        v_flex()
            .gap_2()
            .children(self.entries.into_iter().enumerate().map(|(ix, entry)| {
                let id = SharedString::from(format!("{id}-{ix}"));
                match entry {
                    Entry::Item(item) => (*item).render(id, window, cx).into_any_element(),
                    Entry::Worktree {
                        label,
                        suffix,
                        menu,
                        color,
                        main,
                        counts,
                        threads,
                        folded,
                        toggle,
                    } => {
                        let look = WorktreeLook {
                            color,
                            main,
                            counts,
                            folded,
                            toggle,
                            drag: order
                                .clone()
                                .map(|(key, reorder)| (key, worktrees, reorder)),
                        };
                        worktrees += 1;
                        worktree(id, label, suffix, menu, threads, look, window, cx)
                    }
                    Entry::Host {
                        label,
                        local,
                        new,
                        threads,
                    } => host(id, label, local, new, threads, window, cx),
                    Entry::Hidden {
                        entries,
                        open,
                        toggle,
                    } => hidden(id, entries, open, toggle, window, cx),
                }
            }))
    }
}

/// A project in the sidebar: its name on a tinted band over its rows, with
/// a menu for it on hover (⋯) and on right-click; clicking the band folds
/// the rows away. Stands in for gpui-component's SidebarGroup, which does
/// neither.
#[derive(Clone)]
pub struct Group {
    label: SharedString,
    /// The host it's on, after the name and quieter.
    host: Option<SharedString>,
    menu: Option<MenuBuilder>,
    folded: bool,
    toggle: Option<Toggle>,
    /// The list it can be dragged within, its place there, and the move.
    drag: Option<(SharedString, usize, Reorder)>,
    rows: Menu,
}

impl Group {
    pub fn new(label: impl Into<SharedString>, rows: Menu) -> Self {
        Group {
            label: label.into(),
            host: None,
            menu: None,
            folded: false,
            toggle: None,
            drag: None,
            rows,
        }
    }

    pub fn host(mut self, host: impl Into<SharedString>) -> Self {
        self.host = Some(host.into());
        self
    }

    pub fn menu(mut self, menu: MenuBuilder) -> Self {
        self.menu = Some(menu);
        self
    }

    /// Lets its name be dragged onto another group in the list `key`,
    /// where it's at `index`, to take that one's place.
    pub fn draggable(
        mut self,
        key: impl Into<SharedString>,
        index: usize,
        reorder: Reorder,
    ) -> Self {
        self.drag = Some((key.into(), index, reorder));
        self
    }

    /// Folds the rows away (`folded`); clicking the name calls `toggle`.
    pub fn foldable(mut self, folded: bool, toggle: Toggle) -> Self {
        self.folded = folded;
        self.toggle = Some(toggle);
        self
    }
}

impl Collapsible for Group {
    fn is_collapsed(&self) -> bool {
        self.rows.is_collapsed()
    }

    fn collapsed(mut self, collapsed: bool) -> Self {
        self.rows = self.rows.collapsed(collapsed);
        self
    }
}

impl SidebarItem for Group {
    fn render(
        self,
        id: impl Into<ElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let id: SharedString = id.into().to_string().into();
        let theme = cx.theme();
        let hover_group = SharedString::from(format!("{id}-header"));
        let text = theme.sidebar_foreground;
        let label: SharedString = match &self.host {
            Some(host) => format!("{} · {host}", self.label).into(),
            None => self.label.clone(),
        };
        let dragged_label = label.clone();
        let name = h_flex()
            .flex_1()
            .gap_1p5()
            .items_center()
            .child(div().text_sm().font_semibold().child(self.label))
            .when_some(self.host, |this, host| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(text.opacity(0.8))
                        .child(format!("· {host}")),
                )
            });
        // On the right, as a worktree's caret is; a click on it reaches the
        // header, which folds.
        let (foldable, folded) = (self.toggle.is_some(), self.folded);
        let chevron = move || {
            foldable.then(|| {
                Button::new("fold").xsmall().ghost().icon(
                    Icon::new(IconName::ChevronRight)
                        .size_4()
                        .when(!folded, |this| this.rotate(percentage(90. / 360.))),
                )
            })
        };
        let header = h_flex()
            .id("header")
            .debug_selector(move || format!("project {label}"))
            .group(hover_group.clone())
            .flex_shrink_0()
            .px_2()
            .h_8()
            .gap_1()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.sidebar_border)
            // Lighter than a selected thread's row, so the two don't look alike.
            .bg(theme.sidebar_accent.opacity(0.5))
            .text_color(text)
            .child(name)
            .when_some(self.toggle, |this, toggle| {
                this.cursor_pointer()
                    .on_click(move |_, window, cx| toggle(window, cx))
            });
        let header = match &self.drag {
            Some((key, index, _)) => {
                drag_handle::<ProjectRow>(header, key.clone(), *index, dragged_label)
            }
            None => header,
        };
        let header = match self.menu {
            Some(menu) => {
                let dropdown = menu.clone();
                header
                    .child(
                        div()
                            .id("menu")
                            .invisible()
                            .group_hover(hover_group, |style| style.visible())
                            // Opening the menu doesn't fold the project.
                            .on_click(|_, _, cx| cx.stop_propagation())
                            .child(
                                Button::new("menu")
                                    .icon(IconName::Ellipsis)
                                    .ghost()
                                    .xsmall()
                                    .dropdown_menu(move |m, window, cx| dropdown(m, window, cx)),
                            ),
                    )
                    .children(chevron())
                    .context_menu(move |m, window, cx| menu(m, window, cx))
                    .into_any_element()
            }
            None => header.children(chevron()).into_any_element(),
        };
        v_flex()
            .id(id.clone())
            .gap_1p5()
            .child(header)
            .when(!self.folded, |this| {
                this.child(self.rows.render(format!("{id}-rows"), window, cx))
            })
            // Its rows go where it goes, so all of it takes the drop, and the
            // line showing where it lands goes above or below all of it.
            .when_some(self.drag, |this, (key, index, reorder)| {
                drop_target::<ProjectRow>(this, key, index, reorder, cx)
            })
    }
}

/// What a dragged thread, worktree or project carries; it also draws the
/// label under the pointer. `Kind` is which: a project or worktree takes
/// its drop on all of it, the rows under it too, and gpui hands a drop to
/// the innermost row under the pointer that takes its type, which loses it
/// if it's another list's.
#[derive(Clone)]
struct Dragged<Kind> {
    list: SharedString,
    index: usize,
    label: SharedString,
    kind: PhantomData<Kind>,
}

/// The kinds of row that drag.
#[derive(Clone)]
struct ThreadRow;
#[derive(Clone)]
struct WorktreeRow;
#[derive(Clone)]
struct ProjectRow;
#[derive(Clone)]
struct TabRow;

impl<Kind: 'static> Render for Dragged<Kind> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded(cx.theme().radius)
            .text_sm()
            .bg(cx.theme().sidebar_accent)
            .text_color(cx.theme().sidebar_accent_foreground)
            .shadow_md()
            .child(self.label.clone())
    }
}

/// A row as SidebarMenuItem draws one: icon, label, suffix.
fn row(
    id: impl Into<ElementId>,
    icon: Icon,
    label: SharedString,
    active: bool,
    suffix: AnyElement,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let name = label.clone();
    let (hover, active_bg, accent_text) = (
        theme.sidebar_accent.opacity(0.8),
        theme.tokens.sidebar_accent,
        theme.sidebar_accent_foreground,
    );
    h_flex()
        .id(id)
        .w_full()
        .h_7()
        .flex_shrink_0()
        .overflow_x_hidden()
        .p_2()
        .gap_x_2()
        .rounded(theme.radius)
        .text_sm()
        .when(!active, |this| {
            this.hover(|this| this.bg(hover).text_color(accent_text))
        })
        .when(active, |this| {
            this.font_medium().bg(active_bg).text_color(accent_text)
        })
        .child(icon)
        .child(
            h_flex()
                .flex_1()
                .gap_x_2()
                .justify_between()
                .overflow_x_hidden()
                .child(
                    div()
                        .debug_selector(move || format!("label {name}"))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(label),
                )
                .child(suffix),
        )
}

/// How a worktree row shows: its folder's color, whether its threads are
/// folded and what folds them, and, when it can be dragged, the list's
/// key, its place and what moves it.
struct WorktreeLook {
    color: Option<Hsla>,
    main: bool,
    counts: Counts,
    folded: bool,
    toggle: Toggle,
    drag: Option<(SharedString, usize, Reorder)>,
}

/// A worktree row; clicking it or its caret folds the threads away.
#[allow(clippy::too_many_arguments)]
fn worktree(
    id: SharedString,
    label: SharedString,
    suffix: Suffix,
    menu: MenuBuilder,
    threads: Threads,
    look: WorktreeLook,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let has_threads =
        !threads.rows.is_empty() || !threads.gone.is_empty() || !threads.away.is_empty();
    let is_open = has_threads && !look.folded;
    let toggle = look.toggle;
    let fold = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
        cx.stop_propagation();
        toggle(window, cx);
    };
    let icon = match look.main {
        true => Icon::new(IconName::Folder),
        false => super::icons::git_branch(),
    };
    let icon = icon.when_some(look.color, |i, c| i.text_color(c));
    // Its buttons do what they say without folding it. They show while
    // the pointer is on it; the rest of the time a folded worktree says
    // what runs in it there instead.
    let hovered = SharedString::from(format!("{id}-row"));
    let buttons = format!("buttons {label}");
    // Both in one cell, as wide as the wider, so the label ends before either.
    let suffix = div()
        .grid()
        .grid_cols_max_content(1)
        .child(
            h_flex()
                .id("suffix")
                .debug_selector(move || buttons)
                .col_start(1)
                .row_start(1)
                .justify_end()
                .invisible()
                .group_hover(hovered.clone(), |style| style.visible())
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(suffix(window, cx)),
        )
        .when(look.folded && has_threads, |this| {
            this.child(
                counts(look.counts, &label, cx)
                    .col_start(1)
                    .row_start(1)
                    .group_hover(hovered.clone(), |style| style.invisible()),
            )
        });
    let header = row(
        "worktree",
        icon,
        label.clone(),
        false,
        suffix.into_any_element(),
        cx,
    )
    .when(has_threads, |this| {
        this.cursor_pointer().on_click(fold.clone()).child(
            Button::new("caret")
                .xsmall()
                .ghost()
                .icon(
                    Icon::new(IconName::ChevronRight)
                        .size_4()
                        .when(is_open, |this| this.rotate(percentage(90. / 360.))),
                )
                .on_click(fold),
        )
    });
    let header = match &look.drag {
        Some((key, index, _)) => {
            let index = *index;
            drag_handle::<WorktreeRow>(header, key.clone(), index, label)
                .debug_selector(move || format!("worktree-{index}"))
        }
        None => header,
    };
    let header = header
        .group(hovered)
        .context_menu(move |m, window, cx| menu(m, window, cx))
        .into_any_element();
    let border = cx.theme().sidebar_border;
    div()
        .id(id)
        .w_full()
        .child(header)
        .when(is_open, |this| {
            this.child(
                div()
                    .border_l_1()
                    .border_color(border)
                    .ml_3p5()
                    .pl_2p5()
                    .py_0p5()
                    .child(thread_list(threads, window, cx)),
            )
        })
        // Its threads go where it goes, so the whole tree takes the drop,
        // and the line showing where it lands goes above or below all of it.
        .when_some(look.drag, |this, (key, index, reorder)| {
            drop_target::<WorktreeRow>(this, key, index, reorder, cx)
        })
        .into_any_element()
}

/// What runs in a folded worktree: its agents' and shells' counts, each
/// with its icon, and a dot when one needs you.
fn counts(c: Counts, label: &SharedString, cx: &App) -> Div {
    let theme = cx.theme();
    let selector = format!("counts {label}");
    let count = |icon: Icon, n: usize| {
        h_flex()
            .gap_0p5()
            .items_center()
            .child(icon.xsmall())
            .child(n.to_string())
    };
    h_flex()
        .debug_selector(move || selector)
        .gap_1p5()
        .items_center()
        .justify_end()
        .text_xs()
        .text_color(theme.muted_foreground)
        .when(c.agents > 0, |this| {
            this.child(count(super::sidebar::claude_icon(), c.agents))
        })
        .when(c.shells > 0, |this| {
            this.child(count(Icon::new(IconName::SquareTerminal), c.shells))
        })
        .when(c.needs_input, |this| {
            this.child(div().size_1p5().rounded_full().bg(theme.warning))
        })
}

/// A host's row in Other, and its threads under it.
fn host(
    id: SharedString,
    label: SharedString,
    local: bool,
    new: Option<(Suffix, MenuBuilder)>,
    threads: Threads,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let has_threads =
        !threads.rows.is_empty() || !threads.gone.is_empty() || !threads.away.is_empty();
    let icon = match local {
        true => super::icons::monitor(),
        false => super::icons::server(),
    };
    let selector = format!("host {label}");
    // Its buttons show while the pointer is on it, as a worktree's do.
    let hovered = SharedString::from(format!("{id}-row"));
    let buttons = format!("buttons {label}");
    let suffix = match &new {
        Some((suffix, _)) => h_flex()
            .id("suffix")
            .debug_selector(move || buttons)
            .invisible()
            .group_hover(hovered.clone(), |style| style.visible())
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(suffix(window, cx))
            .into_any_element(),
        None => Empty.into_any_element(),
    };
    let header = row("host", icon, label, false, suffix, cx)
        .debug_selector(move || selector)
        .group(hovered);
    let header = match new {
        Some((_, menu)) => header
            .context_menu(move |m, window, cx| menu(m, window, cx))
            .into_any_element(),
        None => header.into_any_element(),
    };
    let border = cx.theme().sidebar_border;
    div()
        .id(id)
        .w_full()
        .child(header)
        .when(has_threads, |this| {
            this.child(
                div()
                    .border_l_1()
                    .border_color(border)
                    .ml_3p5()
                    .pl_2p5()
                    .py_0p5()
                    .child(thread_list(threads, window, cx)),
            )
        })
        .into_any_element()
}

/// The Hidden row at the bottom of a project, with how many it holds, and
/// while `is_open` the hidden worktrees under it; clicking it calls
/// `toggle`.
/// "Put away" and how many, a click on which shows them under it, dimmed,
/// or folds them; nothing without any.
fn away_rows(
    away: Vec<Thread>,
    is_open: bool,
    toggle: Toggle,
    window: &mut Window,
    cx: &mut App,
) -> Vec<AnyElement> {
    if away.is_empty() {
        return vec![];
    }
    let fold = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
        cx.stop_propagation();
        toggle(window, cx);
    };
    let count = div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(away.len().to_string());
    let header = row(
        "away",
        Icon::new(IconName::EyeOff),
        "Put away".into(),
        false,
        count.into_any_element(),
        cx,
    )
    .debug_selector(|| "away".into())
    .cursor_pointer()
    .text_color(cx.theme().muted_foreground)
    .on_click(fold.clone())
    .child(
        Button::new("caret")
            .xsmall()
            .ghost()
            .icon(
                Icon::new(IconName::ChevronRight)
                    .size_4()
                    .when(is_open, |this| this.rotate(percentage(90. / 360.))),
            )
            .on_click(fold),
    );
    let mut rows = vec![header.into_any_element()];
    if is_open {
        rows.extend(away.into_iter().enumerate().map(|(ix, t)| {
            let (on_click, menu) = (t.on_click, t.menu);
            let id = format!("away-{ix}");
            let suffix = (t.suffix)(window, cx);
            row(
                SharedString::from(id.clone()),
                t.icon,
                t.label,
                t.active,
                suffix,
                cx,
            )
            .debug_selector(move || id)
            .opacity(0.6)
            .on_click(move |event, window, cx| on_click(event, window, cx))
            .context_menu(move |m, window, cx| menu(m, window, cx))
            .into_any_element()
        }));
    }
    rows
}

fn hidden(
    id: SharedString,
    entries: Vec<Entry>,
    is_open: bool,
    toggle: Toggle,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let fold = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
        cx.stop_propagation();
        toggle(window, cx);
    };
    let count = div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(entries.len().to_string());
    let header = row(
        "hidden",
        Icon::new(IconName::EyeOff),
        "Hidden".into(),
        false,
        count.into_any_element(),
        cx,
    )
    .debug_selector(|| "hidden".into())
    .cursor_pointer()
    .on_click(fold.clone())
    .child(
        Button::new("caret")
            .xsmall()
            .ghost()
            .icon(
                Icon::new(IconName::ChevronRight)
                    .size_4()
                    .when(is_open, |this| this.rotate(percentage(90. / 360.))),
            )
            .on_click(fold),
    );
    let rows = Menu {
        entries,
        ..Default::default()
    };
    let border = cx.theme().sidebar_border;
    div()
        .id(id.clone())
        .w_full()
        .child(header)
        .when(is_open, |this| {
            this.child(
                div()
                    .border_l_1()
                    .border_color(border)
                    .ml_3p5()
                    .pl_2p5()
                    .py_0p5()
                    .child(rows.render(format!("{id}-rows"), window, cx)),
            )
        })
        .into_any_element()
}

/// Makes a row draggable within the list `key`: dropped on another row of
/// it, it takes that row's place.
fn draggable<Kind: Clone + 'static>(
    row: Stateful<Div>,
    key: SharedString,
    index: usize,
    label: SharedString,
    reorder: Reorder,
    cx: &App,
) -> Stateful<Div> {
    let row = drag_handle::<Kind>(row, key.clone(), index, label);
    drop_target::<Kind>(row, key, index, reorder, cx)
}

/// Lets `handle` drag the row at `index` of the list `key`.
fn drag_handle<Kind: Clone + 'static>(
    handle: Stateful<Div>,
    key: SharedString,
    index: usize,
    label: SharedString,
) -> Stateful<Div> {
    let dragged = Dragged::<Kind> {
        list: key,
        index,
        label,
        kind: PhantomData,
    };
    handle.on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
}

/// Makes `target` the row at `index` of the list `key`: another row of it
/// dropped there takes its place.
fn drop_target<Kind: 'static>(
    target: Stateful<Div>,
    key: SharedString,
    index: usize,
    reorder: Reorder,
    cx: &App,
) -> Stateful<Div> {
    let line = cx.theme().drag_border;
    target
        .can_drop(move |value, _, _| {
            value
                .downcast_ref::<Dragged<Kind>>()
                .is_some_and(|d| d.list == key)
        })
        // A line on the side the row will land.
        .drag_over::<Dragged<Kind>>(move |style, dragged, _, _| {
            if dragged.index > index {
                style.border_t_2().border_color(line)
            } else if dragged.index < index {
                style.border_b_2().border_color(line)
            } else {
                style
            }
        })
        .on_drop(move |dragged: &Dragged<Kind>, window, cx| {
            if dragged.index != index {
                reorder(dragged.index, index, window, cx);
            }
        })
}

/// Makes `tab` draggable along the tab bar `key`, where it's at `index`:
/// dropped on another tab, it takes that one's place. The line on the side
/// it will land is on a slot around the tab, whose own border is its look.
pub fn draggable_tab(
    tab: Stateful<Div>,
    key: SharedString,
    index: usize,
    label: SharedString,
    reorder: Reorder,
    cx: &App,
) -> Stateful<Div> {
    let line = cx.theme().drag_border;
    let tab = drag_handle::<TabRow>(tab, key.clone(), index, label);
    div()
        .id(SharedString::from(format!("tab-slot-{index}")))
        .flex_shrink_0()
        .child(tab)
        .can_drop(move |value, _, _| {
            value
                .downcast_ref::<Dragged<TabRow>>()
                .is_some_and(|d| d.list == key)
        })
        .drag_over::<Dragged<TabRow>>(move |style, dragged, _, _| {
            if dragged.index > index {
                style.border_l_2().border_color(line)
            } else if dragged.index < index {
                style.border_r_2().border_color(line)
            } else {
                style
            }
        })
        .on_drop(move |dragged: &Dragged<TabRow>, window, cx| {
            if dragged.index != index {
                reorder(dragged.index, index, window, cx);
            }
        })
}

fn thread_list(threads: Threads, window: &mut Window, cx: &mut App) -> AnyElement {
    let Threads {
        key,
        rows,
        gone,
        reorder,
        away,
        away_open,
        toggle_away,
    } = threads;
    let away = away_rows(away, away_open, toggle_away, window, cx);
    let gone: Vec<_> = gone
        .into_iter()
        .enumerate()
        .map(|(ix, t)| {
            let (on_click, menu) = (t.on_click, t.menu);
            let id = format!("gone-{ix}");
            let suffix = (t.suffix)(window, cx);
            row(
                SharedString::from(id.clone()),
                t.icon,
                t.label,
                false,
                suffix,
                cx,
            )
            .debug_selector(move || id)
            .opacity(0.6)
            .on_click(move |event, window, cx| on_click(event, window, cx))
            .context_menu(move |m, window, cx| menu(m, window, cx))
        })
        .collect();
    v_flex()
        .gap_1()
        .children(rows.into_iter().enumerate().map(|(ix, t)| {
            let (on_click, menu) = (t.on_click, t.menu);
            let row = row(
                SharedString::from(format!("thread-{ix}")),
                t.icon,
                t.label.clone(),
                t.active,
                (t.suffix)(window, cx),
                cx,
            )
            .debug_selector(move || format!("thread-{ix}"))
            .on_click(move |event, window, cx| on_click(event, window, cx));
            draggable::<ThreadRow>(row, key.clone(), ix, t.label, reorder.clone(), cx)
                .context_menu(move |m, window, cx| menu(m, window, cx))
        }))
        .children(away)
        .children(gone)
        .into_any_element()
}

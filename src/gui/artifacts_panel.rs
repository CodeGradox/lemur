//! The Artifacts panel, on the right as the notes and the changes are: the
//! links to Claude artifacts that agents used or posted in the worktree on
//! show (see `artifacts`), the latest first. A click opens one; its × takes
//! it off the list for good, and leaves the artifact alone.

use super::icons;
use crate::artifacts::{self, Artifact, Store};
use crate::config::Host;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// How often a worktree's logs are read while the panel shows: an agent's
/// log can be hundreds of megabytes.
const EVERY: Duration = Duration::from_secs(30);

pub(super) struct ArtifactsPanel {
    /// Where the links are kept, if anywhere.
    file: Option<PathBuf>,
    store: Store,
    /// The host and folder shown, and what it's called.
    place: Option<(Host, String, String)>,
    loading: bool,
    /// Why the logs couldn't be read.
    error: Option<String>,
    /// When they were last asked for.
    asked: Option<Instant>,
    _task: Option<Task<()>>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl ArtifactsPanel {
    pub(super) fn new(file: Option<PathBuf>) -> Self {
        let store = file.as_deref().map(Store::load).unwrap_or_default();
        ArtifactsPanel {
            file,
            store,
            place: None,
            loading: false,
            error: None,
            asked: None,
            _task: None,
        }
    }

    /// Shows folder `path` on `host`, called `label`, and reads its logs if
    /// that's new, or due.
    pub(super) fn show(&mut self, host: Host, path: String, label: String, cx: &mut Context<Self>) {
        let same = self.shows(&host.name, &path);
        self.place = Some((host, path, label));
        if !same {
            self.error = None;
        }
        if !same || self.asked.is_none_or(|at| at.elapsed() >= EVERY) {
            self.refresh(cx);
        }
        cx.notify();
    }

    /// Nothing on show to tell the links of.
    pub(super) fn clear(&mut self, cx: &mut Context<Self>) {
        if self.place.take().is_some() {
            self.error = None;
            cx.notify();
        }
    }

    fn shows(&self, host: &str, path: &str) -> bool {
        self.place
            .as_ref()
            .is_some_and(|(h, p, _)| h.name == host && p == path)
    }

    /// Reads the logs of the folder on show, those written since the last
    /// time. What they say goes to the folder asked of, whichever shows by
    /// then.
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some((host, path, _)) = self.place.clone() else {
            return;
        };
        if self.loading {
            return;
        }
        self.loading = true;
        self.asked = Some(Instant::now());
        let started = now();
        let minutes = self
            .store
            .place(&host.name, &path)
            .and_then(|place| place.minutes(started));
        self._task = Some(cx.spawn(async move |this, cx| {
            let (host, path, found) = cx
                .background_spawn(async move {
                    let found = artifacts::scan(&host, &path, minutes);
                    (host, path, found)
                })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                let shown = this.shows(&host.name, &path);
                match found {
                    Ok(found) => {
                        if shown {
                            this.error = None;
                        }
                        let place = this.store.place_mut(&host.name, &path);
                        place.add(found);
                        place.looked = Some(started);
                        this.save();
                    }
                    Err(e) if shown => this.error = Some(format!("{e:#}")),
                    Err(_) => {}
                }
                // Another folder shows now: its links, not these.
                if !shown {
                    this.refresh(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn save(&mut self) {
        if let Some(file) = &self.file
            && let Err(e) = self.store.save(file)
        {
            self.error = Some(format!("keeping the links: {e:#}"));
        }
    }

    /// The links of the folder on show.
    fn links(&self) -> &[Artifact] {
        self.place
            .as_ref()
            .and_then(|(host, path, _)| self.store.place(&host.name, path))
            .map_or(&[], |place| &place.links)
    }

    /// Takes `url` off the list of the folder on show.
    fn remove(&mut self, url: &str, cx: &mut Context<Self>) {
        let Some((host, path, _)) = &self.place else {
            return;
        };
        self.store.place_mut(&host.name, path).remove(url);
        self.save();
        cx.notify();
    }

    #[cfg(test)]
    pub(super) fn listed(&self) -> (bool, Vec<String>) {
        let names = self.links().iter().map(|a| a.name().to_string()).collect();
        (self.loading, names)
    }
}

impl Render for ArtifactsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let title = match &self.place {
            Some((_, _, label)) => format!("Artifacts · {label}"),
            None => "Artifacts".to_string(),
        };
        let links = self.links().to_vec();
        let empty = match (&self.place, &self.error) {
            (None, _) => Some("Open a thread to see its worktree's artifacts.".to_string()),
            (_, Some(e)) => Some(e.clone()),
            _ if links.is_empty() && self.loading => Some("Reading Claude's logs…".into()),
            _ if links.is_empty() => {
                Some("No artifacts here yet. Links agents use or post show up here.".into())
            }
            _ => None,
        };
        let rows = links.iter().enumerate().map(|(i, artifact)| {
            let (open, tip, remove) = (
                artifact.url.clone(),
                SharedString::from(artifact.url.clone()),
                artifact.url.clone(),
            );
            let name = artifact.name().to_string();
            let selector = format!("artifact {i} {name}");
            h_flex()
                .id(SharedString::from(format!("artifact-{i}")))
                .debug_selector(move || selector)
                .px_2()
                .py_0p5()
                .gap_2()
                .items_center()
                .rounded(theme.radius)
                .text_sm()
                .cursor_pointer()
                .hover(|row| row.bg(theme.sidebar_accent.opacity(0.5)))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_click(move |_, _, cx| cx.open_url(&open))
                .child(icons::artifact().small())
                .child(div().flex_1().min_w_0().truncate().child(name))
                .child(
                    div()
                        .debug_selector(move || format!("artifact-remove {i}"))
                        .child(
                            Button::new(SharedString::from(format!("artifact-remove-{i}")))
                                .icon(IconName::Close)
                                .ghost()
                                .xsmall()
                                .tooltip("Take it off this list")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.remove(&remove, cx);
                                })),
                        ),
                )
        });
        let list = v_flex()
            .id("artifacts-list")
            .flex_1()
            .min_h_0()
            .px_2()
            .gap_0p5()
            .overflow_y_scroll()
            .children(rows)
            .children(empty.map(|e| div().p_1().text_sm().text_color(muted).child(e)));
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .h_10()
                    .flex_shrink_0()
                    .px_3()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_semibold()
                            .child(title),
                    )
                    .when(!links.is_empty(), |h| {
                        h.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(links.len().to_string()),
                        )
                    }),
            )
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` would stand in for the plain one.
    use super::ArtifactsPanel;
    use crate::test_support::{HcServer, TempDir, wait_until};
    use gpui_kit::{AppContext, Entity, Modifiers};
    use std::path::PathBuf;
    use std::time::Duration;

    const ONE: &str = "1hJWkh1hjNLMZjGxLk1zCD";
    const TWO: &str = "2VWjSVh3UGph6KPNmbpbzY";

    fn opened(
        file: PathBuf,
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<ArtifactsPanel>, &mut gpui::VisualTestContext) {
        let mut panel = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| ArtifactsPanel::new(Some(file)));
            panel = Some(view.clone());
            gpui_kit::base::Root::new(view, window, cx)
        });
        (panel.unwrap(), cx)
    }

    /// Links in a worktree's logs on its host list by themselves, each to
    /// its own worktree, and open with a click. One taken off stays off,
    /// the next time Lemur starts too, though the log still has it.
    #[gpui::test]
    fn links_list_by_themselves_and_stay_off_once_removed(cx: &mut gpui::TestAppContext) {
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let server = HcServer::new("artifacts-panel");
        let host = server.config().hosts.remove(0);
        let dir = TempDir::new("artifacts-panel");
        let file = dir.join("artifacts.json");
        let logs = server.claude_config().join("projects/-srv-api");
        std::fs::create_dir_all(&logs).unwrap();
        let line = |id: &str, title: &str, at: &str| {
            format!(
                "{{\"message\":{{\"content\":\"[{title}](https://claude.ai/artifact/{id})\"}},\"type\":\"assistant\",\"timestamp\":\"{at}\"}}\n"
            )
        };
        std::fs::write(logs.join("a.jsonl"), line(ONE, "Handover", "2026-11-01")).unwrap();

        let (panel, cx) = opened(file.clone(), cx);
        let listed = |panel: &Entity<ArtifactsPanel>, cx: &mut gpui::VisualTestContext| {
            panel.read_with(cx, |p, _| p.listed())
        };
        let shows =
            |panel: &Entity<ArtifactsPanel>, names: &[&str], cx: &mut gpui::VisualTestContext| {
                wait_until(Duration::from_secs(10), || {
                    cx.run_until_parked();
                    let (loading, got) = listed(panel, cx);
                    !loading && got == names
                })
            };
        panel.update(cx, |p, cx| {
            p.show(host.clone(), "/srv/api".into(), "api".into(), cx)
        });
        assert!(shows(&panel, &["Handover"], cx), "{:?}", listed(&panel, cx));

        // A click on it opens it; on its ×, it doesn't.
        let row = cx.debug_bounds("artifact 0 Handover").expect("its row");
        cx.simulate_click(row.center(), Modifiers::none());
        assert_eq!(
            cx.opened_url().as_deref(),
            Some(format!("https://claude.ai/artifact/{ONE}").as_str())
        );

        // A new link, in the log since the last look, goes on top.
        std::fs::OpenOptions::new()
            .append(true)
            .open(logs.join("a.jsonl"))
            .and_then(|mut log| {
                std::io::Write::write_all(&mut log, line(TWO, "Report", "2026-11-02").as_bytes())
            })
            .unwrap();
        panel.update(cx, |p, cx| p.refresh(cx));
        assert!(
            shows(&panel, &["Report", "Handover"], cx),
            "{:?}",
            listed(&panel, cx)
        );

        let remove = cx.debug_bounds("artifact-remove 0").expect("its ×");
        let opened_before = cx.opened_url();
        cx.simulate_click(remove.center(), Modifiers::none());
        assert_eq!(listed(&panel, cx).1, ["Handover"]);
        assert_eq!(cx.opened_url(), opened_before, "it didn't open");

        // Read from the start again, and by a new Lemur: it stays off.
        panel.update(cx, |p, cx| {
            let (host, path) = (host.name.clone(), "/srv/api");
            p.store.place_mut(&host, path).looked = None;
            p.refresh(cx);
        });
        assert!(shows(&panel, &["Handover"], cx), "{:?}", listed(&panel, cx));
        let again = cx.new(|_| ArtifactsPanel::new(Some(file)));
        again.update(cx, |p, cx| {
            p.show(host.clone(), "/srv/api".into(), "api".into(), cx)
        });
        assert!(shows(&again, &["Handover"], cx), "{:?}", listed(&again, cx));

        // Another worktree has its own.
        panel.update(cx, |p, cx| {
            p.show(host.clone(), "/srv/other".into(), "other".into(), cx)
        });
        assert!(shows(&panel, &[], cx), "{:?}", listed(&panel, cx));
    }
}

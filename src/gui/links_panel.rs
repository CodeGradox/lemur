//! The Links panel, on the right as the notes and the changes are: the
//! worktree on show's pull requests, those of its branch and those agents
//! linked to, with what `gh` says of them; and the Claude artifacts agents
//! used or posted there, the latest first (see `links`). A click opens one;
//! its × takes it off the list for good, and leaves what it links to alone.

use super::icons;
use crate::config::Host;
use crate::gh::{self, Problem, PullRequest, State};
use crate::links::{self, Artifact, Place, Store};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// How often a worktree's logs are read and `gh` asked while the panel
/// shows: an agent's log can be hundreds of megabytes.
const EVERY: Duration = Duration::from_secs(30);

pub(super) struct LinksPanel {
    /// Where the links are kept, if anywhere.
    file: Option<PathBuf>,
    store: Store,
    /// The host and folder shown, and what it's called.
    place: Option<(Host, String, String)>,
    loading: bool,
    /// Why the logs couldn't be read.
    error: Option<String>,
    /// `gh`, and why it can't say anything.
    gh: PathBuf,
    gh_problem: Option<Problem>,
    /// When they were last asked for.
    asked: Option<Instant>,
    _task: Option<Task<()>>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl LinksPanel {
    pub(super) fn new(file: Option<PathBuf>) -> Self {
        let store = file.as_deref().map(Store::load).unwrap_or_default();
        LinksPanel {
            file,
            store,
            place: None,
            loading: false,
            error: None,
            // Tests have a stand-in, and never the real one.
            gh: PathBuf::from(if cfg!(test) { "/nonexistent/gh" } else { "gh" }),
            gh_problem: None,
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
            self.gh_problem = None;
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
            self.gh_problem = None;
            cx.notify();
        }
    }

    /// Whether it shows folder `on`, by its host and path; with None,
    /// nothing.
    pub(super) fn is_on(&self, on: Option<(&str, &str)>) -> bool {
        let place = self.place.as_ref();
        place.map(|(h, p, _)| (h.name.as_str(), p.as_str())) == on
    }

    fn shows(&self, host: &str, path: &str) -> bool {
        self.place
            .as_ref()
            .is_some_and(|(h, p, _)| h.name == host && p == path)
    }

    /// Reads the logs of the folder on show, those written since the last
    /// time, and asks `gh` about its pull requests. What they say goes to the
    /// folder asked of, whichever shows by then.
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
        let known = self.store.place(&host.name, &path);
        let minutes = known.and_then(|place| place.minutes(started));
        let wanted = known.map_or(vec![], Place::wanted);
        let finished = known.map_or(vec![], Place::finished);
        let tool = self.gh.clone();
        self._task = Some(cx.spawn(async move |this, cx| {
            let (host, path, found) = cx
                .background_spawn(async move {
                    let found = links::scan(&host, &path, minutes).map(|scan| {
                        let looked = scan.repo.as_deref().map(|repo| {
                            let mut numbers = wanted;
                            for number in &scan.prs {
                                if !numbers.contains(number) && !finished.contains(number) {
                                    numbers.push(*number);
                                }
                            }
                            gh::look(&tool, repo, scan.branch.as_deref(), &numbers)
                        });
                        (scan, looked)
                    });
                    (host, path, found)
                })
                .await;
            this.update(cx, |this, cx| {
                this.loading = false;
                let shown = this.shows(&host.name, &path);
                match found {
                    Ok((scan, looked)) => {
                        if shown {
                            this.error = None;
                            this.gh_problem = looked.as_ref().and_then(|l| l.problem);
                        }
                        let place = this.store.place_mut(&host.name, &path);
                        place.add(scan.artifacts);
                        // Noted whatever `gh` says, as these logs aren't read again.
                        place.link_prs(&scan.prs);
                        if let Some(looked) = looked {
                            place.add_prs(looked.prs, &looked.settled);
                        }
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

    /// What's known of the folder on show.
    fn shown(&self) -> Option<&Place> {
        let (host, path, _) = self.place.as_ref()?;
        self.store.place(&host.name, path)
    }

    /// The artifacts of the folder on show.
    fn links(&self) -> &[Artifact] {
        self.shown().map_or(&[], |place| &place.links)
    }

    /// The pull requests of the folder on show.
    fn prs(&self) -> &[PullRequest] {
        self.shown().map_or(&[], |place| &place.prs)
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

    #[cfg(test)]
    fn listed_prs(&self) -> (bool, Vec<(u64, State)>) {
        let prs = self.prs().iter().map(|pr| (pr.number, pr.state)).collect();
        (self.loading, prs)
    }

    /// A pull request's row: its state, number and title.
    fn pr_row(&self, i: usize, pr: &PullRequest, cx: &mut Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let color: Hsla = match pr.state {
            State::Open => theme.success,
            State::Draft => theme.muted_foreground,
            State::Merged => rgb(0x8250df).into(),
            State::Closed => theme.danger,
        };
        let (open, tip, remove) = (
            pr.url.clone(),
            SharedString::from(pr.url.clone()),
            pr.url.clone(),
        );
        let muted = theme.muted_foreground;
        let (hover, radius) = (theme.sidebar_accent.opacity(0.5), theme.radius);
        let (number, state) = (pr.number, pr.state);
        h_flex()
            .id(SharedString::from(format!("pr-{i}")))
            .debug_selector(move || format!("pr {number} {}", state.name()))
            .px_2()
            .py_0p5()
            .gap_2()
            .items_center()
            .rounded(radius)
            .text_sm()
            .cursor_pointer()
            .hover(|row| row.bg(hover))
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .on_click(move |_, _, cx| cx.open_url(&open))
            .child(
                div()
                    .flex_shrink_0()
                    .px_1p5()
                    .rounded_sm()
                    .text_xs()
                    .text_color(color)
                    .bg(color.opacity(0.15))
                    .child(pr.state.name()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(muted)
                    .child(format!("#{}", pr.number)),
            )
            .child(div().flex_1().min_w_0().truncate().child(pr.title.clone()))
            .child(
                div()
                    .debug_selector(move || format!("pr-remove {i}"))
                    .child(
                        Button::new(SharedString::from(format!("pr-remove-{i}")))
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
    }
}

/// A section's name, over its rows.
fn section(name: &'static str, muted: Hsla) -> Div {
    div()
        .px_1()
        .pt_2()
        .pb_0p5()
        .text_xs()
        .text_color(muted)
        .child(name)
}

impl Render for LinksPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let title = match &self.place {
            Some((_, _, label)) => format!("Links · {label}"),
            None => "Links".to_string(),
        };
        let links = self.links().to_vec();
        let prs = self.prs().to_vec();
        let count = links.len() + prs.len();
        let empty = match (&self.place, &self.error) {
            (None, _) => Some("Open a thread to see its worktree's links.".to_string()),
            (_, Some(e)) => Some(e.clone()),
            _ if count == 0 && self.loading => Some("Looking for links…".into()),
            _ if count == 0 => Some(
                "No links here yet. This branch's pull requests, and the artifacts and pull requests agents use or post, show up here."
                    .into(),
            ),
            _ => None,
        };
        // Why there may be pull requests missing.
        let gh_note = self.gh_problem.map(|problem| match problem {
            Problem::Missing => "Install GitHub's gh to see pull requests here.",
            Problem::SignedOut => "Run gh auth login to see pull requests here.",
        });
        let pr_rows: Vec<_> = prs
            .iter()
            .enumerate()
            .map(|(i, pr)| self.pr_row(i, pr, cx))
            .collect();
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
            .id("links-list")
            .flex_1()
            .min_h_0()
            .px_2()
            .gap_0p5()
            .overflow_y_scroll()
            .when(!pr_rows.is_empty(), |list| {
                list.child(section("Pull requests", muted))
            })
            .children(pr_rows)
            .children(gh_note.map(|note| {
                div()
                    .debug_selector(|| "gh-note".into())
                    .p_1()
                    .text_xs()
                    .text_color(muted)
                    .child(note)
            }))
            .when(!links.is_empty(), |list| {
                list.child(section("Artifacts", muted))
            })
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
                    .when(count > 0, |h| {
                        h.child(div().text_xs().text_color(muted).child(count.to_string()))
                    }),
            )
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: gpui's `test` would stand in for the plain one.
    use super::LinksPanel;
    use crate::gh::State;
    use crate::test_support::{HcServer, TempDir, fake_gh, git_in, wait_until};
    use gpui_kit::{AppContext, Entity, Modifiers};
    use std::path::PathBuf;
    use std::time::Duration;

    const ONE: &str = "1hJWkh1hjNLMZjGxLk1zCD";
    const TWO: &str = "2VWjSVh3UGph6KPNmbpbzY";

    fn opened(
        file: PathBuf,
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<LinksPanel>, &mut gpui::VisualTestContext) {
        let mut panel = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| LinksPanel::new(Some(file)));
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
        let server = HcServer::new("links-panel");
        let host = server.config().hosts.remove(0);
        let dir = TempDir::new("links-panel");
        let file = dir.join("links.json");
        let logs = server.claude_config().join("projects/-srv-api");
        std::fs::create_dir_all(&logs).unwrap();
        let line = |id: &str, title: &str, at: &str| {
            format!(
                "{{\"message\":{{\"content\":\"[{title}](https://claude.ai/artifact/{id})\"}},\"type\":\"assistant\",\"timestamp\":\"{at}\"}}\n"
            )
        };
        std::fs::write(logs.join("a.jsonl"), line(ONE, "Handover", "2026-11-01")).unwrap();

        let (panel, cx) = opened(file.clone(), cx);
        let listed = |panel: &Entity<LinksPanel>, cx: &mut gpui::VisualTestContext| {
            panel.read_with(cx, |p, _| p.listed())
        };
        let shows =
            |panel: &Entity<LinksPanel>, names: &[&str], cx: &mut gpui::VisualTestContext| {
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
        let again = cx.new(|_| LinksPanel::new(Some(file)));
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

    /// A worktree's pull requests list with what `gh` says of them, and
    /// change as it does: those of its branch, and those agents linked to, of
    /// its repository only. A click opens one, and × takes it off for good.
    /// A merged one isn't asked after again. Without `gh`, or signed out,
    /// the panel says so, and links agents posted meanwhile aren't lost.
    #[gpui::test]
    fn pull_requests_list_with_what_gh_says(cx: &mut gpui::TestAppContext) {
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let server = HcServer::new("links-prs");
        let host = server.config().hosts.remove(0);
        let dir = TempDir::new("links-prs");
        let repo = dir.join("api");
        std::fs::create_dir(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        git_in(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        git_in(
            &repo,
            &["remote", "add", "origin", "git@github.com:owner/api.git"],
        );
        git_in(&repo, &["checkout", "-q", "-b", "fix/thing"]);
        let path = repo.to_string_lossy().into_owned();
        let folder: String = path
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let logs = server.claude_config().join("projects").join(folder);
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(
            logs.join("a.jsonl"),
            "{\"text\":\"https://github.com/owner/api/pull/40 and https://github.com/zed-industries/zed/pull/38639\",\"type\":\"assistant\",\"timestamp\":\"t\"}\n",
        )
        .unwrap();
        let told = |number: u64, state: &str, draft: bool, title: &str| {
            format!(
                "{{\"isDraft\":{draft},\"number\":{number},\"state\":\"{state}\",\"title\":\"{title}\",\"url\":\"https://github.com/owner/api/pull/{number}\"}}"
            )
        };
        // What it says is in files, so it can change.
        let branch_says = dir.join("branch.json");
        std::fs::write(
            &branch_says,
            format!("[{}]", told(41, "OPEN", true, "Fix the thing")),
        )
        .unwrap();
        let forty_says = dir.join("forty.json");
        std::fs::write(&forty_says, told(40, "MERGED", false, "An earlier fix")).unwrap();
        let gh = fake_gh(
            &dir,
            &format!(
                r#"case "$*" in
  "pr list --repo owner/api --head fix/thing "*) cat '{}' ;;
  "pr view 40 --repo owner/api "*) cat '{}' ;;
  *) echo "asked of: $*" >&2; exit 1 ;;
esac"#,
                branch_says.display(),
                forty_says.display()
            ),
        );

        let (panel, cx) = opened(dir.join("links.json"), cx);
        panel.update(cx, |p, _| p.gh = gh.clone());
        let prs = |cx: &mut gpui::VisualTestContext| panel.read_with(cx, |p, _| p.listed_prs());
        let shows = |want: &[(u64, State)], cx: &mut gpui::VisualTestContext| {
            wait_until(Duration::from_secs(10), || {
                cx.run_until_parked();
                let (loading, got) = prs(cx);
                !loading && got == want
            })
        };
        panel.update(cx, |p, cx| {
            p.show(host.clone(), path.clone(), "api".into(), cx)
        });
        assert!(
            shows(&[(41, State::Draft), (40, State::Merged)], cx),
            "the open one first, not another project's: {:?}",
            prs(cx)
        );
        assert!(cx.debug_bounds("pr 41 Draft").is_some());
        assert!(cx.debug_bounds("gh-note").is_none());

        // It's marked ready: so says the panel, and no one asks after 40.
        std::fs::write(
            &branch_says,
            format!("[{}]", told(41, "OPEN", false, "Fix the thing")),
        )
        .unwrap();
        let calls = dir.join("calls");
        std::fs::remove_file(&calls).unwrap();
        panel.update(cx, |p, cx| p.refresh(cx));
        assert!(
            shows(&[(41, State::Open), (40, State::Merged)], cx),
            "{:?}",
            prs(cx)
        );
        assert!(cx.debug_bounds("pr 41 Open").is_some());
        let asked = std::fs::read_to_string(&calls).unwrap();
        assert!(!asked.contains("pr view 40"), "{asked}");

        let row = cx.debug_bounds("pr 41 Open").unwrap();
        cx.simulate_click(row.center(), Modifiers::none());
        assert_eq!(
            cx.opened_url().as_deref(),
            Some("https://github.com/owner/api/pull/41")
        );

        // Taken off, it stays off, though it's still the branch's.
        let remove = cx.debug_bounds("pr-remove 0").expect("its ×");
        cx.simulate_click(remove.center(), Modifiers::none());
        panel.update(cx, |p, cx| p.refresh(cx));
        assert!(shows(&[(40, State::Merged)], cx), "{:?}", prs(cx));

        // Signed out: it says so, and a link posted meanwhile waits for `gh`.
        std::fs::OpenOptions::new()
            .append(true)
            .open(logs.join("a.jsonl"))
            .and_then(|mut log| {
                std::io::Write::write_all(
                    &mut log,
                    b"{\"text\":\"https://github.com/owner/api/pull/42\",\"type\":\"assistant\",\"timestamp\":\"t\"}\n",
                )
            })
            .unwrap();
        let signed_out = TempDir::new("links-prs-out");
        let out = fake_gh(
            &signed_out,
            "echo 'To get started with GitHub CLI, please run:  gh auth login' >&2; exit 4",
        );
        panel.update(cx, |p, cx| {
            p.gh = out;
            p.refresh(cx);
        });
        assert!(
            wait_until(Duration::from_secs(10), || {
                cx.run_until_parked();
                cx.debug_bounds("gh-note").is_some()
            }),
            "it says so"
        );

        // By now that log is too old to be read again: only the note of it
        // is left.
        let long_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(logs.join("a.jsonl"))
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        let forty_two = dir.join("forty-two");
        std::fs::write(&forty_two, told(42, "OPEN", false, "Posted meanwhile")).unwrap();
        let back = fake_gh(&dir, &format!("cat '{}'", forty_two.display()));
        panel.update(cx, |p, cx| {
            p.gh = back;
            p.refresh(cx);
        });
        assert!(
            wait_until(Duration::from_secs(10), || {
                cx.run_until_parked();
                prs(cx).1.iter().any(|(number, _)| *number == 42)
            }),
            "{:?}",
            prs(cx)
        );
        assert!(cx.debug_bounds("gh-note").is_none(), "it can say now");
    }
}

//! Pull requests, from GitHub's `gh` on this Mac: it's signed in already,
//! and hosts needn't have it.

use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long `gh` gets to answer: it runs on the network.
const TIME: Duration = Duration::from_secs(20);
/// What's asked of a pull request.
const FIELDS: &str = "number,title,state,isDraft,url";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Open,
    Draft,
    Merged,
    Closed,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Open => "Open",
            State::Draft => "Draft",
            State::Merged => "Merged",
            State::Closed => "Closed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullRequest {
    pub url: String,
    pub number: u64,
    pub title: String,
    pub state: State,
}

/// A pull request as `gh --json` tells of it.
#[derive(Deserialize)]
struct Told {
    number: u64,
    title: String,
    state: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    url: String,
}

impl From<Told> for PullRequest {
    fn from(told: Told) -> Self {
        let state = match told.state.as_str() {
            "MERGED" => State::Merged,
            "CLOSED" => State::Closed,
            _ if told.is_draft => State::Draft,
            _ => State::Open,
        };
        PullRequest {
            url: told.url,
            number: told.number,
            title: told.title,
            state,
        }
    }
}

/// Why `gh` can't tell of any pull request, for you to put right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    Missing,
    SignedOut,
}

/// Why `gh` didn't answer.
#[derive(Debug, PartialEq)]
enum Failed {
    Problem(Problem),
    /// There's no pull request by that number: it won't be, asked again.
    NoSuchPullRequest,
    /// Anything else, as the network: it may be, asked again.
    Other,
}

/// What `gh` said of a repository's pull requests.
#[derive(Debug, Default, PartialEq)]
pub struct Looked {
    pub prs: Vec<PullRequest>,
    /// The numbers asked of that needn't be asked of again as new ones: told
    /// of, or no pull request at all.
    pub settled: Vec<u64>,
    pub problem: Option<Problem>,
}

/// The pull requests of `branch` in `repo`, if it's given, and those
/// numbered `numbers`.
pub fn look(gh: &Path, repo: &str, branch: Option<&str>, numbers: &[u64]) -> Looked {
    let mut looked = Looked::default();
    if let Some(branch) = branch {
        let args = [
            "pr", "list", "--repo", repo, "--head", branch, "--state", "all", "--limit", "5",
            "--json", FIELDS,
        ];
        match ask(gh, &args, TIME).map(|out| serde_json::from_str::<Vec<Told>>(&out)) {
            Ok(Ok(told)) => looked.prs.extend(told.into_iter().map(PullRequest::from)),
            Ok(Err(_)) | Err(Failed::Other) | Err(Failed::NoSuchPullRequest) => {}
            Err(Failed::Problem(problem)) => {
                looked.problem = Some(problem);
                return looked;
            }
        }
    }
    for &number in numbers {
        if looked.prs.iter().any(|pr| pr.number == number) {
            looked.settled.push(number);
            continue;
        }
        let number_text = number.to_string();
        let args = ["pr", "view", &number_text, "--repo", repo, "--json", FIELDS];
        match ask(gh, &args, TIME).map(|out| serde_json::from_str::<Told>(&out)) {
            Ok(Ok(told)) => {
                looked.prs.push(told.into());
                looked.settled.push(number);
            }
            Err(Failed::NoSuchPullRequest) => looked.settled.push(number),
            Ok(Err(_)) | Err(Failed::Other) => {}
            Err(Failed::Problem(problem)) => {
                looked.problem = Some(problem);
                return looked;
            }
        }
    }
    looked
}

/// What `gh` prints for `args`.
fn ask(gh: &Path, args: &[&str], time: Duration) -> Result<String, Failed> {
    let mut child = Command::new(gh)
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Failed::Problem(Problem::Missing),
            _ => Failed::Other,
        })?;
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = vec![];
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let out = read(child.stdout.take().map(|p| Box::new(p) as _));
    let err = read(child.stderr.take().map(|p| Box::new(p) as _));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < time => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failed::Other);
            }
        }
    };
    if status.success() {
        return out.join().map_err(|_| Failed::Other);
    }
    let said = err.join().unwrap_or_default();
    Err(if said.contains("gh auth login") {
        Failed::Problem(Problem::SignedOut)
    } else if said.contains("Could not resolve to a PullRequest") {
        Failed::NoSuchPullRequest
    } else {
        Failed::Other
    })
}

/// A remote's repository as `gh --repo` takes it: `owner/repo`, with its
/// host in front if that isn't github.com. None for what isn't GitHub. A
/// host named in `~/.ssh/config`, as `github-work`, is taken for github.com.
pub fn repo_of(remote: &str) -> Option<String> {
    let remote = remote.trim().trim_end_matches('/');
    let remote = remote.strip_suffix(".git").unwrap_or(remote);
    let (host, path) = match remote.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?,
        None => remote.split_once(':')?,
    };
    let host = host.rsplit('@').next()?;
    let host = host.split(':').next()?;
    if !host.contains("github") {
        return None;
    }
    let mut parts = path.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return None;
    }
    if host == "github.com" || !host.contains('.') {
        Some(format!("{owner}/{name}"))
    } else {
        Some(format!("{host}/{owner}/{name}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TempDir, fake_gh};

    #[test]
    fn a_remote_names_its_repository() {
        for remote in [
            "git@github.com:CodeGradox/lemur.git",
            "git@github.com:CodeGradox/lemur",
            "https://github.com/CodeGradox/lemur.git\n",
            "https://github.com/CodeGradox/lemur/",
            "git@github-work:CodeGradox/lemur.git",
            "ssh://git@github.com:22/CodeGradox/lemur.git",
        ] {
            assert_eq!(
                repo_of(remote).as_deref(),
                Some("CodeGradox/lemur"),
                "{remote}"
            );
        }
        assert_eq!(
            repo_of("git@github.acme.com:team/api.git").as_deref(),
            Some("github.acme.com/team/api")
        );
        for remote in [
            "",
            "git@gitlab.com:team/api.git",
            "https://bitbucket.org/team/api",
            "/srv/git/api.git",
            "https://github.com/lonely",
        ] {
            assert_eq!(repo_of(remote), None, "{remote}");
        }
    }

    const ONE: &str = r#"{"isDraft":true,"number":7,"state":"OPEN","title":"Draft one","url":"https://github.com/o/r/pull/7"}"#;
    const TWO: &str = r#"{"isDraft":false,"number":8,"state":"OPEN","title":"Open one","url":"https://github.com/o/r/pull/8"}"#;
    const THREE: &str = r#"{"isDraft":false,"number":9,"state":"MERGED","title":"Merged one","url":"https://github.com/o/r/pull/9"}"#;

    /// `gh`'s words for each state; what it's asked, for the branch and for
    /// each other number, and that one it tells of on the branch isn't
    /// asked of again.
    #[test]
    fn what_gh_tells_of_is_read() {
        let dir = TempDir::new("gh-look");
        let gh = fake_gh(
            &dir,
            &format!(
                r#"case "$*" in
  "pr list --repo o/r --head feature --state all --limit 5 --json {FIELDS}") echo '[{ONE}]' ;;
  "pr view 8 --repo o/r --json {FIELDS}") echo '{TWO}' ;;
  "pr view 9 --repo o/r --json {FIELDS}") echo '{THREE}' ;;
  "pr view 404 "*) echo 'GraphQL: Could not resolve to a PullRequest with the number of 404.' >&2; exit 1 ;;
  *) echo 'HTTP 502: the network' >&2; exit 1 ;;
esac"#
            ),
        );
        let looked = look(&gh, "o/r", Some("feature"), &[7, 8, 9, 404, 500]);
        let states: Vec<(u64, State, &str)> = looked
            .prs
            .iter()
            .map(|pr| (pr.number, pr.state, pr.title.as_str()))
            .collect();
        assert_eq!(
            states,
            [
                (7, State::Draft, "Draft one"),
                (8, State::Open, "Open one"),
                (9, State::Merged, "Merged one"),
            ]
        );
        assert_eq!(looked.settled, [7, 8, 9, 404], "not 500: the network");
        assert_eq!(looked.problem, None);

        let calls = std::fs::read_to_string(dir.join("calls")).unwrap();
        assert!(!calls.contains("pr view 7 "), "{calls}");
    }

    #[test]
    fn gh_being_missing_or_signed_out_is_told_apart() {
        let dir = TempDir::new("gh-problem");
        let missing = look(&dir.join("no-such-gh"), "o/r", Some("b"), &[7]);
        assert_eq!(missing.problem, Some(Problem::Missing));
        let gh = fake_gh(
            &dir,
            "echo 'To get started with GitHub CLI, please run:  gh auth login' >&2; exit 4",
        );
        let looked = look(&gh, "o/r", None, &[7, 8]);
        assert_eq!(looked.problem, Some(Problem::SignedOut));
        assert_eq!(
            std::fs::read_to_string(dir.join("calls"))
                .unwrap()
                .lines()
                .count(),
            1,
            "it isn't asked again"
        );
    }

    #[test]
    fn a_gh_that_hangs_is_given_up_on() {
        let dir = TempDir::new("gh-hang");
        let gh = fake_gh(&dir, "exec sleep 30");
        let started = Instant::now();
        assert_eq!(
            ask(&gh, &["pr"], Duration::from_millis(100)),
            Err(Failed::Other)
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}

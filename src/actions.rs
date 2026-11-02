use crate::attach::Active;
use crate::config::{ClaudeThread, Config, Host};
use crate::hc::{self, Stream};
use crate::remote::{Remote, quote, sh_path};
use crate::snapshot::{AWAY, ISSUE, KIND, LABEL, ORDER, SHELL, WORKTREE};
use anyhow::{Context, Result, bail};
use futures::channel::mpsc::TryRecvError;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("names may only use letters, digits, - and _");
    }
    Ok(())
}

const ADJECTIVES: &[&str] = &[
    "amber", "bold", "brave", "bright", "calm", "clever", "cosmic", "crisp", "daring", "eager",
    "gentle", "glad", "golden", "happy", "hidden", "jolly", "keen", "kind", "lively", "lucky",
    "merry", "mighty", "misty", "nimble", "noble", "plucky", "proud", "quiet", "rapid", "rustic",
    "shiny", "silent", "silver", "snowy", "sunny", "swift", "tidy", "witty", "wild", "zesty",
];

const NOUNS: &[&str] = &[
    "badger", "beaver", "bison", "cedar", "comet", "crane", "falcon", "fern", "finch", "fjord",
    "fox", "glacier", "harbor", "heron", "lynx", "maple", "meadow", "moose", "nebula", "orca",
    "otter", "owl", "panda", "pebble", "pine", "puffin", "quasar", "raven", "reef", "river",
    "robin", "salmon", "spruce", "summit", "thistle", "tiger", "walrus", "willow", "wren", "yak",
];

/// A random `adjective-noun` name, like `calm-heron`, that `taken` says
/// is free. After 50 tries it adds a number; after 100 it gives up and
/// returns a taken one.
pub fn random_name(taken: impl Fn(&str) -> bool) -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut name = String::new();
    for attempt in 0..100 {
        // A fresh RandomState hashes nothing into a random u64.
        let n = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish() as usize;
        name = format!(
            "{}-{}",
            ADJECTIVES[n % ADJECTIVES.len()],
            NOUNS[(n >> 32) % NOUNS.len()]
        );
        if attempt >= 50 {
            name = format!("{name}-{}", n % 100);
        }
        if !taken(&name) {
            break;
        }
    }
    name
}

/// A random version 4 UUID, as Claude Code's `--session-id` takes one.
pub fn random_uuid() -> String {
    use std::hash::{BuildHasher, Hasher};
    // A fresh RandomState hashes nothing into a random u64.
    let random = || {
        std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish()
    };
    let high = (random() & !0xf000) | 0x4000;
    let low = (random() & !(0b11 << 62)) | (0b10 << 62);
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        high >> 32,
        (high >> 16) & 0xffff,
        high & 0xffff,
        low >> 48,
        low & 0xffff_ffff_ffff
    )
}

/// Whether `s` is a UUID like `random_uuid`'s, in lowercase hex: all a
/// Claude session ID that lemur types into a shell may be.
pub fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => matches!(c, '0'..='9' | 'a'..='f'),
        })
}

/// Separates a thread's label from the random words that kept its tmux
/// name unique, in threads kept from before herdr-core: `claude--calm-heron`.
const LABEL_END: &str = "--";

/// What lemur shows for a thread kept from before herdr-core: its tmux
/// name without the unique words. `api/claude--calm-heron` shows as
/// `api/claude`.
pub fn label(name: &str) -> &str {
    match name.rsplit_once(LABEL_END) {
        Some((label, words)) if !words.contains('/') => label,
        _ => name,
    }
}

/// Refuses branch names git wouldn't take, before the host is asked.
pub fn validate_branch(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name == "@"
        || name.starts_with(['-', '/', '.'])
        || name.ends_with(['/', '.'])
        || name.ends_with(".lock")
        || ["..", "//", "/.", "@{"].iter().any(|s| name.contains(s))
        || name
            .chars()
            .any(|c| c.is_ascii_control() || " ~^:?*[\\".contains(c));
    if bad {
        bail!("{name} can't be a branch name");
    }
    Ok(())
}

/// A shell function that prints the branch new worktrees start from by
/// default: the remote's default branch (`origin/HEAD`, else `origin/main`
/// or `origin/master`), else the branch checked out.
const DEFAULT_BRANCH: &str = r#"default_branch() {
    git symbolic-ref -q --short refs/remotes/origin/HEAD && return
    for b in origin/main origin/master; do
        if git rev-parse -q --verify "refs/remotes/$b" >/dev/null; then echo "$b"; return; fi
    done
    git symbolic-ref -q --short HEAD || echo HEAD
}"#;

/// The branches a new worktree of `base` can start from: the default one
/// first, then the remotes' branches, then the local ones.
pub fn branches(cfg: &Config, base: usize) -> Result<Vec<String>> {
    let b = &cfg.bases[base];
    let r = Remote::new(cfg.host_or_err(&b.host)?);
    let out = r.run(&format!(
        "set -e\ncd {}\n{DEFAULT_BRANCH}\ndefault_branch\ngit for-each-ref --format='%(refname)' refs/remotes refs/heads\n",
        sh_path(&b.path)
    ))?;
    Ok(parse_branches(&out))
}

/// `branches` from the default branch's line, then one full ref name a line.
fn parse_branches(out: &str) -> Vec<String> {
    let mut lines = out.lines();
    let default = lines.next().unwrap_or_default().trim();
    let (mut remote, mut local) = (Vec::new(), Vec::new());
    for name in lines {
        if let Some(branch) = name.strip_prefix("refs/remotes/") {
            // A remote's HEAD only points at its default branch.
            if !branch.ends_with("/HEAD") {
                remote.push(branch);
            }
        } else if let Some(branch) = name.strip_prefix("refs/heads/") {
            local.push(branch);
        }
    }
    std::iter::once(default)
        .chain(remote.into_iter().chain(local).filter(|b| *b != default))
        .filter(|b| !b.is_empty())
        .map(str::to_string)
        .collect()
}

/// A thread just started: its host and terminal, and for Claude, what
/// resumes it.
#[derive(Debug)]
pub struct Started {
    pub host: String,
    /// Its terminal's address.
    pub session: String,
    pub claude: Option<ClaudeThread>,
}

/// Starts Claude in a new worktree `name` of `base`, in a thread labelled
/// `label`. The worktree checks out a new `branch`, or with none its
/// start, detached. It starts from `from`, or the default branch; a
/// remote's branch is fetched first. Returns the thread, and a note if
/// that fetch failed and the worktree starts where it was last fetched.
pub fn new_worktree_thread(
    cfg: &Config,
    base: usize,
    name: &str,
    label: &str,
    branch: Option<&str>,
    from: Option<&str>,
) -> Result<(Started, Option<String>)> {
    validate_name(name)?;
    validate_name(label)?;
    if let Some(branch) = branch {
        validate_branch(branch)?;
    }
    let b = &cfg.bases[base];
    let host = cfg.host_or_err(&b.host)?;
    let (from, fetched) = fetch_start(cfg, base, from)?;
    let checkout = match branch {
        Some(branch) => format!("--no-track -b {}", quote(branch)),
        None => "--detach".to_string(),
    };
    let (repo, wt) = (sh_path(&b.path), sh_path(&b.worktree_dir));
    let out = Remote::new(host).run(&format!(
        "set -e\ncd {repo}\nmkdir -p {wt}\ngit worktree add -q {checkout} {wt}/{name} {}\ncd {wt}/{name}\npwd -P\n",
        quote(&from),
    ))?;
    let dir = out.lines().last().unwrap_or_default();
    let started = start_claude(host, dir, label)?;
    let note = (!fetched)
        .then(|| format!("couldn't fetch {from}; the worktree starts where it was last fetched"));
    Ok((started, note))
}

/// What a new worktree of `base` starts from: `from`, or the default
/// branch. A remote's branch is fetched first; false if that failed.
fn fetch_start(cfg: &Config, base: usize, from: Option<&str>) -> Result<(String, bool)> {
    let b = &cfg.bases[base];
    let r = Remote::new(cfg.host_or_err(&b.host)?);
    let from = from.map_or_else(|| "\"$(default_branch)\"".to_string(), quote);
    // No prompts: nobody could answer one.
    let out = r.run(&format!(
        r#"set -e
cd {}
{DEFAULT_BRANCH}
from={from}
fetched=yes
remote=${{from%%/*}}
if [ "$remote" != "$from" ] && git remote | grep -qxF "$remote"; then
    GIT_TERMINAL_PROMPT=0 git fetch -q "$remote" "${{from#*/}}" </dev/null 2>/dev/null || fetched=no
fi
printf '%s\n%s\n' "$from" "$fetched"
"#,
        sh_path(&b.path)
    ))?;
    let mut lines = out.lines();
    let from = lines.next().unwrap_or_default().to_string();
    Ok((from, lines.next() == Some("yes")))
}

/// Starts Claude, or with `shell` just a shell, in an existing worktree, in
/// a thread labelled `label`.
pub fn new_thread_in(
    cfg: &Config,
    base: usize,
    worktree: &str,
    label: &str,
    shell: bool,
) -> Result<Started> {
    start_in(cfg, &cfg.bases[base].host, worktree, label, shell)
}

/// Starts a thread labelled `label` in `dir` on `host`, a project's
/// worktree or any folder: Claude, or with `shell` just a shell.
pub fn start_in(cfg: &Config, host: &str, dir: &str, label: &str, shell: bool) -> Result<Started> {
    let host = cfg.host_or_err(host)?;
    if !shell {
        return start_claude(host, dir, label);
    }
    Ok(Started {
        host: host.name.clone(),
        session: start_thread(host, dir, label, None, None)?,
        claude: None,
    })
}

/// Starts Claude in a thread labelled `label`, in `dir` on `host`, with a
/// session ID of lemur's, and returns the thread with what resumes it.
fn start_claude(host: &Host, dir: &str, label: &str) -> Result<Started> {
    let id = random_uuid();
    let command = format!("{} --session-id {id}", host.claude);
    let address = start_thread(host, dir, label, Some(&command), None)?;
    Ok(Started {
        host: host.name.clone(),
        session: address.clone(),
        claude: Some(ClaudeThread {
            host: host.name.clone(),
            session: address,
            path: dir.to_string(),
            claude_session: id,
            label: Some(label.to_string()),
            issue: None,
        }),
    })
}

/// Starts a thread labelled `label` in `dir` on `host`: the host's shell,
/// and with `claude`, the command that starts Claude typed into it, so the
/// shell has set up PATH first. The terminal's labels file it under `dir`
/// and give its icon, and pin `issue` to it. Returns its address.
fn start_thread(
    host: &Host,
    dir: &str,
    label: &str,
    claude: Option<&str>,
    issue: Option<&str>,
) -> Result<String> {
    validate_name(label)?;
    let (core, on) = hc::for_host(host)?;
    let kind = if claude.is_some() { "claude" } else { "shell" };
    let mut labels = BTreeMap::from([
        (LABEL.to_string(), label.to_string()),
        (KIND.to_string(), kind.to_string()),
        (WORKTREE.to_string(), dir.to_string()),
    ]);
    if let Some(issue) = issue {
        labels.insert(ISSUE.to_string(), issue.to_string());
    }
    let address = core.run(on.as_deref(), dir, &[], &labels)?;
    if let Some(command) = claude {
        core.type_line(&address, command)?;
    }
    Ok(address)
}

/// Resumes a thread's Claude conversation once its Claude is gone: in its
/// terminal when that came back from its host restarting as a fresh shell,
/// else in a new one, as when that's gone too. Returns the terminal's
/// address.
pub fn resume_thread(cfg: &Config, thread: &ClaudeThread) -> Result<String> {
    if !is_uuid(&thread.claude_session) {
        bail!("{} isn't a Claude session ID", thread.claude_session);
    }
    let host = cfg.host_or_err(&thread.host)?;
    let (core, _) = hc::for_host(host)?;
    // Back from its host restarting as a fresh shell, it resumes there.
    let back = match core
        .terminals()
        .iter()
        .find(|t| t.address == thread.session)
    {
        Some(t) if crate::snapshot::came_back(t) => t.pid,
        Some(_) => bail!("{} still runs", thread.shown()),
        None => None,
    };
    // A thread from before herdr-core, Lemur 1.8 and older, may still run
    // in tmux: resumed, two Claudes would work on one conversation.
    let in_tmux = if is_address(&thread.session) {
        String::new()
    } else {
        let tmux = match &host.tmux_socket {
            Some(socket) => format!("tmux -L {}", quote(socket)),
            None => "tmux".into(),
        };
        format!(
            "if command -v tmux >/dev/null 2>&1 && {tmux} has-session -t {} 2>/dev/null; then echo {} >&2; exit 1; fi\n",
            quote(&format!("={}", thread.session)),
            quote(&format!(
                "{} still runs in tmux, as {}: end it there first",
                thread.shown(),
                thread.session
            )),
        )
    };
    Remote::new(host).run(&format!(
        "if [ ! -d {} ]; then echo {} >&2; exit 1; fi\n{in_tmux}",
        quote(&thread.path),
        quote(&format!("{} is gone", thread.path)),
    ))?;
    // Threads from before kept their label in their tmux name.
    let label = Some(thread.shown())
        .filter(|l| validate_name(l).is_ok())
        .unwrap_or_else(|| "claude".into());
    let resume = format!("{} --resume {}", host.claude, thread.claude_session);
    if back.is_some() {
        // Where it ran: Claude would go on in whatever folder the shell is,
        // and the shell notes itself once Claude runs, so that a resume that
        // fails still offers another.
        let there = format!("cd {} && {resume}", quote(&thread.path));
        core.type_line(&thread.session, &there)?;
        wait_for_program(&core, &thread.session);
        return Ok(thread.session.clone());
    }
    start_thread(
        host,
        &thread.path,
        &label,
        Some(&resume),
        thread.issue.as_deref(),
    )
}

/// Waits a moment for what was typed into the shell of terminal `address`
/// to run, so what's asked next finds the thread running.
fn wait_for_program(core: &hc::Core, address: &str) {
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(3) {
        let terminals = core.terminals();
        let t = terminals.iter().find(|t| t.address == address);
        if t.is_none_or(|t| !crate::snapshot::came_back(t)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Notes the shell `pid` a Claude thread runs in, to tell when its host
/// restarted.
pub fn note_shell(cfg: &Config, host: &str, session: &str, pid: u32) -> Result<()> {
    let (core, _) = hc::for_host(cfg.host_or_err(host)?)?;
    core.label(session, &[(SHELL, &pid.to_string())])
}

/// How many processes the shell `pid` on `host` started that still run or
/// wait: jobs sent off with `&`, or stopped with ⌃Z. Closing its terminal
/// ends them.
pub fn shell_jobs(cfg: &Config, host: &str, pid: Option<u32>) -> Result<usize> {
    let pid = pid.context("its shell has no pid")?;
    let out = Remote::new(cfg.host_or_err(host)?).run(&format!(
        "ps -A -o ppid= | awk -v p={pid} '$1 == p {{ n++ }} END {{ print n + 0 }}'\n"
    ))?;
    out.trim().parse().context("counting its shell's jobs")
}

/// Whether `session` is a herdr-core address (`t7`, `agent/t7`), not a tmux
/// session's name from before.
fn is_address(session: &str) -> bool {
    let leaf = session.rsplit('/').next().unwrap_or(session);
    leaf.strip_prefix('t')
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// Records the order threads were dragged into: `lemur.order` 0, 1, 2, …
/// Puts a thread away, or with `away` false brings it back.
pub fn set_away(cfg: &Config, host: &str, session: &str, away: bool) -> Result<()> {
    let (core, _) = hc::for_host(cfg.host_or_err(host)?)?;
    core.label(session, &[(AWAY, if away { "1" } else { "" })])
}

/// Pins the Linear issue `issue` (its key) to a thread, or with `None`
/// unpins the one pinned.
pub fn set_issue(cfg: &Config, host: &str, session: &str, issue: Option<&str>) -> Result<()> {
    let (core, _) = hc::for_host(cfg.host_or_err(host)?)?;
    core.label(session, &[(ISSUE, issue.unwrap_or_default())])
}

pub fn set_order(cfg: &Config, host: &str, sessions: &[String]) -> Result<()> {
    let (core, _) = hc::for_host(cfg.host_or_err(host)?)?;
    for (i, s) in sessions.iter().enumerate() {
        core.label(s, &[(ORDER, &i.to_string())])?;
    }
    Ok(())
}

/// Takes a linked worktree of `base` out of git; its branch stays, and its
/// folder stays as plain files unless `delete`. The threads running in it
/// close first: those started or filed there, and those whose program is
/// there now. Nothing closes or changes until all that would keep it is
/// ruled out: a thread that isn't one of `sessions` (the error names them
/// all, see `why_kept`), and as git refuses, a lock, and when deleting,
/// uncommitted changes. `force` unlocks it and throws the changes away.
/// Whatever is left of a deleted folder goes too: git can't delete
/// read-only folders.
pub fn remove_worktree(
    cfg: &Config,
    base: usize,
    worktree: &str,
    sessions: &[String],
    force: bool,
    delete: bool,
) -> Result<()> {
    let b = &cfg.bases[base];
    let host = cfg.host_or_err(&b.host)?;
    let r = Remote::new(host);
    let (core, on) = hc::for_host(host)?;
    let inside = |p: &str| {
        p.strip_prefix(worktree)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    };
    let found: Vec<String> = core
        .terminals()
        .into_iter()
        .filter(|t| hc::on_host(t, on.as_deref()))
        .filter(|t| {
            let filed = t.labels.get(WORKTREE).is_some_and(|w| inside(w));
            filed || inside(&t.cwd) || inside(t.folder())
        })
        .map(|t| t.address)
        .collect();
    if found.iter().any(|f| !sessions.contains(f)) {
        bail!("{}: {STILL_RUNNING}\n{}\n", host.name, found.join("\n"));
    }
    let start = format!("set -e\ncd {}\nwt={}\n", sh_path(&b.path), quote(worktree));
    let unlock = if force {
        "git worktree unlock \"$wt\" 2>/dev/null || true\n"
    } else {
        ""
    };
    // Git's refusals, checked before any thread closes, in git's words, as
    // `why_kept` reads them. A lock keeps it either way.
    let locked = r#"lock=$(git worktree list --porcelain | awk -v wt="$wt" '/^worktree / { here = (substr($0, 10) == wt) } here && /^locked/ { print "x" substr($0, 8); exit }')
if [ -n "$lock" ]; then
    echo "cannot remove a locked working tree, lock reason: ${lock#x}" >&2
    exit 1
fi
"#;
    // Changes keep it only from deleting, which would lose them; git
    // checks what `status` lists, untracked files included.
    let changes = if delete && !force {
        r#"if [ -d "$wt" ] && [ -n "$(git -C "$wt" status --porcelain --ignore-submodules=none)" ]; then
    echo "$wt contains modified or untracked files, use --force to delete it" >&2
    exit 1
fi
"#
    } else {
        ""
    };
    r.run(&format!("{start}{unlock}{locked}{changes}"))?;
    for address in &found {
        core.kill(address)?;
    }
    let remove = if delete {
        let remove = if force { "remove --force" } else { "remove" };
        format!(
            r#"if ! out=$(LC_ALL=C git worktree {remove} "$wt" 2>&1); then
    # Git refused, unless it let go and just couldn't delete all of the folder.
    if git worktree list --porcelain | grep -qxF "worktree $wt"; then
        printf '%s\n' "$out" >&2
        exit 1
    fi
fi
if [ -e "$wt" ]; then
    chmod -R u+w "$wt"
    rm -rf "$wt"
fi
"#
        )
    } else {
        // Without its .git file, git prunes it and leaves the rest.
        "rm -f \"$wt/.git\"\ngit worktree prune\n".to_string()
    };
    r.run(&format!("{start}{remove}"))?;
    Ok(())
}

/// Starts the refusal that names the threads running in a worktree.
const STILL_RUNNING: &str = "threads still run in the worktree:";

/// Why a worktree was kept, and what removing it anyway takes: closing its
/// threads, or `force`.
#[derive(Debug, PartialEq)]
pub enum Kept {
    /// Threads run in it that weren't to be closed: all of them, by
    /// address.
    Threads(Vec<String>),
    Changes,
    /// Locked, with git's reason; Claude Code locks the worktree its
    /// session runs in.
    Locked(String),
}

/// What kept a worktree from being removed, if closing its threads or
/// `force` would help.
pub fn why_kept(e: &anyhow::Error) -> Option<Kept> {
    let text = format!("{e:#}");
    if let Some((_, names)) = text.split_once(STILL_RUNNING) {
        let names = names.lines().filter(|l| !l.is_empty());
        Some(Kept::Threads(names.map(str::to_string).collect()))
    } else if text.contains("locked working tree") {
        let reason = text
            .split_once("lock reason: ")
            .and_then(|(_, rest)| rest.lines().next())
            .unwrap_or_default();
        Some(Kept::Locked(reason.trim().to_string()))
    } else if text.contains("use --force") {
        Some(Kept::Changes)
    } else {
        None
    }
}

/// What this Mac is called where it shows a terminal of its own, as one
/// that sets up a host.
pub const HERE: &str = "this Mac";

/// Starts `hc machine add` for `host` in a terminal on this Mac, in the
/// core that reaches the other hosts, where you answer what herdr-core
/// asks to add it: to install herdr-core there or update it, a password, a
/// new host key, as Herdr does. Returns the terminal's address, and this
/// Mac as the host to show it from.
pub fn set_up_host(host: &Host) -> Result<(String, Host)> {
    let session = hc::machines_session(host);
    let here = Host {
        hc_session: session.clone(),
        ..Host::new(HERE, "local")
    };
    let (core, _) = hc::for_host(&here)?;
    let labels = BTreeMap::from([(LABEL.to_string(), format!("set up {}", host.name))]);
    let home = crate::config::home()?.to_string_lossy().into_owned();
    let added = core.known_machine(host);
    let argv = set_up_argv(host, session.as_deref(), added.as_deref());
    let address = core.run(None, &home, &argv, &labels)?;
    Ok((address, here))
}

/// `hc machine add` for `host`, to the core of `session` here, or with
/// `added`, the machine's name once it has one, `hc machine login`: it would
/// refuse to add it again.
fn set_up_argv(host: &Host, session: Option<&str>, added: Option<&str>) -> Vec<String> {
    #[cfg(test)]
    if let Some((_, argv)) = SET_UP.lock().unwrap().iter().find(|(h, _)| *h == host.name) {
        return argv.clone();
    }
    let mut argv = vec![];
    if let Some(session) = session {
        argv.extend(["env".to_string(), format!("HC_SESSION={session}")]);
    }
    argv.push(hc::binary().to_string_lossy().into_owned());
    if let Some(name) = added {
        argv.extend(["machine", "login", name].map(String::from));
        return argv;
    }
    argv.extend(["machine", "add", &host.ssh, "--name"].map(String::from));
    argv.push(hc::machine_name(&host.name));
    if let Some(theirs) = &host.hc_session {
        argv.extend(["--session".to_string(), theirs.clone()]);
    }
    argv
}

/// What tests run in place of `hc machine add`, which would dial a host,
/// by the host's name: tests run at once.
#[cfg(test)]
pub static SET_UP: std::sync::Mutex<Vec<(String, Vec<String>)>> = std::sync::Mutex::new(vec![]);

/// Ends a thread's terminal.
pub fn kill_session(cfg: &Config, host: &str, session: &str) -> Result<()> {
    let (core, _) = hc::for_host(cfg.host_or_err(host)?)?;
    core.kill(session)
}

/// Relabels a thread; returns its terminal's address, which stays.
/// Claude keeps running and attached terminals stay attached.
pub fn rename_session(cfg: &Config, host: &str, session: &str, name: &str) -> Result<String> {
    validate_name(name)?;
    let (core, _) = hc::for_host(cfg.host_or_err(host)?)?;
    core.label(session, &[(LABEL, name)])?;
    Ok(session.to_string())
}

/// Copies files into the host's upload dir; returns their remote paths.
pub fn upload(host: &Host, files: &[PathBuf]) -> Result<Vec<String>> {
    let r = Remote::new(host);
    let dir = sh_path(&host.upload_dir);
    let dir = r
        .run(&format!("set -e\nmkdir -p {dir}\ncd {dir}\npwd\n"))?
        .trim()
        .to_string();
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let name = f
                .file_name()
                .with_context(|| format!("{} is not a file", f.display()))?
                .to_string_lossy();
            let safe: String = name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || "._-".contains(c) {
                        c
                    } else {
                        '-'
                    }
                })
                .collect();
            let path = format!("{dir}/{stamp}-{i}-{safe}");
            r.copy_to(f, &path)?;
            Ok(path)
        })
        .collect()
}

/// The terminal at `session`, if it runs.
fn terminal(host: &Host, session: &str) -> Result<hc::Terminal> {
    let (core, _) = hc::for_host(host)?;
    core.terminals()
        .into_iter()
        .find(|t| t.address == session)
        .with_context(|| format!("{session} isn't running"))
}

/// Copies a file a session printed the path of from the host, into a
/// temporary folder that mirrors the host's, and returns where it went. A
/// relative path is from the session's current folder, as Claude prints
/// them.
pub fn download(host: &Host, session: &str, path: &str) -> Result<PathBuf> {
    let r = Remote::new(host);
    let folder = terminal(host, session)?.folder().to_string();
    let remote = r.run(&format!(
        "set -e\np={}\ncase $p in /*) ;; *) p={}/\"$p\" ;; esac\n[ -f \"$p\" ] || {{ echo \"no file at $p\" >&2; exit 1; }}\ncd \"$(dirname \"$p\")\"\nprintf '%s/%s\\n' \"$(pwd -P)\" \"$(basename \"$p\")\"\n",
        sh_path(path),
        quote(&folder),
    ))?;
    let remote = remote.trim_end_matches('\n');
    // Only the folders' names: `..` can't climb out.
    let mirrored: PathBuf = Path::new(remote)
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    let local = std::env::temp_dir()
        .join("lemur-files")
        .join(&host.name)
        .join(mirrored);
    let dir = local.parent().expect("has parent");
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    r.copy_from(remote, &local)?;
    Ok(local)
}

/// Shows a file with Quick Look, which previews it and never runs it.
pub fn quick_look(file: &Path) -> Result<()> {
    let mut child = Command::new("qlmanage")
        .arg("-p")
        .arg(file)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("starting Quick Look")?;
    // It runs until the preview closes.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// What a session gets typed after an upload: the remote paths, then a
/// space so you can keep writing.
pub fn paste_line(paths: &[String]) -> String {
    format!("{} ", paths.join(" "))
}

/// Pastes `text` into a session, as a bracketed paste when its program
/// asked for one, as a drop in lemur does. It attaches at the terminal's
/// own size, so nothing on its screen moves.
pub fn paste(host: &Host, session: &str, text: &str) -> Result<()> {
    let t = terminal(host, session)?;
    let (core, _) = hc::for_host(host)?;
    // Its screen's modes are all it needs, not its history.
    let mut attached = core.attach(
        session,
        t.cols.max(1),
        t.rows.max(1),
        0,
        serde_json::json!({}),
    )?;
    // The first paint sets the program's modes, bracketed paste among
    // them; it's over once the stream goes quiet.
    let mut painted = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut quiet_since = Instant::now();
    while Instant::now() < deadline {
        match attached.stream.try_recv() {
            Ok(Stream::Data(bytes)) => {
                core.credit(attached.channel, bytes.len());
                painted.extend(bytes);
                quiet_since = Instant::now();
            }
            Ok(_) | Err(TryRecvError::Closed) => bail!("{session} ended"),
            Err(TryRecvError::Empty)
                if !painted.is_empty() && quiet_since.elapsed() > Duration::from_millis(100) =>
            {
                break;
            }
            Err(TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    let painted = String::from_utf8_lossy(&painted);
    let bracketed = painted.rfind("\x1b[?2004h") > painted.rfind("\x1b[?2004l");
    core.input(attached.channel, &crate::gui::keys::paste(text, bracketed));
    core.detach(attached.channel);
    // `lemur paste` and `lemur upload` exit right after.
    core.flush()
}

/// Uploads files and pastes their remote paths into a session's prompt.
pub fn upload_and_type(
    cfg: &Config,
    host: &str,
    session: &str,
    files: &[PathBuf],
) -> Result<Vec<String>> {
    let h = cfg.host_or_err(host)?;
    let paths = upload(h, files)?;
    paste(h, session, &paste_line(&paths))?;
    Ok(paths)
}

/// The session to send uploads to: explicit flags, else the session lemur
/// showed last.
pub fn resolve_target(
    cfg: &Config,
    host: Option<String>,
    session: Option<String>,
) -> Result<(String, String)> {
    if let (Some(h), Some(s)) = (&host, &session) {
        cfg.host_or_err(h)?;
        return Ok((h.clone(), s.clone()));
    }
    let active = Active::load()
        .context("no active session: open one in Lemur, or pass --host and --session")?;
    // The last session lemur showed is only the right one on its own host.
    if session.is_none() && host.as_ref().is_some_and(|h| *h != active.host) {
        bail!(
            "pass --session too: Lemur last showed a session on {}",
            active.host
        );
    }
    let host = host.unwrap_or(active.host);
    cfg.host_or_err(&host)?;
    Ok((host, session.unwrap_or(active.session)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{HcServer, TempDir, git_in, wait_until};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::Duration;

    /// A path a session printed comes over from its folder, or from where an
    /// absolute one says; a folder or a missing file doesn't.
    #[test]
    fn downloading_a_printed_path_fetches_the_file() {
        let server = HcServer::new("dl");
        let dir = TempDir::new("dl");
        let work = dir.join("work");
        std::fs::create_dir_all(work.join("src")).unwrap();
        std::fs::write(work.join("src/a.rs"), "fn a() {}\n").unwrap();
        let t = server.run(&work.to_string_lossy(), &[], &["sleep", "600"]);
        let cfg = server.config();
        let host = cfg.host("local").unwrap();
        assert!(wait_until(Duration::from_secs(5), || server
            .addresses()
            .contains(&t)));

        let got = download(host, &t, "src/a.rs").unwrap();
        assert_eq!(std::fs::read_to_string(&got).unwrap(), "fn a() {}\n");
        assert!(got.ends_with("work/src/a.rs"), "{}", got.display());
        let absolute = work.join("src/a.rs");
        assert_eq!(
            download(host, &t, &absolute.to_string_lossy()).unwrap(),
            got
        );
        let folder = download(host, &t, "src").unwrap_err();
        assert!(format!("{folder:#}").contains("no file at"), "{folder:#}");
        assert!(download(host, &t, "missing.rs").is_err());
        std::fs::remove_file(got).unwrap();
    }

    /// Waits for the core to list `address`.
    fn listed_by(server: &HcServer, address: &str) {
        assert!(
            wait_until(Duration::from_secs(5), || server
                .addresses()
                .iter()
                .any(|a| a == address)),
            "{address} never showed up"
        );
    }

    /// Removing a worktree closes its threads and then deletes it. A thread
    /// it wasn't asked to close, even one whose program only went there, keeps
    /// it and everything in it; so do changes or a lock, until forced.
    #[test]
    fn removing_a_worktree_closes_its_threads() {
        let server = HcServer::new("rm-wt");
        let dir = TempDir::new("rm-wt");
        let repo = dir.join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&["worktree", "add", "-q", "-b", "a", "wt/a"]);
        git(&["worktree", "add", "-q", "-b", "b", "wt/b"]);
        let (a, b) = (repo.join("wt/a"), repo.join("wt/b"));
        let a1 = server.run(&a.to_string_lossy(), &[], &["sleep", "600"]);
        let b1 = server.run(&b.to_string_lossy(), &[], &["sleep", "600"]);
        listed_by(&server, &a1);
        listed_by(&server, &b1);
        let mut cfg = server.config();
        cfg.bases
            .push(crate::config::Base::new("local", &repo.to_string_lossy()));
        let alive = |s: &str| server.addresses().iter().any(|a| a == s);

        let a_path = a.to_string_lossy().into_owned();
        let err = remove_worktree(&cfg, 0, &a_path, &[], false, true).unwrap_err();
        assert_eq!(
            why_kept(&err),
            Some(Kept::Threads(vec![a1.clone()])),
            "{err:#}"
        );
        assert!(a.exists() && alive(&a1));
        remove_worktree(&cfg, 0, &a_path, std::slice::from_ref(&a1), false, true).unwrap();
        assert!(!a.exists());
        assert!(wait_until(Duration::from_secs(5), || !alive(&a1)));

        // Started in the main checkout, its program has moved into b.
        let visit = format!("cd {} && exec sleep 600", quote(&b.to_string_lossy()));
        let visitor = server.run(&repo.to_string_lossy(), &[], &["sh", "-c", &visit]);
        let b_path = b.to_string_lossy().into_owned();
        let moved = wait_until(Duration::from_secs(5), || {
            server
                .core()
                .terminals()
                .iter()
                .any(|t| t.address == visitor && t.folder() == b_path)
        });
        assert!(moved, "the visitor never got to {b_path}");
        let err =
            remove_worktree(&cfg, 0, &b_path, std::slice::from_ref(&b1), false, true).unwrap_err();
        assert_eq!(
            why_kept(&err),
            Some(Kept::Threads(vec![b1.clone(), visitor.clone()])),
            "{err:#}"
        );
        assert!(b.exists() && alive(&b1) && alive(&visitor));

        // What git would refuse is asked about before any thread closes.
        std::fs::write(b.join("draft.txt"), "unsaved").unwrap();
        let both = [b1.clone(), visitor.clone()];
        let err = remove_worktree(&cfg, 0, &b_path, &both, false, true).unwrap_err();
        assert_eq!(why_kept(&err), Some(Kept::Changes), "{err:#}");
        assert!(b.exists() && alive(&b1) && alive(&visitor));
        // Locked, as Claude Code locks its worktree: force unlocks it too.
        git(&[
            "worktree",
            "lock",
            "--reason",
            "claude session draft (pid 1)",
            "wt/b",
        ]);
        let err = remove_worktree(&cfg, 0, &b_path, &both, false, true).unwrap_err();
        assert_eq!(
            why_kept(&err),
            Some(Kept::Locked("claude session draft (pid 1)".into())),
            "{err:#}"
        );
        assert!(b.exists() && alive(&b1) && alive(&visitor));
        remove_worktree(&cfg, 0, &b_path, &both, true, true).unwrap();
        assert!(!b.exists());
        assert!(wait_until(Duration::from_secs(5), || !alive(&b1) && !alive(&visitor)));
    }

    /// The worktrees git lists, by path.
    fn listed(repo: &Path) -> Vec<String> {
        let out = git_out(repo, &["worktree", "list", "--porcelain"]);
        out.lines()
            .filter_map(|l| l.strip_prefix("worktree "))
            .map(str::to_string)
            .collect()
    }

    /// What a git command in `dir` prints; it must succeed.
    fn git_out(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("running git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Kept, a removed worktree's folder stays with its files, changes
    /// included, as plain files; a lock still needs force. Deleted, it goes
    /// even when git can't delete all of it.
    #[test]
    fn removing_a_worktree_keeps_its_folder_unless_deleted() {
        let server = HcServer::new("keep-wt");
        let dir = TempDir::new("keep-wt");
        let repo = dir.join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| git_in(&repo, args);
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        git(&["worktree", "add", "-q", "-b", "a", "wt/a"]);
        git(&["worktree", "add", "-q", "-b", "b", "wt/b"]);
        let (a, b) = (repo.join("wt/a"), repo.join("wt/b"));
        let (a_path, b_path) = (a.to_string_lossy(), b.to_string_lossy());
        let a1 = server.run(&a_path, &[], &["sleep", "600"]);
        let other = server.run("/", &[], &["sleep", "600"]);
        listed_by(&server, &a1);
        listed_by(&server, &other);
        let mut cfg = server.config();
        cfg.bases
            .push(crate::config::Base::new("local", &repo.to_string_lossy()));

        std::fs::write(a.join("draft.txt"), "unsaved").unwrap();
        git(&["worktree", "lock", "wt/a"]);
        let mine = std::slice::from_ref(&a1);
        let err = remove_worktree(&cfg, 0, &a_path, mine, false, false).unwrap_err();
        assert_eq!(why_kept(&err), Some(Kept::Locked(String::new())), "{err:#}");
        assert!(listed(&repo).contains(&a_path.to_string()));
        assert!(server.addresses().contains(&a1));
        remove_worktree(&cfg, 0, &a_path, mine, true, false).unwrap();
        assert!(!listed(&repo).contains(&a_path.to_string()));
        assert_eq!(
            std::fs::read_to_string(a.join("draft.txt")).unwrap(),
            "unsaved"
        );
        assert!(!a.join(".git").exists());
        assert!(wait_until(Duration::from_secs(5), || server.addresses()
            == [other.clone()]));
        assert_eq!(git_out(&repo, &["branch", "--list", "a"]), "a");

        // A read-only folder, as Go's module cache makes, stops git halfway.
        let cache = b.join("cache/mod");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("go.mod"), "module x").unwrap();
        let read_only = std::fs::Permissions::from_mode(0o555);
        std::fs::set_permissions(&cache, read_only).unwrap();
        remove_worktree(&cfg, 0, &b_path, &[], true, true).unwrap();
        assert!(!b.exists());
        assert!(!listed(&repo).contains(&b_path.to_string()));
    }

    /// A repo cloned from `origin`, which then gets a commit the clone
    /// hasn't fetched. Both are on `main`; the config's one base is the clone.
    fn cloned(tag: &str, server: &HcServer) -> (TempDir, Config) {
        let dir = TempDir::new(tag);
        let origin = dir.join("origin");
        std::fs::create_dir(&origin).unwrap();
        git_in(&origin, &["init", "-q", "-b", "main"]);
        git_in(&origin, &["commit", "-q", "--allow-empty", "-m", "one"]);
        git_in(&dir, &["clone", "-q", "origin", "repo"]);
        git_in(&origin, &["commit", "-q", "--allow-empty", "-m", "two"]);
        let mut cfg = server.config();
        // Typed into the new threads; nothing to start.
        cfg.hosts[0].claude = "true".into();
        cfg.bases.push(crate::config::Base::new(
            "local",
            &dir.join("repo").to_string_lossy(),
        ));
        (dir, cfg)
    }

    #[test]
    fn branches_start_with_the_default_one() {
        let server = HcServer::new("branches");
        let (dir, cfg) = cloned("branches", &server);
        git_in(&dir.join("repo"), &["branch", "local-only"]);
        assert_eq!(
            branches(&cfg, 0).unwrap(),
            ["origin/main", "local-only", "main"]
        );
        assert_eq!(
            parse_branches("main\nrefs/heads/main\nrefs/heads/x\n"),
            ["main", "x"]
        );
    }

    /// A new worktree gets the branch it's given, untracked, or is
    /// detached without one; it starts from the default branch as just
    /// fetched, or what it was told. A fetch that fails leaves a note.
    #[test]
    fn a_new_worktree_starts_from_the_fetched_default_branch() {
        let server = HcServer::new("new-wt");
        let (dir, cfg) = cloned("new-wt", &server);
        let repo = dir.join("repo");
        let worktree = |name: &str| repo.join(".claude/worktrees").join(name);
        let subject = |name: &str| git_out(&worktree(name), &["log", "-1", "--format=%s"]);

        let (started, note) =
            new_worktree_thread(&cfg, 0, "feat", "review", Some("feature/x"), None).unwrap();
        assert_eq!(note, None);
        // Claude resumes in the worktree it starts in.
        let claude = started.claude.expect("a Claude thread");
        assert_eq!(claude.session, started.session);
        assert_eq!(claude.label.as_deref(), Some("review"));
        let feat = worktree("feat").canonicalize().unwrap();
        assert_eq!(claude.path, feat.to_string_lossy());
        let t = server.core().terminals();
        let t = t.iter().find(|t| t.address == started.session).unwrap();
        assert_eq!(t.labels.get(LABEL).map(String::as_str), Some("review"));
        assert_eq!(t.labels.get(KIND).map(String::as_str), Some("claude"));
        assert_eq!(t.cwd, feat.to_string_lossy());
        assert_eq!(
            git_out(&worktree("feat"), &["branch", "--show-current"]),
            "feature/x"
        );
        assert_eq!(subject("feat"), "two");
        let upstream = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "feature/x@{upstream}"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(!upstream.status.success(), "feature/x tracks {upstream:?}");

        new_worktree_thread(&cfg, 0, "loose", "claude", None, None).unwrap();
        assert_eq!(
            git_out(&worktree("loose"), &["branch", "--show-current"]),
            ""
        );
        assert_eq!(subject("loose"), "two");

        // The local main is still at what was cloned.
        new_worktree_thread(&cfg, 0, "old", "claude", None, Some("main")).unwrap();
        assert_eq!(subject("old"), "one");

        git_in(&repo, &["remote", "set-url", "origin", "/nonexistent/repo"]);
        let (_, note) = new_worktree_thread(&cfg, 0, "offline", "claude", None, None).unwrap();
        assert!(note.is_some_and(|n| n.contains("origin/main")));
        assert_eq!(subject("offline"), "two");

        assert!(new_worktree_thread(&cfg, 0, "bad", "claude", Some("a b"), None).is_err());
    }

    /// A Claude thread starts with a session ID of lemur's, and comes back
    /// with what resumes it; once its terminal is gone, resuming starts one
    /// again in its folder, with that conversation.
    #[test]
    fn claude_threads_start_with_an_id_and_resume_with_it() {
        let server = HcServer::new("resume");
        let (dir, mut cfg) = cloned("resume", &server);
        // Prints what Claude would get.
        cfg.hosts[0].claude = r#"sh -c 'echo "claude color=$COLORTERM $*"' claude"#.into();
        let typed = |session: &str, text: &str| {
            wait_until(Duration::from_secs(10), || {
                server.hc(&["read", session]).contains(text)
            })
        };
        let repo = dir.join("repo").canonicalize().unwrap();
        let path = repo.to_string_lossy().into_owned();
        let started = new_thread_in(&cfg, 0, &path, "claude", false).unwrap();
        let claude = started.claude.expect("a Claude thread");
        assert!(is_uuid(&claude.claude_session), "{}", claude.claude_session);
        assert_eq!(
            (claude.host.as_str(), claude.path.as_str()),
            ("local", path.as_str())
        );
        let first = format!(
            "claude color=truecolor --session-id {}",
            claude.claude_session
        );
        assert!(typed(&started.session, &first), "{first} never ran");
        let shell = new_thread_in(&cfg, 0, &path, "shell", true).unwrap();
        assert!(shell.claude.is_none());

        // Its terminal is there: resuming would start a second.
        listed_by(&server, &started.session);
        assert!(resume_thread(&cfg, &claude).is_err());
        kill_session(&cfg, "local", &started.session).unwrap();
        assert!(wait_until(Duration::from_secs(5), || !server
            .addresses()
            .contains(&started.session)));
        // Resumed with the Linear issue pinned to it.
        let pinned = ClaudeThread {
            issue: Some("LAFT-3184".into()),
            ..claude.clone()
        };
        let again = resume_thread(&cfg, &pinned).unwrap();
        assert_ne!(again, started.session, "a new terminal");
        let resumed = format!("claude color=truecolor --resume {}", claude.claude_session);
        assert!(typed(&again, &resumed), "{resumed} never ran");
        let t = server.core().terminals();
        let t = t.iter().find(|t| t.address == again).unwrap();
        assert_eq!(t.cwd, path);
        assert_eq!(t.labels.get(KIND).map(String::as_str), Some("claude"));
        assert_eq!(t.labels.get(LABEL).map(String::as_str), Some("claude"));
        assert_eq!(t.labels.get(ISSUE).map(String::as_str), Some("LAFT-3184"));

        // Nothing starts where its folder was, or with an ID that isn't one.
        let before = server.addresses().len();
        let gone = ClaudeThread {
            session: "t999".into(),
            path: dir.join("deleted").to_string_lossy().into_owned(),
            ..claude.clone()
        };
        let err = resume_thread(&cfg, &gone).unwrap_err();
        assert!(format!("{err:#}").contains("is gone"), "{err:#}");
        // Typed into a shell, it would run: here only a mark, should the
        // check ever fail.
        let ran = dir.join("ran");
        let odd = ClaudeThread {
            session: "t998".into(),
            claude_session: format!("x; touch {}", ran.display()),
            ..claude
        };
        assert!(resume_thread(&cfg, &odd).is_err());
        std::thread::sleep(Duration::from_millis(500));
        assert!(!ran.exists(), "it ran");
        assert_eq!(server.addresses().len(), before);
    }

    /// After its host restarts, herdr-core gives a Claude thread's terminal
    /// a fresh shell at the same address: it counts as gone, and resumes
    /// there, not in a second terminal.
    #[test]
    fn a_thread_back_from_a_restart_resumes_in_its_terminal() {
        let server = HcServer::new("resume-back");
        let (dir, mut cfg) = cloned("resume-back", &server);
        cfg.hosts[0].claude = r#"sh -c 'echo "claude $* in $PWD"; exec sleep 600' claude"#.into();
        let path = dir.join("repo").canonicalize().unwrap();
        let path = path.to_string_lossy().into_owned();
        let started = new_thread_in(&cfg, 0, &path, "claude", false).unwrap();
        let thread = started.claude.expect("a Claude thread");
        cfg.threads.push(thread.clone());
        let t = thread.session.clone();
        listed_by(&server, &t);
        let pid = || {
            server
                .core()
                .terminals()
                .into_iter()
                .find(|x| x.address == t)
                .and_then(|x| x.pid)
        };
        let before = pid().expect("its shell");
        note_shell(&cfg, "local", &t, before).unwrap();
        // Filed as a thread, or as one to resume.
        let filed = |cfg: &Config| {
            let snap = crate::snapshot::fetch(cfg);
            let worktrees = snap.hosts[0].bases[0].worktrees.clone();
            let live = worktrees
                .iter()
                .any(|w| w.sessions.iter().any(|s| s.name == t));
            let gone = worktrees
                .iter()
                .any(|w| w.gone.iter().any(|g| g.session == t));
            (live, gone)
        };
        assert!(wait_until(Duration::from_secs(5), || filed(&cfg) == (true, false)));
        assert!(resume_thread(&cfg, &thread).is_err(), "it still runs");

        // The host restarts: herdr-core stops, and starts again.
        server.hc(&["server", "stop"]);
        let back = wait_until(Duration::from_secs(15), || {
            pid().is_some_and(|now| now != before)
        });
        assert!(back, "a fresh shell at {t}");
        assert!(wait_until(Duration::from_secs(5), || filed(&cfg) == (false, true)));
        // However far its shell went, it resumes where it ran.
        server.hc(&["send", &t, "cd /"]);
        let count = server.addresses().len();
        assert_eq!(resume_thread(&cfg, &thread).unwrap(), t, "in its terminal");
        let resumed = format!("claude --resume {} in {path}", thread.claude_session);
        assert!(
            wait_until(Duration::from_secs(10), || server
                .hc(&["read", &t])
                .contains(&resumed)),
            "{}",
            server.hc(&["read", &t])
        );
        assert_eq!(server.addresses().len(), count, "no second terminal");
        assert!(wait_until(Duration::from_secs(5), || filed(&cfg) == (true, false)));
    }

    /// Setting up a host runs `hc machine add` for it, under the name
    /// herdr-core takes, with the session on it the config names, to this
    /// Mac's core for the hosts.
    #[test]
    fn setting_up_a_host_adds_it_as_herdr_does() {
        let mut host = Host::new("Agent", "me@agent.example");
        let argv = set_up_argv(&host, None, None);
        assert_eq!(
            &argv[1..],
            ["machine", "add", "me@agent.example", "--name", "agent"]
        );
        assert!(argv[0].ends_with("herdr-core"), "{argv:?}");
        let argv = set_up_argv(&host, None, Some("agent"));
        assert_eq!(&argv[1..], ["machine", "login", "agent"], "added before");
        host.hc_session = Some("lemur".into());
        let argv = set_up_argv(&host, Some("test"), None);
        assert_eq!(&argv[..2], ["env", "HC_SESSION=test"]);
        assert_eq!(&argv[argv.len() - 2..], ["--session", "lemur"]);
    }

    /// A thread from before herdr-core still running in tmux isn't resumed
    /// a second time; once its tmux session is gone, it is.
    #[test]
    fn a_thread_still_in_tmux_isnt_resumed_twice() {
        struct Tmux(String);
        impl Drop for Tmux {
            fn drop(&mut self) {
                let _ = std::process::Command::new("tmux")
                    .args(["-L", &self.0, "kill-server"])
                    .output();
            }
        }
        if std::process::Command::new("tmux")
            .arg("-V")
            .output()
            .is_err()
        {
            eprintln!("no tmux here: skipped");
            return;
        }
        let server = HcServer::new("resume-tmux");
        let dir = TempDir::new("resume-tmux");
        let mut cfg = server.config();
        let socket = Tmux(format!("lemur-test-{}", std::process::id()));
        cfg.hosts[0].tmux_socket = Some(socket.0.clone());
        cfg.hosts[0].claude = "true".into();
        let name = "api/claude--calm-heron";
        let started = std::process::Command::new("tmux")
            .args([
                "-L",
                &socket.0,
                "new-session",
                "-d",
                "-s",
                name,
                "sleep 600",
            ])
            .output()
            .unwrap();
        assert!(started.status.success(), "{started:?}");
        let thread = ClaudeThread {
            host: "local".into(),
            session: name.into(),
            path: dir.to_string_lossy().into_owned(),
            claude_session: random_uuid(),
            label: None,
            issue: None,
        };
        let before = server.addresses().len();
        let err = resume_thread(&cfg, &thread).unwrap_err();
        assert!(format!("{err:#}").contains("still runs in tmux"), "{err:#}");
        assert_eq!(server.addresses().len(), before, "nothing started");

        drop(socket);
        assert!(resume_thread(&cfg, &thread).is_ok());
        assert!(is_address("t7") && is_address("agent/t12"));
        assert!(!is_address(name) && !is_address("agent/t") && !is_address("tx"));
    }

    /// A paste goes to the program bracketed when it asked for that, and
    /// as is otherwise.
    #[test]
    fn pastes_reach_the_program() {
        let server = HcServer::new("paste");
        let cfg = server.config();
        let host = cfg.host("local").unwrap();
        // Shows, in hex, the bytes it reads.
        let reads = |bracketed: bool, count: usize| {
            let ask = if bracketed {
                r"printf '\033[?2004h'; "
            } else {
                ""
            };
            let script = format!("{ask}stty raw -echo; head -c {count} | xxd -p");
            let t = server.run("/", &[], &["sh", "-c", &script]);
            listed_by(&server, &t);
            paste(host, &t, "a.png ").unwrap();
            t
        };
        let shows = |t: &str, hex: &str| {
            wait_until(Duration::from_secs(5), || {
                server.hc(&["read", t]).contains(hex)
            })
        };
        let bracketed = reads(true, 18);
        let plain = reads(false, 6);
        assert!(
            shows(&bracketed, "1b5b3230307e612e706e67201b5b3230317e"),
            "{}",
            server.hc(&["read", &bracketed])
        );
        assert!(
            shows(&plain, "612e706e6720"),
            "{}",
            server.hc(&["read", &plain])
        );
    }

    /// A host herdr-core reaches as another machine: commands, files,
    /// pastes and threads go there through it, and only its own terminals
    /// file under it.
    #[test]
    fn hosts_are_reached_through_the_core() {
        let server = HcServer::new("far");
        let dir = TempDir::new("far");
        let path = dir.to_string_lossy().into_owned();
        let mut cfg = server.far_config("box");
        cfg.hosts[0].upload_dir = format!("{path}/uploads");
        cfg.bases.push(crate::config::Base::new("box", &path));
        let host = cfg.host("box").unwrap().clone();
        assert_eq!(Remote::new(&host).run("echo $((1 + 2))").unwrap(), "3\n");

        let up = dir.join("up.txt");
        std::fs::write(&up, "hello").unwrap();
        let sent = upload(&host, &[up]).unwrap();
        assert!(sent[0].starts_with(&format!("{path}/uploads/")), "{sent:?}");
        assert_eq!(std::fs::read_to_string(&sent[0]).unwrap(), "hello");
        // Bigger than herdr-core takes in one write, or one frame: in parts.
        let big: Vec<u8> = (0..13_000_000u32).map(|i| (i % 251) as u8).collect();
        let up = dir.join("big.bin");
        std::fs::write(&up, &big).unwrap();
        let sent = upload(&host, &[up]).unwrap();
        assert!(std::fs::read(&sent[0]).unwrap() == big, "the same bytes");
        let left = std::fs::read_dir(dir.join("uploads")).unwrap().count();
        assert_eq!(left, 2, "no parts left behind");

        let started = new_thread_in(&cfg, 0, &path, "build", true).unwrap();
        assert!(started.session.starts_with("box/"), "{}", started.session);
        let here = server.run("/", &[], &["sleep", "600"]);
        let mut shown = vec![];
        let filed = wait_until(Duration::from_secs(10), || {
            let snap = crate::snapshot::fetch(&cfg);
            let w = snap.hosts[0].bases[0].worktrees.first();
            shown = w
                .map(|w| {
                    w.sessions
                        .iter()
                        .map(|s| (s.name.clone(), s.label.clone()))
                        .collect()
                })
                .unwrap_or_default();
            !shown.is_empty()
        });
        assert!(filed, "{:?}", crate::snapshot::fetch(&cfg).hosts[0]);
        assert_eq!(shown, [(started.session.clone(), "build".to_string())]);
        assert!(
            !shown.iter().any(|(s, _)| *s == here),
            "this Mac's terminal"
        );

        std::fs::write(dir.join("x.txt"), "there").unwrap();
        let got = download(&host, &started.session, "x.txt").unwrap();
        assert_eq!(std::fs::read_to_string(&got).unwrap(), "there");
        std::fs::remove_file(got).unwrap();

        paste(&host, &started.session, "far-paste").unwrap();
        assert!(wait_until(Duration::from_secs(5), || server
            .hc(&["read", &started.session])
            .contains("far-paste")));
        kill_session(&cfg, "box", &started.session).unwrap();
        assert!(wait_until(Duration::from_secs(5), || !server
            .addresses()
            .contains(&started.session)));
    }

    #[test]
    fn uuids_are_version_4() {
        let (a, b) = (random_uuid(), random_uuid());
        assert!(is_uuid(&a) && is_uuid(&b), "{a} {b}");
        assert_ne!(a, b);
        assert_eq!(&a[14..15], "4");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"), "{a}");
        assert!(!is_uuid("0FDFABEB-CA80-4916-A103-275BBB5E8614"));
        assert!(!is_uuid("0fdfabeb-ca80-4916-a103-275bbb5e861"));
        assert!(is_uuid("0fdfabeb-ca80-4916-a103-275bbb5e8614"));
    }

    #[test]
    fn branch_names_are_checked() {
        for ok in ["feature/x", "fix-1", "a.b", "release/2.0"] {
            assert!(validate_branch(ok).is_ok(), "{ok}");
        }
        for bad in [
            "", "-x", "a b", "a..b", "a/", "/a", "a.lock", "a~1", "x:y", "@", "a@{b",
        ] {
            assert!(validate_branch(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn why_a_worktree_was_kept() {
        let kept = |text: &str| why_kept(&anyhow::anyhow!("{text}"));
        assert_eq!(
            kept(
                "agent: fatal: cannot remove a locked working tree, lock reason: claude session testing-stuff (pid 1460666 start 8911022)\nuse 'remove -f -f' to override or unlock first"
            ),
            Some(Kept::Locked(
                "claude session testing-stuff (pid 1460666 start 8911022)".into()
            ))
        );
        assert_eq!(
            kept(
                "agent: fatal: cannot remove a locked working tree;\nuse 'remove -f -f' to override or unlock first"
            ),
            Some(Kept::Locked(String::new()))
        );
        assert_eq!(
            kept(
                "agent: fatal: '/w' contains modified or untracked files, use --force to delete it"
            ),
            Some(Kept::Changes)
        );
        assert_eq!(kept("agent: fatal: '/w' is not a working tree"), None);
    }

    #[test]
    fn names_are_restricted() {
        assert!(validate_name("auth-ui_2").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("a b").is_err());
        assert!(validate_name("x;rm").is_err());
    }

    #[test]
    fn labels_hide_the_unique_words() {
        assert_eq!(label("api/claude--calm-heron"), "api/claude");
        assert_eq!(label("api/calm-heron"), "api/calm-heron");
        // Only the last part counts: `--` in a base name isn't a label end.
        assert_eq!(label("my--repo/claude"), "my--repo/claude");
    }

    #[test]
    fn random_names_are_valid_and_avoid_taken_ones() {
        for _ in 0..200 {
            let name = random_name(|n| n.starts_with('a'));
            assert!(validate_name(&name).is_ok(), "{name}");
            assert!(!name.starts_with('a'), "{name}");
        }
        let names: std::collections::HashSet<String> =
            (0..50).map(|_| random_name(|_| false)).collect();
        assert!(names.len() > 25, "names barely vary: {names:?}");
    }
}

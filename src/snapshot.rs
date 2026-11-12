//! What the sidebar shows: each host's worktrees of every base, the
//! terminals in them, and what Claude is doing in each. Terminals come
//! from herdr-core as they change; worktrees and Claude's conversations
//! from one script per host.

use crate::config::{Agent, ClaudeThread, Config, Host};
use crate::hc::{self, Terminal};
use crate::machine::{self, Reading, Server};
use crate::remote::{Remote, sh_path};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub hosts: Vec<HostView>,
}

impl Snapshot {
    /// The worktrees of every base on `host`.
    pub fn worktrees<'a>(&'a self, host: &'a str) -> impl Iterator<Item = &'a Worktree> {
        self.hosts
            .iter()
            .filter(move |h| h.name == host)
            .flat_map(|h| h.bases.iter().flat_map(|b| b.worktrees.iter()))
    }

    /// Drops the gone threads `keep` turns down.
    pub fn retain_gone(&mut self, keep: impl Fn(&ClaudeThread) -> bool) {
        for h in &mut self.hosts {
            h.gone.retain(&keep);
            for w in h.bases.iter_mut().flat_map(|b| b.worktrees.iter_mut()) {
                w.gone.retain(&keep);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct HostView {
    pub name: String,
    pub error: Option<String>,
    /// Its error needs you: herdr-core asks for it, to add the host
    /// (`actions::set_up_host`).
    pub needs_you: bool,
    /// Its user's home folder, where threads outside every project start;
    /// empty until its script has said.
    pub home: String,
    /// How busy it is, and the servers listening on it.
    pub machine: Reading,
    pub bases: Vec<BaseView>,
    /// Sessions that don't belong to any base's worktrees.
    pub other: Vec<Session>,
    /// Threads kept to resume whose session is gone, outside every base's
    /// worktrees.
    pub gone: Vec<ClaudeThread>,
}

#[derive(Debug, Clone)]
pub struct BaseView {
    /// Index into `Config::bases`.
    pub index: usize,
    pub error: Option<String>,
    /// The main checkout comes first, as `git worktree list` prints it.
    pub worktrees: Vec<Worktree>,
    /// Not a git repository: its one "worktree" is the folder itself, and
    /// no others can be added.
    pub plain: bool,
}

#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: String,
    pub branch: Option<String>,
    /// The commit checked out.
    pub head: Option<String>,
    /// The repository's own checkout (git lists it first), or a plain
    /// folder: the project itself rather than a linked worktree.
    pub main: bool,
    pub sessions: Vec<Session>,
    /// Threads kept to resume whose session is gone, as after the host
    /// restarted.
    pub gone: Vec<ClaudeThread>,
}

/// A thread: a terminal herdr-core runs, Claude's or a shell.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// Its terminal's address in herdr-core (`agent/t7`).
    pub name: String,
    /// What it's called everywhere it shows: the name you gave it, else what
    /// its program calls it, else what it was started as.
    pub label: String,
    /// The label kept on its terminal, whatever its program calls it: what it
    /// goes back to, is resumed as, and a rename starts from.
    pub own_label: String,
    /// The worktree it was started for, or its start directory.
    pub path: String,
    /// Since its program last said what it's doing, or since it started.
    pub idle_secs: u64,
    /// What Claude in this session is doing.
    pub activity: Activity,
    /// What Claude is waiting on, when it needs input.
    pub waiting_for: Option<String>,
    /// lemur started it for Claude rather than as a plain shell.
    pub started_claude: bool,
    /// Started for Codex, or Codex found running in it.
    pub codex: bool,
    /// Its place in its worktree once threads were dragged into an order.
    pub order: Option<u32>,
    /// When it started, in ms since the epoch: threads never placed follow
    /// in that order.
    pub started: u64,
    /// The Claude conversation running in it, as `claude agents` reports.
    pub claude: Option<ClaudeRun>,
    /// Something other than its shell at its prompt runs in it, as Claude
    /// or a server, or herdr-core hasn't said what does yet.
    pub busy: bool,
    /// What runs in it besides its shell at its prompt, by its program's name,
    /// unless it's a program you work in rather than wait on (`WAITS_ON_YOU`).
    pub command: Option<String>,
    /// Its program runs: herdr-core knows its pid. A kept terminal whose
    /// program was lost has none until its new shell starts.
    pub running: bool,
    /// Its program's pid, on its own host: the shell's, for a thread
    /// lemur started.
    pub pid: Option<u32>,
    /// The shell lemur last saw it run (`SHELL`), if it noted one.
    pub shell: Option<u32>,
    /// What it runs to help it, as a tree in order: Claude Code's subagents
    /// while they work, and terminals its agent started with `hc run`.
    pub helpers: Vec<Helper>,
    /// Put away: listed after the rest, and out of the sidebar's lists and
    /// the tabs.
    pub away: bool,
    /// Silenced: it makes no notifications; its dot says what it does as ever.
    pub silent: bool,
    /// The Linear issue pinned to it, by its key.
    pub issue: Option<String>,
}

/// Something a thread runs to help it: one of Claude Code's subagents, as
/// Claude reports it (a status record), or a terminal of its own that its
/// agent started (`hc run`, which records the starter as its parent).
#[derive(Debug, Clone, PartialEq)]
pub struct Helper {
    /// The record's id, or the terminal's address.
    pub id: String,
    /// The subagent's task, or the terminal's label.
    pub label: String,
    pub activity: Activity,
    /// A terminal of its own, which opens; a subagent runs inside Claude.
    pub terminal: bool,
    /// How many helpers up it is from the thread: 0 is the thread's own.
    pub depth: usize,
}

/// A Claude conversation running in a session: its ID, and the folder it
/// runs in.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeRun {
    pub id: String,
    pub path: String,
}

impl Session {
    /// The agent it was started for, or found running in it.
    pub fn agent(&self) -> Option<Agent> {
        if self.codex {
            Some(Agent::Codex)
        } else if self.started_claude || self.activity != Activity::Unknown {
            Some(Agent::Claude)
        } else {
            None
        }
    }

    /// The command a shell thread runs, by its program's name: none in an
    /// agent's thread.
    pub fn shell_runs(&self) -> Option<&str> {
        self.command.as_deref().filter(|_| self.agent().is_none())
    }

    /// What its dot says it's doing: what its agent says, or for a shell,
    /// working while it runs a command.
    pub fn doing(&self) -> Activity {
        match self.shell_runs() {
            Some(_) => Activity::Working,
            None => self.activity,
        }
    }

    /// Started for Claude, or Claude found running in it.
    pub fn runs_claude(&self) -> bool {
        self.agent() == Some(Agent::Claude)
    }

    /// Came back from its host restarting as a fresh shell at its prompt
    /// (see `came_back`).
    pub fn came_back(&self) -> bool {
        self.shell
            .is_some_and(|noted| self.pid.is_some_and(|pid| pid != noted))
            && !self.busy
    }

    /// The shell of a Claude thread that lemur hasn't noted: one started
    /// for Claude, one Claude was found running in, typed by hand, or one
    /// back from its host restarting once Claude runs in it again. Until
    /// then it's still to resume.
    pub fn shell_to_note(&self) -> Option<u32> {
        // Only Claude's conversations are kept to resume.
        if self.codex {
            return None;
        }
        let pid = self.pid?;
        let claude_runs = self.activity != Activity::Unknown || self.claude.is_some();
        match self.shell {
            None if self.started_claude || claude_runs => Some(pid),
            Some(noted) if noted != pid && claude_runs => Some(pid),
            _ => None,
        }
    }
}

/// Ordered by urgency, so a session with several Claudes shows the most
/// pressing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Activity {
    /// No Claude found in the session.
    Unknown,
    Idle,
    Working,
    NeedsInput,
}

/// What files a terminal under a space and a tab in Herdr, which shows only
/// those that have them. Each machine has ids of its own.
pub const HERDR_SPACE: &str = "herdr.space";
pub const HERDR_TAB: &str = "herdr.tab";
/// The labels lemur puts on the terminals it starts.
pub const LABEL: &str = "lemur.label";
/// The labels a thread starts with until you name it: not names of yours, so
/// what its program calls it shows instead.
pub const DEFAULT_LABELS: [&str; 3] = ["claude", "codex", "shell"];
pub const KIND: &str = "lemur.kind";
pub const WORKTREE: &str = "lemur.worktree";
pub const ORDER: &str = "lemur.order";
/// Set on a thread put away: out of the sidebar's lists and the tabs while
/// it runs, until it needs you or is opened.
pub const AWAY: &str = "lemur.away";
/// Set on a thread silenced: it makes no notifications, whatever it does.
pub const SILENT: &str = "lemur.silent";
/// The Linear issue pinned to a thread, by its key (`LAFT-3184`).
pub const ISSUE: &str = "lemur.issue";
/// The pid of the shell a Claude thread's Claude runs in, as lemur last
/// saw it. herdr-core keeps lemur's terminals: after their host restarts,
/// each comes back at its address with a fresh shell and another pid.
pub const SHELL: &str = "lemur.shell";

/// What Claude is doing. `claude agents` lists every running session with
/// its pid and status; the process tree (`@@ps`) ties each one to the
/// terminal it runs in. Starting `claude agents` takes a moment of CPU, so
/// it runs only when a Claude does: a `claude` process, or a live pid in
/// Claude's session registry. Without a registry (an older Claude Code,
/// or none) it runs anyway; no answer leaves the status Claude reports to
/// herdr-core.
const CLAUDE: &str = r#"echo '@@agents'
claude_runs() {
  dir=${CLAUDE_CONFIG_DIR:-$HOME/.claude}/sessions
  [ -d "$dir" ] || return 0
  pgrep -x claude >/dev/null 2>&1 && return 0
  for f in "$dir"/*.json; do
    pid=${f##*/}; pid=${pid%.json}
    case $pid in ''|*[!0-9]*) continue ;; esac
    kill -0 "$pid" 2>/dev/null && return 0
  done
  return 1
}
if claude_runs; then
  agents=$(claude agents --json 2>/dev/null) || agents=
  printf '%s\n' "$agents"
  case $agents in ''|'[]') ;; *) echo '@@ps'; ps -A -o pid= -o ppid= ;; esac
else
  echo '[]'
fi
"#;

/// What a host's script found: its bases' worktrees, and the Claudes
/// running there. Terminals change more often, and come on their own.
#[derive(Debug, Clone)]
pub struct HostData {
    pub name: String,
    pub error: Option<String>,
    /// The error needs you: herdr-core asks for it (`hc::NeedsYou`).
    pub needs_you: bool,
    pub home: String,
    pub machine: Reading,
    bases: Vec<BaseView>,
    agents: String,
    parents: HashMap<u32, u32>,
}

impl HostData {
    /// Puts project `index`'s worktrees in the order dragged, as the next
    /// refresh would: a recompose builds from this.
    pub fn sort_worktrees(&mut self, index: usize, order: &[String]) {
        for b in self.bases.iter_mut().filter(|b| b.index == index) {
            sort_worktrees(b, order);
        }
    }

    /// Renumbers its projects' views as the config's bases move, dropping
    /// those `renumber` says are gone: they point into the bases by index.
    pub fn renumber_bases(&mut self, renumber: impl Fn(usize) -> Option<usize>) {
        renumber_views(&mut self.bases, renumber);
    }
}

/// Puts a project's worktrees in `order`; those not in it follow as they are.
fn sort_worktrees(b: &mut BaseView, order: &[String]) {
    b.worktrees.sort_by_key(|w| {
        order
            .iter()
            .position(|p| *p == w.path)
            .unwrap_or(usize::MAX)
    });
}

/// Renumbers projects' views as the config's bases move, dropping those
/// `renumber` says are gone.
pub fn renumber_views(views: &mut Vec<BaseView>, renumber: impl Fn(usize) -> Option<usize>) {
    views.retain_mut(|b| match renumber(b.index) {
        Some(index) => {
            b.index = index;
            true
        }
        None => false,
    });
}

pub fn fetch(cfg: &Config) -> Snapshot {
    compose(cfg, &fetch_data(cfg))
}

/// Runs each host's script, all at once: an unreachable host takes the
/// whole connect timeout.
pub fn fetch_data(cfg: &Config) -> Vec<HostData> {
    std::thread::scope(|scope| {
        let fetches: Vec<_> = cfg
            .hosts
            .iter()
            .map(|h| scope.spawn(|| fetch_host(cfg, h)))
            .collect();
        fetches
            .into_iter()
            .map(|f| f.join().expect("fetch thread"))
            .collect()
    })
}

/// The snapshot `data` and the terminals herdr-core has now make.
pub fn compose(cfg: &Config, data: &[HostData]) -> Snapshot {
    let hosts = data
        .iter()
        .filter_map(|d| {
            let host = cfg.host(&d.name)?;
            if let Some(e) = &d.error {
                // A host added before that asks for a login again.
                let needs_login = || {
                    let known = hc::core_of(host).ok();
                    let down = known
                        .as_ref()
                        .and_then(|core| core.machine_down(&core.known_machine(host)?));
                    down.is_some_and(|(_, needs_you)| needs_you)
                };
                let needs_you = d.needs_you || (host.ssh != "local" && needs_login());
                return Some(failed(host, e.clone(), needs_you));
            }
            Some(match hc::for_host(host) {
                Ok((core, machine)) => {
                    if let Some((why, needs_you)) =
                        machine.as_deref().and_then(|name| core.machine_down(name))
                    {
                        return Some(failed(host, why, needs_you));
                    }
                    let terminals: Vec<Terminal> = core
                        .terminals()
                        .into_iter()
                        .filter(|t| hc::on_host(t, machine.as_deref()))
                        .collect();
                    view(cfg, host, d, &terminals)
                }
                Err(e) => failed(host, format!("{e:#}"), hc::needs_you(&e)),
            })
        })
        .collect();
    Snapshot { hosts }
}

fn failed(host: &Host, error: String, needs_you: bool) -> HostView {
    HostView {
        name: host.name.clone(),
        error: Some(error),
        needs_you,
        home: String::new(),
        machine: Reading::default(),
        bases: vec![],
        other: vec![],
        gone: vec![],
    }
}

/// Each host's name on its tailnet, asked once.
static TAILNETS: Mutex<Vec<(String, Option<String>)>> = Mutex::new(Vec::new());

/// How often `claude agents` is asked on a host: it costs a moment of CPU
/// each time, and what Claude is doing comes from herdr-core anyway. What
/// it adds, each conversation's ID for resuming it, rarely changes.
const AGENTS_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// How often a host's listening servers are asked: `lsof` takes a moment of
/// CPU each time, and servers rarely come and go.
const PORTS_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// When each host (and herdr-core session) last answered something asked
/// only every so often, and what it said.
type Asked<T> = Mutex<Vec<(String, (std::time::Instant, T))>>;

/// What `claude agents` said: the agents, and the process tree that places
/// them.
static AGENTS: Asked<(String, HashMap<u32, u32>)> = Mutex::new(Vec::new());
static PORTS: Asked<Vec<Server>> = Mutex::new(Vec::new());

/// What `place` said last, unless it's been `every` since.
fn said<T: Clone>(asked: &Asked<T>, place: &str, every: std::time::Duration) -> Option<T> {
    asked
        .lock()
        .unwrap()
        .iter()
        .find(|(p, (at, _))| p == place && at.elapsed() < every)
        .map(|(_, (_, said))| said.clone())
}

/// Keeps what `place` said just now.
fn remember<T>(asked: &Asked<T>, place: &str, said: T) {
    let mut all = asked.lock().unwrap();
    all.retain(|(p, _)| p != place);
    all.push((place.to_string(), (std::time::Instant::now(), said)));
}

fn fetch_host(cfg: &Config, host: &Host) -> HostData {
    let known = TAILNETS
        .lock()
        .unwrap()
        .iter()
        .find(|(h, _)| *h == host.ssh)
        .map(|(_, name)| name.clone());
    let place = format!(
        "{}|{}",
        host.ssh,
        host.hc_session.as_deref().unwrap_or_default()
    );
    let agents = said(&AGENTS, &place, AGENTS_EVERY);
    let servers = said(&PORTS, &place, PORTS_EVERY);
    let mut script = script(cfg, host, agents.is_none(), servers.is_none());
    if known.is_none() {
        script += machine::TAILNET;
    }
    match Remote::new(host).run(&script) {
        Ok(out) => {
            let mut data = parse(cfg, host, &out);
            // Not asked this time: what it said last.
            match agents {
                Some((agents, parents)) => (data.agents, data.parents) = (agents, parents),
                None => remember(&AGENTS, &place, (data.agents.clone(), data.parents.clone())),
            }
            match servers {
                Some(servers) => data.machine.servers = servers,
                None => remember(&PORTS, &place, data.machine.servers.clone()),
            }
            match known {
                Some(name) => data.machine.tailnet = name,
                // None, as while tailscale is down: asked again next time.
                None if data.machine.tailnet.is_some() => TAILNETS
                    .lock()
                    .unwrap()
                    .push((host.ssh.clone(), data.machine.tailnet.clone())),
                None => {}
            }
            data
        }
        Err(e) => HostData {
            name: host.name.clone(),
            error: Some(format!("{e:#}")),
            needs_you: hc::needs_you(&e),
            home: String::new(),
            machine: Reading::default(),
            bases: vec![],
            agents: String::new(),
            parents: HashMap::new(),
        },
    }
}

/// The host's script; `agents`: with `claude agents` too, `ports`: with
/// the servers listening.
fn script(cfg: &Config, host: &Host, agents: bool, ports: bool) -> String {
    let mut s = String::from("printf '@@home %s\\n' \"$HOME\"\n");
    for (i, b) in cfg
        .bases
        .iter()
        .enumerate()
        .filter(|(_, b)| b.host == host.name)
    {
        s += &format!(
            // Only a folder git says isn't a repository is plain; any other
            // failure (no git, dubious ownership) is shown as git's error.
            "echo '@@base {i}'\n( cd {} 2>/dev/null || {{ echo '@@error no such directory'; exit 0; }}\nif out=$(LC_ALL=C git worktree list --porcelain 2>&1); then printf '%s\\n' \"$out\"\nelse case $out in *'not a git repository'*) echo \"@@plain $(pwd -P)\" ;; *) echo \"@@error $out\" | head -n 1 ;; esac; fi )\n",
            sh_path(&b.path)
        );
    }
    if agents {
        s += CLAUDE;
    }
    s += machine::LOAD;
    if ports {
        s += machine::PORTS;
    }
    s
}

fn parse(cfg: &Config, host: &Host, out: &str) -> HostData {
    let mut bases: Vec<BaseView> = vec![];
    let mut parents: HashMap<u32, u32> = HashMap::new();
    let mut agents = String::new();
    let mut reading = Reading::default();
    let mut home = String::new();
    let mut section = "";
    for line in out.lines() {
        if let Some(h) = line.strip_prefix("@@home ") {
            home = h.to_string();
            section = "";
            continue;
        }
        if let Some(i) = line.strip_prefix("@@base ") {
            if let Ok(index) = i.trim().parse::<usize>()
                && index < cfg.bases.len()
            {
                bases.push(BaseView {
                    index,
                    error: None,
                    worktrees: vec![],
                    plain: false,
                });
                section = "base";
                continue;
            }
            section = "";
            continue;
        }
        match line {
            "@@ps" => section = "ps",
            "@@agents" => section = "agents",
            "@@load" => section = "load",
            "@@ports" => section = "ports",
            "@@tailnet" => section = "tailnet",
            _ => match section {
                "base" => parse_worktree_line(bases.last_mut().expect("base section"), line),
                "ps" => {
                    let mut f = line.split_whitespace().map(str::parse::<u32>);
                    if let (Some(Ok(pid)), Some(Ok(ppid))) = (f.next(), f.next()) {
                        parents.insert(pid, ppid);
                    }
                }
                "agents" => {
                    agents.push_str(line);
                    agents.push('\n');
                }
                "load" | "ports" | "tailnet" => machine::parse_line(&mut reading, section, line),
                _ => {}
            },
        }
    }
    // Worktrees dragged into an order keep it; others follow in git's.
    for b in &mut bases {
        sort_worktrees(b, &cfg.bases[b.index].worktree_order);
    }
    HostData {
        name: host.name.clone(),
        error: None,
        needs_you: false,
        home,
        machine: reading,
        bases,
        agents,
        parents,
    }
}

/// `host` as its script found it, with `terminals` filed under its
/// worktrees.
fn view(cfg: &Config, host: &Host, data: &HostData, terminals: &[Terminal]) -> HostView {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    // An agent's helpers go under it, not in the sidebar's lists; one whose
    // agent is gone is a thread of its own.
    let addresses: HashSet<&str> = terminals.iter().map(|t| t.address.as_str()).collect();
    let mut sessions: Vec<Session> = terminals
        .iter()
        .filter(|t| parent_of(t).is_none_or(|p| !addresses.contains(p.as_str())))
        .map(|t| {
            let mut s = session(t, now);
            s.helpers = helpers(t, terminals, 0);
            s
        })
        .collect();
    let pids: HashMap<u32, String> = terminals
        .iter()
        .filter_map(|t| Some((t.pid?, t.address.clone())))
        .collect();
    // `claude agents` knows each conversation, for resuming it; what Claude
    // reports to herdr-core is newer than its status, so that comes first.
    if let Some(claude) = claude_activity(&data.agents, &pids, &data.parents) {
        for s in &mut sessions {
            let found = claude.get(&s.name);
            // An answer up to `AGENTS_EVERY` old: not of a Claude gone since,
            // its shell back at its prompt.
            if s.activity == Activity::Unknown
                && s.busy
                && let Some(f) = found
            {
                (s.activity, s.waiting_for) = (f.activity, f.waiting_for.clone());
                s.codex = false;
            }
            s.claude = found.and_then(|f| f.run.clone()).map(|(_, run)| run);
        }
    }

    // A Claude thread whose host restarted is back at its address as a
    // fresh shell: gone, as far as resuming it goes, which happens there.
    sessions.retain(|s| {
        !(s.came_back()
            && cfg
                .threads
                .iter()
                .any(|t| t.host == host.name && t.session == s.name))
    });
    let mut bases = data.bases.clone();
    let live: HashSet<String> = sessions.iter().map(|s| s.name.clone()).collect();
    let mut other = vec![];
    for s in sessions {
        match find_worktree(&bases, &s.path) {
            Some((bi, wi)) => bases[bi].worktrees[wi].sessions.push(s),
            None => other.push(s),
        }
    }
    for w in bases.iter_mut().flat_map(|b| b.worktrees.iter_mut()) {
        sort(&mut w.sessions);
    }
    sort(&mut other);
    // Threads kept to resume whose session is gone go where they ran.
    let mut gone = vec![];
    for t in cfg
        .threads
        .iter()
        .filter(|t| t.host == host.name && !live.contains(&t.session))
    {
        match find_worktree(&bases, &t.path) {
            Some((bi, wi)) => bases[bi].worktrees[wi].gone.push(t.clone()),
            None => gone.push(t.clone()),
        }
    }
    HostView {
        name: host.name.clone(),
        error: None,
        needs_you: false,
        home: data.home.clone(),
        machine: data.machine.clone(),
        bases,
        other,
        gone,
    }
}

/// A terminal as a thread: what lemur labelled it with, or what it shows.
fn session(t: &Terminal, now_ms: u64) -> Session {
    let label = |key: &str| t.labels.get(key).filter(|v| !v.is_empty());
    let (activity, waiting_for) = match &t.status {
        // With its shell back at its prompt, what a program said is over:
        // a Claude killed outright never says it stopped.
        Some(_) if !busy(t) => (Activity::Unknown, None),
        Some(status) => {
            let activity = activity(&status.state);
            let asks = activity == Activity::NeedsInput && !status.msg.is_empty();
            (activity, asks.then(|| status.msg.clone()))
        }
        None => (Activity::Unknown, None),
    };
    let since = t.status_ms.unwrap_or(t.created_ms);
    // What runs says what it is, whatever the thread was started for.
    let reported = t
        .status
        .as_ref()
        .filter(|s| !s.app.is_empty() && busy(t))
        .map(|s| s.app.as_str());
    Session {
        name: t.address.clone(),
        label: match label(LABEL) {
            Some(name) if !DEFAULT_LABELS.contains(&name.as_str()) => name.clone(),
            Some(name) => followed_title(t).unwrap_or_else(|| name.clone()),
            None => shown_name(t),
        },
        own_label: label(LABEL).cloned().unwrap_or_else(|| shown_name(t)),
        path: label(WORKTREE).cloned().unwrap_or_else(|| t.cwd.clone()),
        idle_secs: now_ms.saturating_sub(since) / 1000,
        activity,
        waiting_for,
        started_claude: label(KIND).is_some_and(|k| k == "claude"),
        codex: match reported {
            Some(app) => app == "codex",
            None => label(KIND).is_some_and(|k| k == "codex"),
        },
        order: label(ORDER).and_then(|o| o.parse().ok()),
        started: t.created_ms,
        claude: None,
        busy: busy(t),
        command: t
            .foreground
            .as_ref()
            .filter(|f| busy(t) && !WAITS_ON_YOU.contains(&f.name.as_str()))
            .map(|f| f.name.clone()),
        running: t.pid.is_some(),
        pid: t.pid,
        shell: label(SHELL).and_then(|p| p.parse().ok()),
        helpers: vec![],
        away: label(AWAY).is_some(),
        silent: label(SILENT).is_some(),
        issue: label(ISSUE).cloned(),
    }
}

/// What a program's status word (OSC 7501) says it's doing.
fn activity(state: &str) -> Activity {
    match state {
        "working" => Activity::Working,
        "blocked" | "error" => Activity::NeedsInput,
        "idle" | "done" => Activity::Idle,
        _ => Activity::Unknown,
    }
}

/// What `t` runs to help it, among `all`, as a tree in order: its own
/// subagents first, then each terminal it started, with what that one
/// runs under it.
fn helpers(t: &Terminal, all: &[Terminal], depth: usize) -> Vec<Helper> {
    // Deeper than an agent's helpers' helpers' helpers is a loop.
    if depth > 3 {
        return vec![];
    }
    // A Claude killed can't say its subagents ended: with its shell back
    // at its prompt, what they last said is over too.
    let mut out: Vec<Helper> = t
        .records
        .iter()
        .flatten()
        .filter(|_| busy(t))
        .map(|(id, r)| Helper {
            id: id.clone(),
            label: [&r.title, &r.msg]
                .into_iter()
                .find(|s| !s.is_empty())
                .cloned()
                .unwrap_or_else(|| "Subagent".into()),
            activity: activity(&r.state),
            terminal: false,
            depth,
        })
        .collect();
    let mut children: Vec<&Terminal> = all
        .iter()
        .filter(|c| parent_of(c).as_deref() == Some(t.address.as_str()))
        .collect();
    children.sort_by_key(|c| c.created_ms);
    for c in children {
        let label = c.labels.get(LABEL).filter(|v| !v.is_empty());
        out.push(Helper {
            id: c.address.clone(),
            label: label.cloned().unwrap_or_else(|| shown_name(c)),
            activity: c
                .status
                .as_ref()
                .filter(|_| busy(c))
                .map_or(Activity::Unknown, |s| activity(&s.state)),
            terminal: true,
            depth,
        });
        out.extend(helpers(c, all, depth + 1));
    }
    out
}

/// The address of the terminal that started `t`, as this core calls it:
/// on another machine, a parent's bare `t3` is that machine's.
fn parent_of(t: &Terminal) -> Option<String> {
    let parent = t.parent.as_deref().filter(|p| !p.is_empty())?;
    Some(match (parent.contains('/'), t.machine()) {
        (false, Some(machine)) => format!("{machine}/{parent}"),
        _ => parent.to_string(),
    })
}

/// The shells a terminal can idle in at their prompt, as the OS names
/// their processes.
const SHELLS: [&str; 12] = [
    "sh", "bash", "zsh", "fish", "dash", "ksh", "mksh", "tcsh", "csh", "nu", "elvish", "xonsh",
];

/// Programs you work in rather than wait on, as the OS names their
/// processes: open, they aren't a command at work. Claude and Codex say what
/// they do themselves.
const WAITS_ON_YOU: [&str; 24] = [
    "vi", "vim", "nvim", "nano", "emacs", "micro", "hx", "kak", "less", "more", "most", "man",
    "top", "htop", "btop", "watch", "fzf", "ssh", "mosh-client", "tmux", "screen", "zellij",
    "claude", "codex",
];

/// Whether `t` came back from its host restarting as a fresh shell at its
/// prompt, where lemur saw another shell run it.
pub fn came_back(t: &Terminal) -> bool {
    let noted = t.labels.get(SHELL).and_then(|p| p.parse::<u32>().ok());
    noted.is_some_and(|noted| t.pid.is_some_and(|pid| pid != noted)) && !busy(t)
}

/// Whether anything but a shell at its prompt runs in `t`: in the
/// foreground is something other than its own program, or that program
/// isn't a shell. herdr-core reads that about twice a second; until it
/// has, a live terminal counts as busy.
fn busy(t: &Terminal) -> bool {
    match &t.foreground {
        Some(f) => t.pid != Some(f.pid) || !SHELLS.contains(&f.name.trim_start_matches('-')),
        None => t.state != "finished",
    }
}

/// What a terminal lemur didn't start is called: the title its program
/// gives it when that says something, else the program.
fn shown_name(t: &Terminal) -> String {
    if let Some(title) = telling_title(t) {
        return title;
    }
    let program = t
        .foreground
        .as_ref()
        .map(|f| f.name.as_str())
        .or(t.argv.first().map(|a| crate::config::leaf(a)))
        .filter(|p| !p.is_empty());
    program.unwrap_or("terminal").to_string()
}

/// The title its program gives it, if it says something.
fn telling_title(t: &Terminal) -> Option<String> {
    // A shell's title is often where it is (`~/src`) or who (`me@box`).
    let title = t.title.trim();
    let telling = !title.starts_with(['~', '/']) && !title.contains('@');
    // Claude's starts with its spinner.
    let title = title.trim_start_matches(|c: char| !c.is_alphanumeric());
    (telling && !title.is_empty()).then(|| title.to_string())
}

/// What a thread you haven't named shows in place of what it started as: its
/// program's title, while it runs, if that says more than the program's name or
/// the folder it's in. Codex's is the folder until told otherwise.
fn followed_title(t: &Terminal) -> Option<String> {
    let title = telling_title(t).filter(|_| busy(t))?;
    let foreground = t.foreground.as_ref();
    let dull = ["claude code", "claude", "codex"]
        .into_iter()
        .chain(foreground.map(|f| f.name.as_str()))
        .chain(foreground.map(|f| crate::config::leaf(&f.cwd)))
        .chain([crate::config::leaf(&t.cwd)])
        .chain(t.labels.get(WORKTREE).map(|w| crate::config::leaf(w)));
    let said = title.to_lowercase();
    (!dull.into_iter().any(|d| d.to_lowercase() == said)).then_some(title)
}

/// Threads in the order they were dragged into; ones never placed follow,
/// oldest first. Those put away go last, so the rest are the first in a
/// list, as the sidebar and the tabs show and drag them.
fn sort(sessions: &mut [Session]) {
    sessions.sort_by(|a, b| {
        (a.away, a.order.is_none(), a.order, a.started, &a.name).cmp(&(
            b.away,
            b.order.is_none(),
            b.order,
            b.started,
            &b.name,
        ))
    });
}

fn parse_worktree_line(base: &mut BaseView, line: &str) {
    if let Some(e) = line.strip_prefix("@@error ") {
        base.error = Some(e.to_string());
    } else if let Some(p) = line.strip_prefix("@@plain ") {
        base.plain = true;
        base.worktrees.push(Worktree {
            path: p.to_string(),
            branch: None,
            head: None,
            main: true,
            sessions: vec![],
            gone: vec![],
        });
    } else if let Some(p) = line.strip_prefix("worktree ") {
        let main = base.worktrees.is_empty();
        base.worktrees.push(Worktree {
            path: p.to_string(),
            branch: None,
            head: None,
            main,
            sessions: vec![],
            gone: vec![],
        });
    } else if let Some(h) = line.strip_prefix("HEAD ")
        && let Some(w) = base.worktrees.last_mut()
    {
        w.head = Some(h.to_string());
    } else if let Some(b) = line.strip_prefix("branch ")
        && let Some(w) = base.worktrees.last_mut()
    {
        w.branch = Some(b.trim_start_matches("refs/heads/").to_string());
    }
}

#[derive(Deserialize)]
struct RunningClaude {
    kind: Option<String>,
    pid: Option<u32>,
    status: Option<String>,
    #[serde(rename = "waitingFor")]
    waiting_for: Option<serde_json::Value>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "startedAt")]
    started_at: Option<u64>,
}

/// What `claude agents` says of the Claudes in one terminal.
struct InSession {
    /// The most pressing one's activity, and what it waits on.
    activity: Activity,
    waiting_for: Option<String>,
    /// The newest one's conversation, and when it started.
    run: Option<(u64, ClaudeRun)>,
}

/// Maps `claude agents --json` output to terminals: each interactive
/// Claude's pid is walked up the process tree to the program a terminal
/// runs (`terminals`, by pid).
fn claude_activity(
    agents: &str,
    terminals: &HashMap<u32, String>,
    parents: &HashMap<u32, u32>,
) -> Option<HashMap<String, InSession>> {
    // No answer means an older Claude Code, or none.
    let agents: Vec<RunningClaude> = serde_json::from_str(agents.trim()).ok()?;
    let mut found: HashMap<String, InSession> = HashMap::new();
    for agent in agents {
        let (Some("interactive"), Some(pid)) = (agent.kind.as_deref(), agent.pid) else {
            continue;
        };
        let activity = match agent.status.as_deref() {
            Some("busy") => Activity::Working,
            Some("waiting") => Activity::NeedsInput,
            Some("idle") => Activity::Idle,
            _ => continue,
        };
        let waiting_for = agent.waiting_for.map(|v| match v {
            serde_json::Value::String(s) => s,
            other => other.to_string(),
        });
        let run = match (agent.session_id, agent.cwd) {
            (Some(id), Some(path)) => Some((agent.started_at.unwrap_or(0), ClaudeRun { id, path })),
            _ => None,
        };
        let mut p = pid;
        for _ in 0..32 {
            if let Some(address) = terminals.get(&p) {
                let entry = found.entry(address.clone()).or_insert(InSession {
                    activity: Activity::Unknown,
                    waiting_for: None,
                    run: None,
                });
                if activity > entry.activity {
                    (entry.activity, entry.waiting_for) = (activity, waiting_for);
                }
                if let Some(run) = run
                    && entry.run.as_ref().is_none_or(|(at, _)| run.0 >= *at)
                {
                    entry.run = Some(run);
                }
                break;
            }
            match parents.get(&p) {
                Some(&parent) if parent > 1 => p = parent,
                _ => break,
            }
        }
    }
    Some(found)
}

/// The worktree containing `path`, preferring the deepest match: linked
/// worktrees under `.claude/worktrees/` sit inside the main checkout.
pub fn find_worktree(bases: &[BaseView], path: &str) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize, usize)> = None;
    for (bi, b) in bases.iter().enumerate() {
        for (wi, w) in b.worktrees.iter().enumerate() {
            let inside = path
                .strip_prefix(&w.path)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));
            if inside && best.is_none_or(|(_, _, len)| w.path.len() > len) {
                best = Some((bi, wi, w.path.len()));
            }
        }
    }
    best.map(|(bi, wi, _)| (bi, wi))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hc::{Foreground, Status};
    use crate::test_support::{HcServer, TempDir, wait_until};
    use std::collections::BTreeMap;
    use std::time::Duration;

    /// `claude agents`, and the servers listening, are asked of a host at
    /// most every 30 seconds; in between, a refresh goes by what it said
    /// last.
    #[test]
    fn claude_agents_and_ports_are_asked_at_most_every_30_seconds() {
        let server = crate::test_support::HcServer::new("agents-every");
        let cfg = server.config();
        let host = &cfg.hosts[0];
        let place = format!(
            "{}|{}",
            host.ssh,
            host.hc_session.as_deref().unwrap_or_default()
        );
        fn asked_at<T>(asked: &Asked<T>, place: &str) -> Option<std::time::Instant> {
            let all = asked.lock().unwrap();
            all.iter().find(|(p, _)| p == place).map(|(_, (at, _))| *at)
        }
        fn age<T>(asked: &Asked<T>, place: &str, by: std::time::Duration) {
            for (p, (at, _)) in asked.lock().unwrap().iter_mut() {
                if p == place {
                    *at -= by;
                }
            }
        }
        let asked = || (asked_at(&AGENTS, &place), asked_at(&PORTS, &place));
        assert!(!script(&cfg, host, false, false).contains("@@agents"));
        assert!(script(&cfg, host, true, false).contains("@@agents"));
        assert!(!script(&cfg, host, false, false).contains("@@ports"));
        assert!(script(&cfg, host, false, true).contains("@@ports"));

        let servers = fetch_host(&cfg, host).machine.servers;
        let first = asked();
        assert!(
            first.0.is_some() && first.1.is_some(),
            "asked the first time"
        );
        assert_eq!(fetch_host(&cfg, host).machine.servers, servers, "as before");
        assert_eq!(asked(), first, "not again so soon");
        // Once one is due, it alone is asked again.
        age(&AGENTS, &place, AGENTS_EVERY);
        fetch_host(&cfg, host);
        assert!(asked().0 > first.0, "agents again");
        assert_eq!(asked().1, first.1);
        age(&PORTS, &place, PORTS_EVERY);
        fetch_host(&cfg, host);
        assert!(asked().1 > first.1, "ports again");
    }

    /// `claude agents` is asked only when a Claude may run: without a
    /// session registry, or with a live pid in it. Otherwise the answer is
    /// known: none.
    #[test]
    fn claude_agents_runs_only_when_a_claude_does() {
        let dir = TempDir::new("claude-registry");
        let run = || {
            let out = std::process::Command::new("/bin/sh")
                .args(["-c", CLAUDE])
                .env("CLAUDE_CONFIG_DIR", &*dir)
                // Neither pgrep nor claude: only the registry finds a
                // Claude, and asking `claude agents` gets no answer.
                .env("PATH", "/nonexistent")
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        let (asked, none) = ("@@agents\n\n", "@@agents\n[]\n");
        assert_eq!(run(), asked, "no registry");
        let sessions = dir.join("sessions");
        std::fs::create_dir(&sessions).unwrap();
        assert_eq!(run(), none);
        // Ended long ago: no process has that pid.
        std::fs::write(sessions.join("999999999.json"), "{}").unwrap();
        assert_eq!(run(), none);
        let alive = format!("{}.json", std::process::id());
        std::fs::write(sessions.join(alive), "{}").unwrap();
        assert_eq!(run(), asked);
    }

    fn cfg() -> Config {
        toml::from_str(
            r#"
            [[hosts]]
            name = "agent"
            ssh = "agent"

            [[bases]]
            host = "agent"
            path = "~/projects/api"
            "#,
        )
        .unwrap()
    }

    fn terminal(address: &str, cwd: &str, pid: u32, labels: &[(&str, &str)]) -> Terminal {
        Terminal {
            address: address.into(),
            state: "live".into(),
            cwd: cwd.into(),
            pid: Some(pid),
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>(),
            ..Default::default()
        }
    }

    /// A thread's helpers go under it as a tree: Claude's subagents as it
    /// reports them, then the terminals its agent started, each with its
    /// own under it. A helper whose agent is gone is a thread again.
    #[test]
    fn helpers_go_under_their_thread() {
        let cfg = cfg();
        let data = parse(&cfg, &cfg.hosts[0], "");
        let record = |state: &str, title: &str| Status {
            state: state.into(),
            title: title.into(),
            ..Default::default()
        };
        // Started at `at`, as its pid is.
        let child = |address: &str, parent: &str, at: u32, label: &str| Terminal {
            parent: Some(parent.into()),
            status: Some(record("working", "")),
            created_ms: at.into(),
            ..terminal(address, "/tmp", at, &[(LABEL, label)])
        };
        let terminals = [
            Terminal {
                records: Some(BTreeMap::from([
                    ("a1".to_string(), record("working", "Explore the parser")),
                    ("a2".to_string(), record("blocked", "")),
                ])),
                ..terminal("agent/t1", "/tmp", 100, &[(LABEL, "main")])
            },
            // As the host's core calls its parent, and as this one does.
            child("agent/t3", "t1", 130, "fix"),
            child("agent/t2", "agent/t1", 120, "review"),
            Terminal {
                records: Some(BTreeMap::from([(
                    "b1".to_string(),
                    record("working", "Read the tests"),
                )])),
                ..child("agent/t4", "t2", 140, "deeper")
            },
            child("agent/t5", "agent/t9", 150, "orphan"),
        ];
        let h = view(&cfg, &cfg.hosts[0], &data, &terminals);
        let names: Vec<&str> = h.other.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["agent/t1", "agent/t5"]);
        let tree: Vec<(&str, &str, bool, usize, Activity)> = h.other[0]
            .helpers
            .iter()
            .map(|x| {
                (
                    x.id.as_str(),
                    x.label.as_str(),
                    x.terminal,
                    x.depth,
                    x.activity,
                )
            })
            .collect();
        assert_eq!(
            tree,
            [
                ("a1", "Explore the parser", false, 0, Activity::Working),
                ("a2", "Subagent", false, 0, Activity::NeedsInput),
                ("agent/t2", "review", true, 0, Activity::Working),
                ("agent/t4", "deeper", true, 1, Activity::Working),
                ("b1", "Read the tests", false, 2, Activity::Working),
                ("agent/t3", "fix", true, 0, Activity::Working),
            ]
        );
        assert!(h.other[1].helpers.is_empty());
    }

    #[test]
    fn parses_worktrees_and_assigns_sessions() {
        let cfg = cfg();
        let out = "@@base 0\n\
            worktree /home/agent/projects/api\nHEAD abc\nbranch refs/heads/main\n\n\
            worktree /home/agent/projects/api/.claude/worktrees/auth\nHEAD def\nbranch refs/heads/worktree-auth\n\n\
            worktree /home/agent/projects/api/.worktrees/x/api\nHEAD 0123456789abcdef\ndetached\n\n\
            @@ps\n  90     1\n 100     1\n 110     1\n 200   100\n 250   100\n 300   250\n 400     1\n\
            @@agents\n[\n\
              {\"kind\":\"interactive\",\"pid\":200,\"status\":\"busy\"},\n\
              {\"kind\":\"interactive\",\"pid\":300,\"status\":\"waiting\",\"waitingFor\":\"Allow Bash(rm -rf build)?\"},\n\
              {\"kind\":\"interactive\",\"pid\":400,\"status\":\"busy\"},\n\
              {\"kind\":\"background\",\"state\":\"working\"}\n\
            ]\n";
        let data = parse(&cfg, &cfg.hosts[0], out);
        let auth = "/home/agent/projects/api/.claude/worktrees/auth";
        let terminals = [
            terminal(
                "agent/t1",
                "/home/agent/projects/api",
                90,
                &[(LABEL, "review"), (KIND, "claude"), (WORKTREE, auth)],
            ),
            terminal("agent/t2", "/home/agent/projects/api", 100, &[]),
            terminal("agent/t3", "/tmp", 110, &[(KIND, "shell")]),
            // An agent's helper isn't a thread.
            Terminal {
                parent: Some("agent/t2".into()),
                ..terminal("agent/t4", "/tmp", 120, &[])
            },
        ];
        let h = view(&cfg, &cfg.hosts[0], &data, &terminals);
        let wts = &h.bases[0].worktrees;
        assert_eq!(wts.len(), 3);
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
        assert_eq!(wts[2].branch, None);
        assert_eq!(wts[2].head.as_deref(), Some("0123456789abcdef"));
        let names = |list: &[Session]| list.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&wts[0].sessions), ["agent/t2"]);
        assert_eq!(wts[1].sessions[0].name, "agent/t1");
        assert_eq!(wts[1].sessions[0].label, "review");
        assert_eq!(names(&h.other), ["agent/t3"]);
        // Two Claudes in one terminal: the one waiting (two levels below
        // the terminal's shell) outranks the busy one.
        let tests = &wts[0].sessions[0];
        assert_eq!(tests.activity, Activity::NeedsInput);
        assert_eq!(
            tests.waiting_for.as_deref(),
            Some("Allow Bash(rm -rf build)?")
        );
        // pid 400 is in no terminal.
        assert_eq!(wts[1].sessions[0].activity, Activity::Unknown);
        assert_eq!(h.other[0].activity, Activity::Unknown);
        // Claude's icon: started for Claude, or Claude found running.
        assert!(wts[1].sessions[0].runs_claude());
        assert!(tests.runs_claude());
        assert!(!h.other[0].runs_claude());
    }

    /// What Claude reports to herdr-core is its status, ahead of what
    /// `claude agents` last said.
    #[test]
    fn claudes_own_status_comes_first() {
        let cfg = cfg();
        let data = parse(
            &cfg,
            &cfg.hosts[0],
            "@@ps\n 100 1\n 200 100\n@@agents\n[{\"kind\":\"interactive\",\"pid\":200,\"status\":\"busy\"}]\n",
        );
        let reported = |state: &str, msg: &str| Terminal {
            status: Some(Status {
                state: state.into(),
                msg: msg.into(),
                ..Default::default()
            }),
            ..terminal("agent/t1", "/tmp", 100, &[])
        };
        let shown = |t: Terminal| {
            let h = view(&cfg, &cfg.hosts[0], &data, &[t]);
            let s = &h.other[0];
            (s.activity, s.waiting_for.clone())
        };
        assert_eq!(
            shown(reported("blocked", "Allow Bash(ls)?")),
            (Activity::NeedsInput, Some("Allow Bash(ls)?".into()))
        );
        assert_eq!(shown(reported("done", "")), (Activity::Idle, None));
        assert_eq!(
            shown(terminal("agent/t1", "/tmp", 100, &[])),
            (Activity::Working, None),
            "no report: claude agents says"
        );
    }

    /// What a program said is over once its shell is back at its prompt: a
    /// Claude killed outright never says it stopped.
    #[test]
    fn a_status_left_at_the_prompt_is_over() {
        let cfg = cfg();
        let data = parse(&cfg, &cfg.hosts[0], "");
        let left = |program: &str, pid: u32| Terminal {
            status: Some(Status {
                state: "working".into(),
                ..Default::default()
            }),
            foreground: Some(Foreground {
                pid,
                name: program.into(),
                ..Default::default()
            }),
            ..terminal("agent/t1", "/tmp", 100, &[])
        };
        let shown = |t: Terminal| view(&cfg, &cfg.hosts[0], &data, &[t]).other[0].activity;
        assert_eq!(shown(left("claude", 200)), Activity::Working);
        assert_eq!(shown(left("zsh", 100)), Activity::Unknown);
    }

    /// Which shell is noted, to tell when a host restarted: one started for
    /// Claude at once, one with a Claude typed by hand once that's found,
    /// and one back from a restart only once Claude runs in it again.
    #[test]
    fn a_claude_threads_shell_is_noted_as_it_runs() {
        let cfg = cfg();
        let data = parse(&cfg, &cfg.hosts[0], "");
        let session = |labels: &[(&str, &str)]| {
            let t = terminal("agent/t1", "/tmp", 100, labels);
            view(&cfg, &cfg.hosts[0], &data, &[t]).other.remove(0)
        };
        let run = || {
            Some(ClaudeRun {
                id: "id".into(),
                path: "/tmp".into(),
            })
        };
        assert_eq!(
            session(&[("lemur.kind", "claude")]).shell_to_note(),
            Some(100)
        );
        let plain = session(&[("lemur.kind", "shell")]);
        assert_eq!(plain.shell_to_note(), None, "only a shell");
        let codex = Session {
            activity: Activity::Working,
            ..session(&[("lemur.kind", "codex")])
        };
        assert_eq!(codex.agent(), Some(Agent::Codex));
        assert_eq!(codex.shell_to_note(), None, "not kept to resume");
        let typed = Session {
            claude: run(),
            ..plain
        };
        assert_eq!(typed.shell_to_note(), Some(100), "Claude typed by hand");
        let noted = session(&[("lemur.kind", "claude"), ("lemur.shell", "100")]);
        assert_eq!(noted.shell_to_note(), None, "noted already");
        let back = session(&[("lemur.kind", "claude"), ("lemur.shell", "99")]);
        assert_eq!(back.shell_to_note(), None, "still to resume");
        let resumed = Session {
            claude: run(),
            ..back
        };
        assert_eq!(resumed.shell_to_note(), Some(100), "resumed there");
    }

    /// A Claude killed leaves what it last said with herdr-core, and in
    /// `claude agents`' last answer: with its shell back at its prompt,
    /// neither it nor its subagents show as working.
    #[test]
    fn a_dead_claude_says_nothing_more() {
        let cfg = cfg();
        let mut data = parse(&cfg, &cfg.hosts[0], "");
        data.agents =
            r#"[{"kind":"interactive","pid":200,"sessionId":"s1","cwd":"/tmp","status":"busy"}]"#
                .into();
        data.parents = HashMap::from([(200, 100)]);
        let left = |program: &str, pid: u32| Terminal {
            records: Some(BTreeMap::from([(
                "a1".to_string(),
                Status {
                    state: "working".into(),
                    ..Default::default()
                },
            )])),
            foreground: Some(Foreground {
                pid,
                name: program.into(),
                ..Default::default()
            }),
            ..terminal("agent/t1", "/tmp", 100, &[])
        };
        let shown = |t: Terminal| {
            let s = view(&cfg, &cfg.hosts[0], &data, &[t]).other.remove(0);
            (s.activity, s.helpers.len())
        };
        assert_eq!(shown(left("claude", 200)), (Activity::Working, 1));
        assert_eq!(shown(left("zsh", 100)), (Activity::Unknown, 0));

        // A helper's terminal is the same.
        let helper = |program: &str, pid: u32| Terminal {
            parent: Some("agent/t1".into()),
            status: Some(Status {
                state: "working".into(),
                ..Default::default()
            }),
            foreground: Some(Foreground {
                pid,
                name: program.into(),
                ..Default::default()
            }),
            ..terminal("agent/t2", "/tmp", 300, &[])
        };
        let of = |t: Terminal| {
            let main = terminal("agent/t1", "/tmp", 100, &[]);
            let shown = view(&cfg, &cfg.hosts[0], &data, &[main, t]);
            shown.other[0].helpers[0].activity
        };
        assert_eq!(of(helper("claude", 400)), Activity::Working);
        assert_eq!(of(helper("zsh", 300)), Activity::Unknown);
    }

    /// A terminal lemur didn't label is called by what it shows.
    #[test]
    fn unlabelled_terminals_are_named_by_their_title_or_program() {
        let named = |title: &str, program: &str| {
            shown_name(&Terminal {
                title: title.into(),
                foreground: Some(Foreground {
                    name: program.into(),
                    ..Default::default()
                }),
                ..Default::default()
            })
        };
        assert_eq!(named("✳ Fix the login", "claude"), "Fix the login");
        assert_eq!(named("~/src/app", "fish"), "fish");
        assert_eq!(named("me@box: ~", "bash"), "bash");
        assert_eq!(named("", ""), "terminal");
    }

    /// A thread you haven't named shows what its program calls it, while it
    /// runs, unless that only says what it is or where; a name of yours stays.
    #[test]
    fn a_thread_follows_its_programs_title_until_named() {
        let shown = |own: Option<&str>, title: &str, program: &str| {
            let labels: Vec<(&str, &str)> = own.iter().map(|o| (LABEL, *o)).collect();
            let t = Terminal {
                title: title.into(),
                foreground: Some(Foreground {
                    pid: if SHELLS.contains(&program) { 100 } else { 200 },
                    name: program.into(),
                    cwd: "/srv/api".into(),
                }),
                ..terminal("t1", "/srv/api", 100, &labels)
            };
            let s = session(&t, 0);
            (s.label, s.own_label)
        };
        let is = |label: &str, own: &str| (label.to_string(), own.to_string());
        let claude = Some("claude");
        assert_eq!(
            shown(claude, "✳ Fix the login", "claude"),
            is("Fix the login", "claude")
        );
        assert_eq!(
            shown(claude, "✳ Claude Code", "claude"),
            is("claude", "claude")
        );
        assert_eq!(shown(Some("codex"), "api", "codex"), is("codex", "codex"));
        assert_eq!(shown(Some("codex"), "⠋ api", "codex"), is("codex", "codex"));
        assert_eq!(shown(Some("shell"), "~/src", "node"), is("shell", "shell"));
        assert_eq!(
            shown(Some("shell"), "npm run dev", "node"),
            is("npm run dev", "shell")
        );
        // Over, what it said is over too.
        assert_eq!(
            shown(claude, "✳ Fix the login", "fish"),
            is("claude", "claude")
        );
        // Yours stays.
        assert_eq!(
            shown(Some("review"), "✳ Fix the login", "claude"),
            is("review", "review")
        );
        // A terminal Lemur didn't start is named by its title, as ever.
        assert_eq!(
            shown(None, "✳ Fix the login", "claude"),
            is("Fix the login", "Fix the login")
        );
    }

    /// Each session learns its Claude's conversation, the newest one's if
    /// it has several; kept threads whose session is gone are filed where
    /// they ran.
    #[test]
    fn claude_conversations_and_gone_threads() {
        let mut cfg = cfg();
        let thread = |session: &str, path: &str| ClaudeThread {
            host: "agent".into(),
            session: session.into(),
            path: path.into(),
            claude_session: "0fdfabeb-ca80-4916-a103-275bbb5e8614".into(),
            label: None,
            issue: None,
        };
        let auth = "/home/agent/projects/api/.claude/worktrees/auth";
        cfg.threads = vec![
            thread("agent/t1", "/home/agent/projects/api"),
            thread("agent/t5", auth),
            thread("scratch--bold-wren", "/tmp/x"),
            ClaudeThread {
                host: "other".into(),
                ..thread("elsewhere", "/tmp")
            },
        ];
        let out = format!(
            "@@base 0\nworktree /home/agent/projects/api\nworktree {auth}\n\
            @@ps\n 100     1\n 200   100\n 300   100\n\
            @@agents\n[\n\
              {{\"kind\":\"interactive\",\"pid\":200,\"status\":\"busy\",\"sessionId\":\"old\",\"cwd\":\"/a\",\"startedAt\":5}},\n\
              {{\"kind\":\"interactive\",\"pid\":300,\"status\":\"idle\",\"sessionId\":\"new\",\"cwd\":\"/b\",\"startedAt\":9}}\n\
            ]\n"
        );
        let data = parse(&cfg, &cfg.hosts[0], &out);
        let terminals = [terminal("agent/t1", "/home/agent/projects/api", 100, &[])];
        let h = view(&cfg, &cfg.hosts[0], &data, &terminals);
        let tests = &h.bases[0].worktrees[0].sessions[0];
        assert_eq!(tests.activity, Activity::Working);
        assert_eq!(
            tests.claude,
            Some(ClaudeRun {
                id: "new".into(),
                path: "/b".into()
            })
        );
        let gone = |list: &[ClaudeThread]| -> Vec<String> {
            list.iter().map(|t| t.session.clone()).collect()
        };
        assert!(h.bases[0].worktrees[0].gone.is_empty(), "agent/t1 runs");
        assert_eq!(gone(&h.bases[0].worktrees[1].gone), ["agent/t5"]);
        assert_eq!(gone(&h.gone), ["scratch--bold-wren"]);
    }

    #[test]
    fn worktrees_keep_the_order_they_were_put_in() {
        let mut cfg = cfg();
        cfg.bases[0].worktree_order = vec!["/r/c".into(), "/r".into()];
        let out = "@@base 0\nworktree /r\nworktree /r/a\nworktree /r/b\nworktree /r/c\n";
        let h = view(&cfg, &cfg.hosts[0], &parse(&cfg, &cfg.hosts[0], out), &[]);
        let paths: Vec<&str> = h.bases[0]
            .worktrees
            .iter()
            .map(|w| w.path.as_str())
            .collect();
        assert_eq!(paths, ["/r/c", "/r", "/r/a", "/r/b"]);
        // Moved or not, the main checkout is known.
        let main: Vec<bool> = h.bases[0].worktrees.iter().map(|w| w.main).collect();
        assert_eq!(main, [false, true, false, false]);
    }

    #[test]
    fn reports_base_errors() {
        let cfg = cfg();
        let data = parse(&cfg, &cfg.hosts[0], "@@base 0\n@@error no such directory\n");
        let h = view(&cfg, &cfg.hosts[0], &data, &[]);
        assert_eq!(h.bases[0].error.as_deref(), Some("no such directory"));
    }

    /// The labels lemur puts on a terminal file it, name it and place it.
    #[test]
    fn labels_file_and_name_threads() {
        let server = HcServer::new("labels");
        let cfg = server.config();
        server.run(
            "/",
            &[
                "lemur.kind=claude",
                "lemur.worktree=/srv/a b",
                "lemur.label=fix",
                "lemur.order=0",
            ],
            &["sleep", "600"],
        );
        server.run("/tmp", &[], &["sleep", "600"]);
        let mut other = vec![];
        assert!(wait_until(Duration::from_secs(5), || {
            other = fetch(&cfg).hosts[0].other.clone();
            other.len() == 2
        }));
        assert!(other[0].started_claude);
        assert_eq!(other[0].path, "/srv/a b");
        assert_eq!((other[0].label.as_str(), other[0].order), ("fix", Some(0)));
        assert!(!other[1].started_claude);
        assert_eq!(other[1].path, "/tmp");
        assert_eq!(other[1].order, None);
    }

    /// A folder without git is one row, holding the threads started in it;
    /// added through a link, it's filed under its real path.
    #[test]
    fn a_folder_without_git_is_a_single_row() {
        let server = HcServer::new("plain");
        let dir = TempDir::new("plain");
        let real = dir.join("real").to_string_lossy().into_owned();
        std::fs::create_dir(&real).unwrap();
        let linked = dir.join("link");
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        server.run(
            &real,
            &[&format!("lemur.worktree={real}")],
            &["sleep", "600"],
        );
        let mut cfg = server.config();
        cfg.bases
            .push(crate::config::Base::new("local", &linked.to_string_lossy()));

        let mut snap = Snapshot::default();
        assert!(wait_until(Duration::from_secs(5), || {
            snap = fetch(&cfg);
            snap.hosts[0].bases[0]
                .worktrees
                .first()
                .is_some_and(|w| !w.sessions.is_empty())
        }));
        let base = &snap.hosts[0].bases[0];
        assert!(base.plain);
        assert_eq!(base.error, None);
        assert_eq!(base.worktrees.len(), 1);
        assert_eq!(base.worktrees[0].path, real);
        assert_eq!(base.worktrees[0].sessions.len(), 1);
    }

    #[test]
    fn threads_keep_the_order_they_were_put_in() {
        let server = HcServer::new("order");
        let names: Vec<String> = (0..4)
            .map(|_| server.run("/", &[], &["sleep", "600"]))
            .collect();
        let shown = |cfg: &Config| -> Vec<String> {
            fetch(cfg).hosts[0]
                .other
                .iter()
                .map(|s| s.name.clone())
                .collect()
        };
        let cfg = server.config();
        assert!(wait_until(Duration::from_secs(5), || shown(&cfg) == names));
        // d and b were placed; a and c never were, so they follow, oldest
        // first.
        let (a, b, c, d) = (&names[0], &names[1], &names[2], &names[3]);
        crate::actions::set_order(&cfg, "local", &[d.clone(), b.clone()]).unwrap();
        let placed = [d, b, a, c].map(String::clone);
        assert!(wait_until(Duration::from_secs(5), || shown(&cfg) == placed));
        // One put away goes last, whatever its place, and is marked so.
        crate::actions::set_away(&cfg, "local", d, true).unwrap();
        let away = [b, a, c, d].map(String::clone);
        assert!(wait_until(Duration::from_secs(5), || shown(&cfg) == away));
        assert!(fetch(&cfg).hosts[0].other[3].away);
        crate::actions::set_away(&cfg, "local", d, false).unwrap();
        assert!(wait_until(Duration::from_secs(5), || shown(&cfg) == placed));
    }

    /// Only a shell at its prompt is idle: what it runs is busy, and so
    /// is a program in a terminal of its own, or one not yet read.
    #[test]
    fn only_a_shell_at_its_prompt_is_idle() {
        let live = Terminal {
            state: "live".into(),
            ..Default::default()
        };
        assert!(busy(&live), "not read yet");
        let finished = Terminal {
            state: "finished".into(),
            ..Default::default()
        };
        assert!(!busy(&finished));

        let server = HcServer::new("busy");
        let shell = server.run("/", &[], &["sh"]);
        let own = server.run("/", &[], &["sleep", "600"]);
        let core = server.core();
        let read = |address: &str| {
            core.terminals()
                .into_iter()
                .find(|t| t.address == address && t.foreground.is_some())
        };
        let busy_now = |address: &str| read(address).map(|t| busy(&t));
        assert!(
            wait_until(Duration::from_secs(5), || busy_now(&shell) == Some(false)),
            "{:?}",
            read(&shell)
        );
        assert!(wait_until(Duration::from_secs(5), || read(&own).is_some()));
        assert_eq!(busy_now(&own), Some(true), "{:?}", read(&own));
        core.type_line(&shell, "sleep 600").unwrap();
        assert!(
            wait_until(Duration::from_secs(5), || busy_now(&shell) == Some(true)),
            "{:?}",
            read(&shell)
        );
    }

    /// A shell runs a command from when it starts until its prompt is back,
    /// but not while what it started is a program you work in, as a pager.
    #[test]
    fn a_shell_runs_a_command_until_its_prompt_is_back() {
        let server = HcServer::new("command");
        let (quick, pager) = (server.run("/", &[], &["sh"]), server.run("/", &[], &["sh"]));
        let core = server.core();
        let read = |address: &str| {
            core.terminals()
                .into_iter()
                .find(|t| t.address == address && t.foreground.is_some())
                .map(|t| session(&t, 0))
        };
        let command = |address: &str| read(address).map(|s| s.command);
        assert!(wait_until(Duration::from_secs(5), || command(&quick) == Some(None)));
        assert!(wait_until(Duration::from_secs(5), || command(&pager) == Some(None)));
        core.type_line(&quick, "sleep 3").unwrap();
        assert!(
            wait_until(Duration::from_secs(5), || command(&quick) == Some(Some("sleep".into()))),
            "{:?}",
            read(&quick)
        );
        let s = read(&quick).unwrap();
        assert_eq!((s.shell_runs(), s.doing()), (Some("sleep"), Activity::Working));
        assert!(
            wait_until(Duration::from_secs(10), || command(&quick) == Some(None)),
            "back at its prompt: {:?}",
            read(&quick)
        );
        assert_eq!(read(&quick).unwrap().doing(), Activity::Unknown);

        core.type_line(&pager, "less /etc/hosts").unwrap();
        assert!(
            wait_until(Duration::from_secs(5), || read(&pager).is_some_and(|s| s.busy)),
            "{:?}",
            read(&pager)
        );
        assert_eq!(command(&pager), Some(None), "less waits on you");
    }
}

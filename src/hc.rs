//! herdr-core (`hc`), which runs lemur's terminals: on this Mac, and on the
//! hosts it reaches over SSH. lemur talks to the core here through one
//! framed connection (`herdr-core connect`): JSON-RPC on channel 0, each
//! attached terminal's bytes on a channel of its own, and the event stream
//! that keeps the list of terminals current. The core keeps the SSH
//! connections to other hosts and runs their commands there.
//!
//! The protocol is herdr-core's (`schema/README.md` in its source). This
//! client follows the one in herdr-app (`src/hc.rs`, Apache-2.0).

use crate::config::Host;
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::Duration;

const HEADER: usize = 9;
const DATA: u8 = 0;
const CREDIT: u8 = 1;
const CLOSE: u8 = 2;
const RESIZE: u8 = 3;
const FOCUS: u8 = 4;
const FACTS: u8 = 5;

/// The core API generation lemur speaks.
const GENERATION: u64 = 1;

/// A terminal as the core describes it; lemur reads only these.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Terminal {
    /// `t7` on this Mac, `agent/t7` on the host the core calls `agent`.
    pub address: String,
    /// "live" or "finished".
    pub state: String,
    pub argv: Vec<String>,
    /// Where it started.
    pub cwd: String,
    pub labels: BTreeMap<String, String>,
    pub title: String,
    /// Its program's pid, on its own host.
    pub pid: Option<u32>,
    /// The terminal that started it (an agent's helper).
    pub parent: Option<String>,
    /// What its program last reported (OSC 7501), as Claude Code does.
    pub status: Option<Status>,
    /// What it reports of its own helpers, by id: Claude Code reports each
    /// subagent it runs, its task as the title, until it's done.
    pub records: Option<BTreeMap<String, Status>>,
    /// What runs in it now, and where.
    pub foreground: Option<Foreground>,
    pub cols: u16,
    pub rows: u16,
    /// When it started, and when its program last reported a status, in
    /// ms since the epoch.
    pub created_ms: u64,
    pub status_ms: Option<u64>,
}

/// A program's own word for what it's doing: idle, working, done, blocked
/// or error, and what it says about it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Status {
    pub state: String,
    pub msg: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Foreground {
    /// Its process group's leader: the terminal's own program while that
    /// is a shell at its prompt.
    pub pid: u32,
    pub name: String,
    pub cwd: String,
}

impl Terminal {
    /// The machine it runs on, as the core calls it; None on this Mac.
    pub fn machine(&self) -> Option<&str> {
        machine_of(&self.address)
    }

    /// Where its program is now, else where it started.
    pub fn folder(&self) -> &str {
        self.foreground
            .as_ref()
            .map(|f| f.cwd.as_str())
            .filter(|c| !c.is_empty())
            .unwrap_or(&self.cwd)
    }
}

/// The machine in an address (`agent` in `agent/t7`); None on this Mac.
pub fn machine_of(address: &str) -> Option<&str> {
    address.rsplit_once('/').map(|(machine, _)| machine)
}

/// What an attached terminal's channel carries to lemur.
#[derive(Debug)]
pub enum Stream {
    /// The program's output, after a first paint of its screen.
    Data(Vec<u8>),
    /// The core ended the stream: the program exited, or the terminal was
    /// lost and will restart (`reason`).
    Closed(Value),
    /// The connection to the core dropped; the program likely still runs.
    Lost,
}

/// A terminal attached on its own channel.
pub struct Attached {
    pub channel: u32,
    pub stream: UnboundedReceiver<Stream>,
}

/// What a command run without a terminal printed, and how it ended.
pub struct Output {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub timed_out: bool,
    /// It printed more than the core keeps (8 MB).
    pub truncated: bool,
}

/// A refusal from the core, with its stable kind (`not_found`, ...).
#[derive(Debug)]
pub struct RpcError(pub Value);

impl RpcError {
    pub fn kind(&self) -> Option<&str> {
        self.0.pointer("/data/kind").and_then(Value::as_str)
    }

    fn message(&self) -> Option<&str> {
        self.0.get("message").and_then(Value::as_str)
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message().unwrap_or("herdr-core refused"))?;
        if let Some(hint) = self.0.pointer("/data/hint").and_then(Value::as_str) {
            write!(f, " ({hint})")?;
        }
        Ok(())
    }
}

impl std::error::Error for RpcError {}

/// The kind of refusal an error carries, if it's the core's.
pub fn error_kind(e: &anyhow::Error) -> Option<&str> {
    e.downcast_ref::<RpcError>().and_then(RpcError::kind)
}

/// Where replies and stream frames go.
#[derive(Default)]
struct Routes {
    closed: bool,
    replies: HashMap<u64, mpsc::Sender<Result<Value, Value>>>,
    streams: HashMap<u32, UnboundedSender<Stream>>,
    /// Frames for a channel whose reply hasn't been handled yet.
    early: HashMap<u32, Vec<Stream>>,
    /// Channels let go of, whose last frames, and the core's close, are
    /// still on their way: dropped, not kept as early.
    detached: std::collections::HashSet<u32>,
    /// The event stream's channel.
    events: Option<u32>,
}

/// The terminals as the event stream keeps them (schema/README.md, Events).
/// Events are JSON lines; a frame can end mid-line.
#[derive(Default)]
struct EventLines {
    pending: Vec<u8>,
    /// The last event applied: older ones were in the snapshot.
    seq: u64,
    terminals: Vec<Value>,
}

impl EventLines {
    /// From `events.subscribe`'s reply: its `seq` and `terminals`.
    fn new(snapshot: &Value) -> Self {
        EventLines {
            pending: vec![],
            seq: snapshot["seq"].as_u64().unwrap_or(0),
            terminals: snapshot["terminals"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        }
    }

    /// Applies the lines in `bytes`; true if a terminal changed.
    fn push(&mut self, bytes: &[u8]) -> bool {
        self.pending.extend_from_slice(bytes);
        let Some(end) = self.pending.iter().rposition(|b| *b == b'\n') else {
            return false;
        };
        let lines: Vec<u8> = self.pending.drain(..=end).collect();
        let mut changed = false;
        for line in lines.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
            let Ok(line) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            let p = &line["params"];
            match line["method"].as_str() {
                Some("missed") => {
                    self.terminals = p["terminals"].as_array().cloned().unwrap_or_default();
                    self.seq = p["seq"].as_u64().unwrap_or(self.seq);
                    changed = true;
                }
                Some("event") if p["seq"].as_u64().is_none_or(|s| s > self.seq) => {
                    self.seq = p["seq"].as_u64().unwrap_or(self.seq);
                    changed |= self.apply(p);
                }
                _ => {}
            }
        }
        changed
    }

    /// One event's change to the terminal it names.
    fn apply(&mut self, p: &Value) -> bool {
        let kind = p["type"].as_str().unwrap_or_default();
        let (address, data) = (&p["terminal"], &p["data"]);
        let at = self.terminals.iter().position(|t| &t["address"] == address);
        let whole = match kind {
            "created" | "restarted" => Some(data),
            "labels" => data.get("record"),
            _ => None,
        };
        if let Some(record) = whole {
            match at {
                Some(i) => self.terminals[i] = record.clone(),
                None => self.terminals.push(record.clone()),
            }
            return true;
        }
        let Some(i) = at else { return false };
        if kind == "removed" {
            self.terminals.remove(i);
            return true;
        }
        let t = &mut self.terminals[i];
        // A status event doesn't carry its time: herdr-core moves it when
        // the word changes, as here.
        if kind == "status" && t["status"]["state"] != data["status"]["state"] {
            t["status_ms"] = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64)
                .into();
        }
        let mut set = |keys: &[&str]| {
            for k in keys {
                t[*k] = data[*k].clone();
            }
        };
        match kind {
            "finished" | "lost" => {
                set(&["reason", "exit_code", "signal", "finished_ms"]);
                t["state"] = if kind == "lost" { "live" } else { "finished" }.into();
                if kind == "lost" {
                    t["pid"] = Value::Null;
                }
                if let Some(record) = t.as_object_mut() {
                    record.remove("foreground");
                }
            }
            "status" => set(&["status", "records", "reported"]),
            "turn" => set(&["turn"]),
            "title" => set(&["title"]),
            "labels" => set(&["labels"]),
            "keep" => set(&["keep"]),
            "size" => set(&["cols", "rows"]),
            "foreground" => t["foreground"] = data.clone(),
            _ => return false,
        }
        true
    }

    fn live(&self) -> Vec<Terminal> {
        self.terminals
            .iter()
            .filter(|t| t["state"] == "live")
            .filter_map(|t| serde_json::from_value(t.clone()).ok())
            .collect()
    }
}

/// One connection to a core.
pub struct Core {
    /// Frames for the writer thread: a core that stops reading never
    /// blocks the caller.
    writer: mpsc::Sender<Vec<u8>>,
    routes: Arc<Mutex<Routes>>,
    lines: Arc<Mutex<EventLines>>,
    watchers: Arc<Mutex<Vec<UnboundedSender<()>>>>,
    lost: Arc<AtomicBool>,
    next_id: AtomicU64,
    child: Mutex<Child>,
    /// What the core said of each host, by lemur's name: its machine's
    /// name, or why it couldn't add it, and when. Each host has a lock of
    /// its own, so one being dialled (an offline host takes some 20 s)
    /// doesn't hold up the others.
    machines: Mutex<HashMap<String, Arc<Mutex<Option<Machine>>>>>,
}

/// A machine's name, or why the core couldn't add it, when, and whether it
/// needs you for it (`NeedsYou`).
type Machine = std::result::Result<String, (std::time::Instant, String, bool)>;

/// herdr-core can't add a host without you: herdr-core to install there,
/// or to update, a password, or a new host key. `hc machine add` asks for
/// them in a terminal (`actions::set_up_host`).
#[derive(Debug)]
pub struct NeedsYou(pub String);

impl std::fmt::Display for NeedsYou {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NeedsYou {}

/// Whether `e` is herdr-core needing you to add a host.
pub fn needs_you(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.downcast_ref::<NeedsYou>().is_some())
}

/// Whether herdr-core's answer to adding `host` asks for what only you can
/// give, as Herdr tells: then `hc machine add` asks in a terminal. By what
/// it says itself: its hint for a host that's only down speaks of passwords
/// too, and a host may be called `login01`.
fn asks_you(e: &anyhow::Error, host: &Host) -> bool {
    let Some(refused) = e.chain().find_map(|c| c.downcast_ref::<RpcError>()) else {
        return false;
    };
    let kinds = [
        "hc_missing",
        "incompatible",
        "login_needed",
        "auth_failed",
        "host_key",
    ];
    let mut said = refused.message().unwrap_or_default().to_lowercase();
    let host_part = host.ssh.rsplit('@').next().unwrap_or_default();
    for name in [host.ssh.as_str(), host_part, host.name.as_str()] {
        said = said.replace(&name.to_lowercase(), "");
    }
    refused.kind().is_some_and(|k| kinds.contains(&k))
        || [
            "password",
            "passphrase",
            "host key",
            "permission denied",
            "login",
        ]
        .iter()
        .any(|w| said.contains(w))
}

/// How long a host the core couldn't add isn't asked again: each refresh
/// would dial it again.
const MACHINE_RETRY: Duration = Duration::from_secs(30);

/// The most input a frame carries.
const INPUT_PART: usize = 4 << 20;

fn encode(channel: u32, kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(&channel.to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn read_frame(r: &mut impl Read) -> Option<(u32, u8, Vec<u8>)> {
    let mut h = [0u8; HEADER];
    r.read_exact(&mut h).ok()?;
    let channel = u32::from_le_bytes(h[0..4].try_into().ok()?);
    let len = u32::from_le_bytes(h[5..9].try_into().ok()?) as usize;
    let mut payload = vec![0; len];
    r.read_exact(&mut payload).ok()?;
    Some((channel, h[4], payload))
}

/// herdr-core: on PATH, else where its installer puts it.
pub fn binary() -> PathBuf {
    // Found once; not found, looked for again, as it may be installed
    // while Lemur runs.
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    if let Some(found) = BIN.get() {
        return found.clone();
    }
    let on_path = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("herdr-core"))
            .find(|p| p.is_file())
    });
    let installed = crate::config::home()
        .ok()
        .map(|h| h.join(".local/bin/herdr-core"))
        .filter(|p| p.is_file());
    match on_path.or(installed) {
        Some(found) => BIN.get_or_init(|| found).clone(),
        None => "herdr-core".into(),
    }
}

/// A `herdr-core connect` until it's a core's: one that fails the
/// handshake is ended and reaped when this drops. A std Child dropped as is
/// stays a zombie, and Lemur tries again every few seconds.
struct Connecting(Option<Child>);

impl Connecting {
    fn child(&mut self) -> &mut Child {
        self.0.as_mut().expect("a child until it's the core's")
    }
}

impl Drop for Connecting {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Core {
    /// Connects to the core of `session` (hc's default without one),
    /// starting it if needed, and follows its terminals.
    fn connect(session: Option<&str>) -> Result<Arc<Core>> {
        Core::connect_with(binary(), session)
    }

    fn connect_with(binary: PathBuf, session: Option<&str>) -> Result<Arc<Core>> {
        let mut command = Command::new(binary);
        command.arg("connect");
        if let Some(session) = session {
            command.args(["--session", session]);
        }
        #[cfg(test)]
        command.envs(test_env(session));
        let mut child = Connecting(Some(
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .context("can't run herdr-core: is it installed? See https://herdr.dev")?,
        ));
        let mut stdin = child.child().stdin.take().expect("piped stdin");
        let mut reader = BufReader::new(child.child().stdout.take().expect("piped stdout"));
        let hello = json!({"jsonrpc": "2.0", "id": 0, "method": "hello",
            "params": {"framed": true, "generation": GENERATION}});
        stdin.write_all(format!("{hello}\n").as_bytes())?;
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let hello: Value = serde_json::from_str(&line).context("herdr-core didn't say hello")?;
        if let Some(e) = hello.get("error") {
            bail!("herdr-core refused lemur: {}", RpcError(e.clone()));
        }
        let generation = hello["result"]["generation"].as_u64().unwrap_or(0);
        if generation != GENERATION {
            bail!(
                "herdr-core speaks protocol generation {generation}; this Lemur needs {GENERATION}"
            );
        }

        let (writer, frames) = mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("hc-writer".into())
            .spawn(move || {
                while let Ok(frame) = frames.recv() {
                    if stdin.write_all(&frame).is_err() {
                        break;
                    }
                }
            })?;
        let routes = Arc::new(Mutex::new(Routes::default()));
        let lines = Arc::new(Mutex::new(EventLines::default()));
        let watchers = Arc::new(Mutex::new(Vec::<UnboundedSender<()>>::new()));
        let lost = Arc::new(AtomicBool::new(false));
        let core = Arc::new(Core {
            writer: writer.clone(),
            routes: routes.clone(),
            lines: lines.clone(),
            watchers: watchers.clone(),
            lost: lost.clone(),
            next_id: AtomicU64::new(1),
            child: Mutex::new(child.0.take().expect("the child")),
            machines: Mutex::new(HashMap::new()),
        });
        std::thread::Builder::new()
            .name("hc-reader".into())
            .spawn(move || {
                while let Some((channel, kind, payload)) = read_frame(&mut reader) {
                    let mut r = routes.lock().unwrap();
                    if channel == 0 {
                        let Ok(msg) = serde_json::from_slice::<Value>(&payload) else {
                            continue;
                        };
                        let Some(id) = msg.get("id").and_then(Value::as_u64) else {
                            continue;
                        };
                        if let Some(tx) = r.replies.remove(&id) {
                            let _ = tx.send(match msg.get("error") {
                                Some(e) => Err(e.clone()),
                                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                            });
                        }
                        continue;
                    }
                    let item = match kind {
                        DATA => Stream::Data(payload),
                        CLOSE => {
                            Stream::Closed(serde_json::from_slice(&payload).unwrap_or(Value::Null))
                        }
                        _ => continue,
                    };
                    if r.events == Some(channel) {
                        drop(r);
                        follow(&item, channel, &lines, &watchers, &writer);
                        continue;
                    }
                    let closing = matches!(item, Stream::Closed(_));
                    match r.streams.get(&channel) {
                        Some(tx) => {
                            let _ = tx.unbounded_send(item);
                        }
                        None if r.detached.contains(&channel) => {
                            if closing {
                                r.detached.remove(&channel);
                            }
                        }
                        None => r.early.entry(channel).or_default().push(item),
                    }
                    if closing {
                        r.streams.remove(&channel);
                    }
                }
                // The core went away: tell every terminal, and fail calls
                // still waiting.
                lost.store(true, Ordering::SeqCst);
                let mut r = routes.lock().unwrap();
                r.closed = true;
                for (_, tx) in r.streams.drain() {
                    let _ = tx.unbounded_send(Stream::Lost);
                }
                for (_, tx) in r.replies.drain() {
                    let _ = tx.send(Err(json!({"message": "lost herdr-core"})));
                }
                watchers.lock().unwrap().clear();
            })?;

        let snapshot = core.call("events.subscribe", json!({}))?;
        let channel = snapshot["channel"]
            .as_u64()
            .ok_or_else(|| anyhow!("herdr-core gave events no channel"))?
            as u32;
        *core.lines.lock().unwrap() = EventLines::new(&snapshot);
        // Under the routes' lock, so the reader can't apply a frame read
        // meanwhile before these, which came first.
        let mut r = core.routes.lock().unwrap();
        r.events = Some(channel);
        for item in r.early.remove(&channel).unwrap_or_default() {
            follow(&item, channel, &core.lines, &core.watchers, &core.writer);
        }
        drop(r);
        Ok(core)
    }

    /// How many terminals' streams it follows.
    #[cfg(test)]
    pub fn streams(&self) -> usize {
        self.routes.lock().unwrap().streams.len()
    }

    /// Whether the connection to the core dropped.
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    /// The live terminals here and on the hosts the core reaches.
    pub fn terminals(&self) -> Vec<Terminal> {
        self.lines.lock().unwrap().live()
    }

    /// Wakes for every change to a terminal; ends when the core is lost.
    pub fn changes(&self) -> UnboundedReceiver<()> {
        let (tx, rx) = unbounded();
        if !self.is_lost() {
            self.watchers.lock().unwrap().push(tx);
        }
        rx
    }

    fn send(&self, channel: u32, kind: u8, payload: &[u8]) {
        let _ = self.writer.send(encode(channel, kind, payload));
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_timeout(method, params, Duration::from_secs(15))
    }

    pub fn call_timeout(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        {
            let mut routes = self.routes.lock().unwrap();
            if routes.closed {
                bail!("{method}: lost herdr-core");
            }
            routes.replies.insert(id, tx);
        }
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send(0, DATA, request.to_string().as_bytes());
        match rx.recv_timeout(timeout) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => {
                let refused = RpcError(e);
                // Removed or renamed meanwhile, as with `hc machine rm`:
                // what's known of the machines is out of date.
                if refused.kind() == Some("machine_unknown") {
                    self.machines.lock().unwrap().clear();
                }
                Err(anyhow::Error::new(refused).context(method.to_owned()))
            }
            Err(_) => {
                self.routes.lock().unwrap().replies.remove(&id);
                Err(anyhow!("{method}: no answer from herdr-core"))
            }
        }
    }

    /// The core's name for `host`, which it reaches over SSH; it learns
    /// the host the first time.
    pub fn machine(&self, host: &Host) -> Result<String> {
        let entry = self
            .machines
            .lock()
            .unwrap()
            .entry(host.name.clone())
            .or_default()
            .clone();
        // Held while asking, so two threads asking at once add a host once.
        let mut known = entry.lock().unwrap();
        let failed = |why: String, needs: bool| match needs {
            true => anyhow::Error::new(NeedsYou(why)),
            false => anyhow!(why),
        };
        match &*known {
            Some(Ok(name)) => return Ok(name.clone()),
            Some(Err((at, why, needs))) if at.elapsed() < MACHINE_RETRY => {
                return Err(failed(why.clone(), *needs));
            }
            _ => {}
        }
        match self.find_machine(host) {
            Ok(name) => {
                *known = Some(Ok(name.clone()));
                Ok(name)
            }
            Err(e) => {
                let (why, needs) = (format!("{e:#}"), asks_you(&e, host));
                *known = Some(Err((std::time::Instant::now(), why.clone(), needs)));
                Err(failed(why, needs))
            }
        }
    }

    /// Has the core said, a moment ago, that adding the host `name` needs
    /// you: what a test can't make herdr-core say without dialing a host.
    #[cfg(test)]
    pub fn needed_you(&self, name: &str) {
        let said = Err((std::time::Instant::now(), format!("{name} has no hc"), true));
        self.machines
            .lock()
            .unwrap()
            .insert(name.into(), Arc::new(Mutex::new(Some(said))));
    }

    /// Forgets what the core said of `host`, as once it's been set up.
    pub fn forget_machine(&self, host: &Host) {
        self.machines.lock().unwrap().remove(&host.name);
    }

    /// Finds the core's machine for `host`, or adds it.
    fn find_machine(&self, host: &Host) -> Result<String> {
        let list = self.call("machine.list", json!({}))?;
        let machines = list["machines"].as_array().cloned().unwrap_or_default();
        let wanted = machine_name(&host.name);
        // Its session, where the host's config names one.
        let session = |m: &Value| host.hc_session.as_deref().is_none_or(|s| m["session"] == s);
        let reached =
            |m: &&Value| m["target"] == host.ssh.as_str() && m["via"] != true && session(m);
        let named = machines.iter().find(|m| m["name"] == wanted.as_str());
        // One by the host's name that goes elsewhere would run all of it
        // there. One reached through a command (`--via`) was set up by
        // hand, and can't be compared.
        if let Some(m) = named
            && ((m["via"] != true && m["target"] != host.ssh.as_str()) || !session(m))
        {
            bail!(
                "herdr-core's machine {wanted} reaches {} (session {}), but the config's host {} \
                 says {} (session {}): change one, or remove the machine (`hc machine rm {wanted}`)",
                m["target"].as_str().unwrap_or("?"),
                m["session"].as_str().unwrap_or("default"),
                host.name,
                host.ssh,
                host.hc_session.as_deref().unwrap_or("default"),
            );
        }
        let found = named.or_else(|| machines.iter().find(reached));
        let name = match found.and_then(|m| m["name"].as_str()) {
            Some(name) => name.to_string(),
            None => {
                let added = self
                    .call_timeout(
                        "machine.add",
                        json!({"target": host.ssh, "name": wanted, "session": host.hc_session}),
                        Duration::from_secs(60),
                    )
                    .with_context(|| format!("adding {} to herdr-core", host.name))?;
                added["name"].as_str().unwrap_or(&wanted).to_string()
            }
        };
        Ok(name)
    }

    /// Starts a terminal on `on` (this Mac with None): `argv`, or with none
    /// the machine's own shell. Returns its address.
    pub fn run(
        &self,
        on: Option<&str>,
        cwd: &str,
        argv: &[String],
        labels: &BTreeMap<String, String>,
    ) -> Result<String> {
        let env =
            json!({"TERM": "xterm-256color", "COLORTERM": "truecolor", "TERM_PROGRAM": "lemur"});
        let r = self.call_timeout(
            "terminal.run",
            json!({"argv": argv, "cwd": cwd, "env": env, "labels": labels, "on": on, "keep": true}),
            Duration::from_secs(60),
        )?;
        r["address"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("herdr-core started a terminal without an address"))
    }

    /// Runs `argv` on `on` without a terminal, with `input` on its stdin.
    pub fn exec(
        &self,
        on: Option<&str>,
        argv: &[&str],
        input: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<Output> {
        let secs = timeout.as_secs().clamp(1, 600);
        let mut params = json!({"argv": argv, "on": on, "timeout": secs});
        if let Some(input) = input {
            params["input"] = BASE64.encode(input).into();
        }
        let r = self.call_timeout("machine.exec", params, Duration::from_secs(secs + 15))?;
        let decode = |key: &str| BASE64.decode(r[key].as_str().unwrap_or_default());
        Ok(Output {
            code: r["code"].as_i64().map(|c| c as i32),
            stdout: decode("stdout")?,
            stderr: String::from_utf8_lossy(&decode("stderr")?).into_owned(),
            timed_out: r["timed_out"] == true,
            truncated: r["truncated"] == true,
        })
    }

    /// Writes a file on `on`, making its folders; `path` is from home
    /// unless absolute. Returns where it went.
    pub fn write_file(&self, on: Option<&str>, path: &str, data: &[u8]) -> Result<String> {
        let r = self.call_timeout(
            "file.write",
            json!({"on": on, "path": path, "data": BASE64.encode(data)}),
            Duration::from_secs(120),
        )?;
        Ok(r["path"].as_str().unwrap_or(path).to_string())
    }

    /// Sets labels on a terminal; an empty value removes one.
    pub fn label(&self, address: &str, labels: &[(&str, &str)]) -> Result<()> {
        let labels: BTreeMap<&str, &str> = labels.iter().copied().collect();
        self.call(
            "terminal.label",
            json!({"terminal": address, "labels": labels}),
        )?;
        Ok(())
    }

    /// Pastes `text` into a terminal and presses Enter.
    pub fn type_line(&self, address: &str, text: &str) -> Result<()> {
        self.call_timeout(
            "terminal.send",
            json!({"terminal": address, "text": text}),
            Duration::from_secs(30),
        )?;
        Ok(())
    }

    /// Ends a terminal's program and forgets the terminal.
    pub fn kill(&self, address: &str) -> Result<()> {
        match self.call_timeout(
            "terminal.kill",
            json!({"terminal": address}),
            Duration::from_secs(30),
        ) {
            Ok(_) => {}
            // Already over: it only needs forgetting.
            Err(e) if error_kind(&e) == Some("finished") => {}
            // Already gone: closed and removed meanwhile, or started to be
            // removed once it ends (`hc run --rm`), as killing it just did.
            Err(e) if error_kind(&e) == Some("not_found") => return Ok(()),
            Err(e) => return Err(e),
        }
        let _ = self.call("terminal.rm", json!({"terminal": address}));
        Ok(())
    }

    /// Opens a terminal's byte stream at `cols` × `rows`: its screen and up
    /// to `history` lines of scrollback first (as many as herdr-core kept),
    /// then its output.
    pub fn attach(
        &self,
        address: &str,
        cols: u16,
        rows: u16,
        history: u32,
        display: Value,
    ) -> Result<Attached> {
        let r = self.call_timeout(
            "terminal.attach",
            json!({"terminal": address, "cols": cols, "rows": rows,
                "history": history, "display": display}),
            Duration::from_secs(30),
        )?;
        let channel = r["channel"]
            .as_u64()
            .ok_or_else(|| anyhow!("herdr-core attached {address} without a channel"))?
            as u32;
        let (tx, rx) = unbounded();
        let mut routes = self.routes.lock().unwrap();
        for item in routes.early.remove(&channel).unwrap_or_default() {
            let _ = tx.unbounded_send(item);
        }
        routes.streams.insert(channel, tx);
        Ok(Attached {
            channel,
            stream: rx,
        })
    }

    /// Keys and pastes for an attached terminal's program: in parts, as a
    /// frame over 16 MiB ends the whole connection.
    pub fn input(&self, channel: u32, bytes: &[u8]) {
        for part in bytes.chunks(INPUT_PART) {
            self.send(channel, DATA, part);
        }
    }

    /// Says `n` bytes of a stream were handled, so more may come.
    pub fn credit(&self, channel: u32, n: usize) {
        self.send(channel, CREDIT, &(n as u32).to_le_bytes());
    }

    pub fn resize(&self, channel: u32, cols: u16, rows: u16) {
        let mut p = cols.to_le_bytes().to_vec();
        p.extend(rows.to_le_bytes());
        self.send(channel, RESIZE, &p);
    }

    pub fn focus(&self, channel: u32, focused: bool) {
        self.send(channel, FOCUS, &[focused as u8]);
    }

    /// The colors and cell size lemur shows the terminal in, which the core
    /// answers programs' questions with.
    pub fn facts(&self, channel: u32, display: &Value) {
        self.send(channel, FACTS, display.to_string().as_bytes());
    }

    pub fn detach(&self, channel: u32) {
        let mut routes = self.routes.lock().unwrap();
        // Already closed by the core: nothing more comes.
        if routes.streams.remove(&channel).is_some() {
            routes.detached.insert(channel);
        }
        drop(routes);
        self.send(channel, CLOSE, &[]);
    }

    /// Waits until the core has read every frame sent before: input, for
    /// one, is only queued for the writer thread, which a process about to
    /// exit doesn't wait for. A call goes after them, and is answered.
    pub fn flush(&self) -> Result<()> {
        self.call("server.status", json!({}))?;
        Ok(())
    }
}

/// Applies a frame of the event stream, and wakes the watchers if a
/// terminal changed.
fn follow(
    item: &Stream,
    channel: u32,
    lines: &Mutex<EventLines>,
    watchers: &Mutex<Vec<UnboundedSender<()>>>,
    writer: &mpsc::Sender<Vec<u8>>,
) {
    let Stream::Data(bytes) = item else { return };
    let changed = lines.lock().unwrap().push(bytes);
    let _ = writer.send(encode(channel, CREDIT, &(bytes.len() as u32).to_le_bytes()));
    if changed {
        watchers
            .lock()
            .unwrap()
            .retain(|w| w.unbounded_send(()).is_ok());
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The connection to the core of each session lemur uses, made when first
/// needed and again once lost.
pub fn core(session: Option<&str>) -> Result<Arc<Core>> {
    // The default core is the one in use, Herdr's too: a test that reached
    // it could start it, and with it shells in the user's kept terminals.
    #[cfg(test)]
    assert!(
        session.is_some(),
        "a test reached herdr-core's default session: use an HcServer's"
    );
    static CORES: OnceLock<Mutex<HashMap<Option<String>, Arc<Core>>>> = OnceLock::new();
    let mut cores = CORES.get_or_init(Default::default).lock().unwrap();
    let key = session.map(str::to_owned);
    if let Some(core) = cores.get(&key)
        && !core.is_lost()
    {
        return Ok(core.clone());
    }
    let core = Core::connect(session)?;
    cores.insert(key, core.clone());
    Ok(core)
}

/// `name` as herdr-core takes a machine's: 1-32 of a-z 0-9 - _ ., starting
/// with a letter or digit, and not shaped like a terminal's id (`t7`). A
/// host typed as `me@Build.example.com` is `me-build.example.com`.
pub fn machine_name(name: &str) -> String {
    let lower: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name: String = lower
        .trim_start_matches(|c: char| !c.is_ascii_alphanumeric())
        .chars()
        .take(32)
        .collect();
    // herdr-core's ids: t, then 1-9, then its own digits.
    let id_shaped = name.strip_prefix('t').is_some_and(|rest| {
        rest.starts_with(|c: char| matches!(c, '1'..='9'))
            && rest
                .bytes()
                .all(|b| b"0123456789bcdfghjkmnpqrstvwxyz".contains(&b))
    });
    match name.as_str() {
        "" => "host".into(),
        _ if id_shaped => format!("host-{}", &name[..name.len().min(27)]),
        _ => name,
    }
}

/// The core that runs `host`'s terminals, and its name for the host: None
/// for this Mac.
pub fn for_host(host: &Host) -> Result<(Arc<Core>, Option<String>)> {
    if host.ssh == "local" {
        return Ok((core(host.hc_session.as_deref())?, None));
    }
    let core = core(machines_session(host).as_deref())?;
    let machine = core.machine(host)?;
    Ok((core, Some(machine)))
}

/// The session of the core on this Mac that reaches the other hosts, as
/// machines of its: herdr-core's default. Tests reach their stand-in hosts
/// through their own core.
pub fn machines_session(host: &Host) -> Option<String> {
    #[cfg(test)]
    return test_route(&host.name);
    #[cfg(not(test))]
    {
        let _ = host;
        None
    }
}

/// Whether `t` runs on `host`, as the core `for_host` gives calls it.
pub fn on_host(t: &Terminal, machine: Option<&str>) -> bool {
    t.machine() == machine
}

/// Environment variables, by name.
#[cfg(test)]
type Env = Vec<(String, String)>;

/// The environment each test session's core runs in.
#[cfg(test)]
static TEST_ENV: Mutex<Vec<(String, Env)>> = Mutex::new(Vec::new());

/// Gives the core of a test's session its own state folder.
#[cfg(test)]
pub fn set_test_env(session: &str, env: Env) {
    TEST_ENV.lock().unwrap().push((session.to_string(), env));
}

/// The session of the test core each stand-in host is reached through.
#[cfg(test)]
static TEST_ROUTES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Reaches the host named `host` through the test core of `session`. Each
/// test names its stand-in hosts its own way: tests run side by side.
#[cfg(test)]
pub fn set_test_route(host: &str, session: &str) {
    TEST_ROUTES
        .lock()
        .unwrap()
        .push((host.to_string(), session.to_string()));
}

#[cfg(test)]
fn test_route(host: &str) -> Option<String> {
    let routes = TEST_ROUTES.lock().unwrap();
    routes
        .iter()
        .rfind(|(h, _)| h == host)
        .map(|(_, s)| s.clone())
}

#[cfg(test)]
fn test_env(session: Option<&str>) -> Env {
    let env = TEST_ENV.lock().unwrap();
    env.iter()
        .find(|(s, _)| Some(s.as_str()) == session)
        .map(|(_, e)| e.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{HcServer, TempDir, wait_until};
    use futures::StreamExt;

    /// What comes for a terminal Lemur let go of, the core's close of it and
    /// output on its way, is dropped, not kept for the life of the
    /// connection.
    #[test]
    fn a_detached_stream_leaves_nothing_behind() {
        let server = HcServer::new("detach-early");
        let ticking = "while :; do echo tick; sleep 0.01; done";
        let address = server.run("/", &[], &["sh", "-c", ticking]);
        let core = server.core();
        let attached = core.attach(&address, 40, 10, 0, json!({})).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        core.detach(attached.channel);
        let closed = wait_until(Duration::from_secs(5), || {
            core.routes.lock().unwrap().detached.is_empty()
        });
        assert!(closed, "the core closed it");
        std::thread::sleep(Duration::from_millis(100));
        assert!(core.routes.lock().unwrap().early.is_empty());
    }

    /// Killing a terminal already gone, or one herdr-core removes as it
    /// ends (`hc run --rm`), is done: it's not an error.
    #[test]
    fn killing_a_terminal_already_gone_is_done() {
        let server = HcServer::new("kill-gone");
        let out = server.hc(&["run", "--bg", "--json", "--rm", "--", "sleep", "600"]);
        let started: Value = serde_json::from_str(&out).unwrap();
        let address = started["address"].as_str().unwrap();
        let core = server.core();
        core.kill(address).unwrap();
        core.kill(address).unwrap();
        core.kill("t999").unwrap();
    }

    /// What herdr-core asks you for to add a host, `hc machine add` asks
    /// in a terminal; a host that's just down isn't that.
    #[test]
    fn what_only_you_can_give_is_told_apart() {
        let host = Host::new("login01", "me@login01.example");
        let said = |kind: &str, message: &str| {
            let hint = "check that `ssh me@login01.example` logs in without a password prompt";
            anyhow::Error::new(RpcError(
                json!({"message": message, "data": {"kind": kind, "hint": hint}}),
            ))
            .context("adding login01 to herdr-core")
        };
        for kind in ["hc_missing", "incompatible", "host_key"] {
            assert!(asks_you(&said(kind, "no"), &host), "{kind}");
        }
        for down in [
            "no",
            "cannot reach me@login01.example: ssh: connect to host login01.example port 22: \
             Operation timed out",
        ] {
            assert!(!asks_you(&said("unreachable", down), &host), "{down}");
        }
        assert!(asks_you(
            &said("unreachable", "Permission denied (publickey,password)."),
            &host
        ));
        assert!(asks_you(
            &said("other", "login01 needs a login typed by a person"),
            &host
        ));
        // Its own words, as the machine by the host's name going elsewhere.
        assert!(!asks_you(&anyhow!("it reaches login02"), &host));
        assert!(needs_you(
            &anyhow::Error::new(NeedsYou("x".into())).context("refreshing")
        ));
        assert!(!needs_you(&anyhow!("x")));
    }

    #[test]
    fn hosts_get_names_herdr_core_takes() {
        assert_eq!(machine_name("agent"), "agent");
        assert_eq!(machine_name("me@Build.example.com"), "me-build.example.com");
        assert_eq!(machine_name("-dev box"), "dev-box");
        assert_eq!(machine_name("t7"), "host-t7");
        assert_eq!(machine_name("@@"), "host");
        assert_eq!(machine_name(&"x".repeat(40)).len(), 32);
        assert_eq!(machine_name("team"), "team", "a word, not an id");
    }

    /// A host is found under the name herdr-core takes for it; one by its
    /// name that runs another session than the config says isn't used.
    #[test]
    fn a_machine_by_the_hosts_name_must_match_it() {
        let server = HcServer::new("machine-match");
        let _far = server.far_config("match-box");
        let core = server.core();
        let host = Host::new("Match-Box", "match-box");
        assert_eq!(core.machine(&host).unwrap(), "match-box");

        let mut other = Host::new("match-box", "match-box");
        other.hc_session = Some("another".into());
        let e = core.machine(&other).unwrap_err();
        assert!(
            format!("{e:#}").contains("machine match-box reaches"),
            "{e:#}"
        );

        // Kept a while, so each refresh doesn't ask, or dial, again.
        let kept = core.machines.lock().unwrap()["match-box"].clone();
        assert!(matches!(&*kept.lock().unwrap(), Some(Err(_))));
        let again = core.machine(&other).unwrap_err();
        assert_eq!(format!("{again:#}"), format!("{e:#}"));
    }

    /// A `herdr-core connect` that fails the handshake is ended and reaped,
    /// not left a zombie: Lemur tries again every few seconds.
    #[test]
    fn a_failed_handshake_leaves_no_zombie() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("hc-zombie");
        for (name, answer) in [
            // Another protocol, then waiting, as a core would.
            (
                "other",
                r#"printf '{"jsonrpc":"2.0","id":0,"result":{"generation":999}}\n'; cat >/dev/null"#,
            ),
            // Gone at once, as when the core can't start.
            ("gone", "exit 1"),
        ] {
            let pid_file = dir.join(format!("{name}.pid"));
            let fake = dir.join(format!("herdr-core-{name}"));
            std::fs::write(
                &fake,
                format!("#!/bin/sh\necho $$ > '{}'\n{answer}\n", pid_file.display()),
            )
            .unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(Core::connect_with(fake, None).is_err(), "{name}");
            let pid = std::fs::read_to_string(&pid_file).unwrap();
            let stat = Command::new("ps")
                .args(["-o", "stat=", "-p", pid.trim()])
                .output()
                .unwrap();
            assert_eq!(
                String::from_utf8_lossy(&stat.stdout).trim(),
                "",
                "{name}: pid {pid}"
            );
        }
    }

    /// A machine removed while Lemur runs is looked for again, not asked
    /// for by the name it had until Lemur restarts.
    #[test]
    fn a_machine_removed_is_looked_for_again() {
        let server = HcServer::new("machine-rm");
        let cfg = server.far_config("rm-box");
        let core = server.core();
        assert_eq!(core.machine(&cfg.hosts[0]).unwrap(), "rm-box");
        assert!(!core.machines.lock().unwrap().is_empty());
        server.hc(&["machine", "rm", "rm-box"]);
        let e = core
            .exec(Some("rm-box"), &["true"], None, Duration::from_secs(5))
            .err()
            .expect("it's gone");
        assert_eq!(error_kind(&e), Some("machine_unknown"), "{e:#}");
        assert!(core.machines.lock().unwrap().is_empty(), "forgotten");
    }

    /// A paste bigger than a frame takes doesn't end the connection, and
    /// all of it arrives.
    #[test]
    fn a_huge_paste_goes_in_parts() {
        let server = HcServer::new("huge-paste");
        let dir = TempDir::new("huge-paste");
        let out = dir.join("out");
        let script = format!("stty -icanon; cat > '{}'", out.display());
        let t = server.run("/", &[], &["sh", "-c", &script]);
        let core = server.core();
        let attached = core.attach(&t, 80, 24, 0, json!({})).unwrap();
        let size = (17 << 20) + 5;
        let paste = vec![b'x'; size];
        core.input(attached.channel, &paste);
        core.flush().expect("the connection stands");
        assert!(!core.is_lost());
        let arrived = crate::test_support::wait_until(Duration::from_secs(60), || {
            std::fs::metadata(&out).is_ok_and(|m| m.len() >= size as u64)
        });
        assert!(arrived, "{:?}", std::fs::metadata(&out).map(|m| m.len()));
    }

    #[test]
    fn frames_round_trip() {
        let frame = encode(7, RESIZE, &[1, 2, 3, 4]);
        assert_eq!(frame.len(), HEADER + 4);
        assert_eq!(
            read_frame(&mut frame.as_slice()),
            Some((7, RESIZE, vec![1, 2, 3, 4]))
        );
        assert_eq!(read_frame(&mut &frame[..5]), None, "cut short");
    }

    #[test]
    fn the_event_stream_keeps_the_terminals_as_the_core_has_them() {
        let event = |seq: u64, kind: &str, t: &str, data: Value| {
            format!(
                "{}\n",
                json!({"method": "event", "params": {"seq": seq, "type": kind, "terminal": t, "data": data}})
            )
        };
        let rec = |a: &str| json!({"address": a, "state": "live", "labels": {}});
        let mut lines = EventLines::new(&json!({"seq": 5, "terminals": [rec("t1")]}));
        let addresses =
            |l: &EventLines| l.live().into_iter().map(|t| t.address).collect::<Vec<_>>();
        assert_eq!(addresses(&lines), ["t1"]);
        assert!(
            !lines.push(event(5, "title", "t1", json!({"title": "old"})).as_bytes()),
            "not newer than the snapshot"
        );
        // Split mid-line, as frames may be.
        let both = format!(
            "{}{}",
            event(6, "created", "t2", rec("t2")),
            event(7, "title", "t1", json!({"title": "hi"}))
        );
        let (a, b) = both.as_bytes().split_at(30);
        assert!(!lines.push(a));
        assert!(lines.push(b));
        assert_eq!(addresses(&lines), ["t1", "t2"]);
        assert_eq!(lines.live()[0].title, "hi");
        lines.push(event(8, "status", "t2", json!({"status": {"state": "working"}})).as_bytes());
        lines.push(event(9, "foreground", "t2", json!({"name": "vim", "cwd": "/src"})).as_bytes());
        assert_eq!(lines.live()[1].status.as_ref().unwrap().state, "working");
        // Its time moves with its word, as herdr-core's does, though the
        // event doesn't carry it; the same word again leaves it.
        let since = lines.live()[1].status_ms.expect("a time");
        assert!(since > 1_700_000_000_000, "{since}");
        lines.terminals[1]["status_ms"] = 1.into();
        lines.push(event(10, "status", "t2", json!({"status": {"state": "working"}})).as_bytes());
        assert_eq!(lines.live()[1].status_ms, Some(1));
        assert_eq!(lines.live()[1].folder(), "/src");
        lines.push(event(11, "finished", "t1", json!({"reason": "exited"})).as_bytes());
        assert_eq!(addresses(&lines), ["t2"], "finished: no longer live");
        lines.push(event(12, "removed", "t2", json!({})).as_bytes());
        assert!(addresses(&lines).is_empty());
        let missed = format!(
            "{}\n",
            json!({"method": "missed", "params": {"seq": 20, "terminals": [rec("t9")]}})
        );
        assert!(lines.push(missed.as_bytes()));
        assert_eq!(addresses(&lines), ["t9"]);
    }

    #[test]
    fn addresses_name_their_machine() {
        assert_eq!(machine_of("t7"), None);
        assert_eq!(machine_of("agent/t7"), Some("agent"));
    }

    /// A terminal started through the core shows up in its list, takes
    /// input, prints through an attached stream, and goes when killed.
    #[test]
    fn terminals_run_attach_and_end() {
        let server = HcServer::new("core");
        let core = server.core();
        let mut changes = core.changes();
        let labels = BTreeMap::from([("lemur.kind".to_string(), "shell".to_string())]);
        let argv: Vec<String> = [
            "sh",
            "-c",
            "printf 'ready\\n'; read x; printf 'got %s\\n' \"$x\"; sleep 30",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let address = core.run(None, "/", &argv, &labels).unwrap();
        assert!(
            wait_until(Duration::from_secs(5), || core
                .terminals()
                .iter()
                .any(|t| t.address == address && t.labels == labels)),
            "listed with its labels"
        );
        assert!(futures::executor::block_on(changes.next()).is_some());
        let mut attached = core.attach(&address, 40, 10, 0, json!({})).unwrap();
        let mut seen = Vec::new();
        let mut read_until = |needle: &str, seen: &mut Vec<u8>| {
            while !String::from_utf8_lossy(seen).contains(needle) {
                match futures::executor::block_on(attached.stream.next()) {
                    Some(Stream::Data(bytes)) => {
                        core.credit(attached.channel, bytes.len());
                        seen.extend(bytes);
                    }
                    other => panic!("stream ended: {other:?}"),
                }
            }
        };
        read_until("ready", &mut seen);
        core.input(attached.channel, b"abc\r");
        read_until("got abc", &mut seen);
        core.kill(&address).unwrap();
        assert!(
            wait_until(Duration::from_secs(5), || !core
                .terminals()
                .iter()
                .any(|t| t.address == address)),
            "gone once killed"
        );
    }

    #[test]
    fn commands_run_without_a_terminal() {
        let server = HcServer::new("exec");
        let core = server.core();
        let out = core
            .exec(
                None,
                &["sh", "-s"],
                Some(b"echo hi; echo oops >&2; exit 3"),
                Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(out.code, Some(3));
        assert_eq!(out.stdout, b"hi\n");
        assert_eq!(out.stderr, "oops\n");
    }
}

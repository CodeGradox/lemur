//! Linear, for its issues: Lemur signs in with its own Linear app (OAuth
//! with PKCE, so no secret ships in Lemur), keeps the sign-in in the macOS
//! Keychain where only Lemur can read it, and asks Linear's GraphQL API.
//! It only reads. Requests go through /usr/bin/curl, as the herdr-core
//! release check's do; tokens reach curl on its stdin, never on its
//! command line.

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use security_framework::os::macos::keychain::SecKeychain;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Lemur's Linear app.
pub const CLIENT_ID: &str = "acc866a714d5b3a9fa0e1545a60ae7f6";
/// Where Linear sends the browser back once you approve Lemur, as the app
/// has it: `http://localhost:38417/linear/callback`.
pub const PORT: u16 = 38417;
const CALLBACK: &str = "/linear/callback";
const AUTHORIZE: &str = "https://linear.app/oauth/authorize";
const API: &str = "https://api.linear.app";
/// How long a sign-in waits for you in the browser.
pub const SIGN_IN_TIME: Duration = Duration::from_secs(5 * 60);
/// The Keychain item that keeps the sign-in.
const SERVICE: &str = "Lemur Linear";
const ACCOUNT: &str = "oauth";
/// The Keychain has no such item.
const NOT_FOUND: i32 = -25300;

/// An issue's fields as lists show it; `FULL` adds its description.
const FIELDS: &str =
    "identifier title url branchName priorityLabel state { name color type } assignee { name }";
const FULL: &str = "identifier title url branchName priorityLabel description \
                    state { name color type } assignee { name }";

/// Lemur isn't signed in to Linear, or Linear no longer takes its sign-in.
#[derive(Debug)]
pub struct SignedOut;

impl std::fmt::Display for SignedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Lemur isn't signed in to Linear")
    }
}

impl std::error::Error for SignedOut {}

/// A sign-in called off before Linear answered.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Signing in was cancelled")
    }
}

impl std::error::Error for Cancelled {}

pub fn is_signed_out(e: &anyhow::Error) -> bool {
    e.downcast_ref::<SignedOut>().is_some()
}

pub fn is_cancelled(e: &anyhow::Error) -> bool {
    e.downcast_ref::<Cancelled>().is_some()
}

/// Linear as Lemur reaches it: its API, and where the sign-in is kept.
#[derive(Clone, Debug)]
pub struct Linear {
    api: String,
    store: Store,
    /// The sign-in, once read: the Keychain is asked once a launch, so a
    /// build macOS doesn't know yet asks you once. Held for a refresh, as
    /// each hands out a new refresh token and retires the one it used.
    kept: Arc<Mutex<Option<Option<Tokens>>>>,
}

/// Where the sign-in is kept.
#[derive(Clone, Debug)]
pub enum Store {
    /// In the macOS Keychain, the login keychain or the one at a path: an
    /// item only the Lemur that saved it reads without asking you, through
    /// Apple's API. (One the `security` command saves, any program reads
    /// through that command.)
    Keychain(Option<PathBuf>),
    /// Nowhere: never signed in. Tests that don't sign in have it.
    #[cfg(test)]
    Nowhere,
    /// In a file, for tests.
    #[cfg(test)]
    File(PathBuf),
}

/// What signing in got: a token for the API, valid until `expires` (Unix
/// seconds), and one that gets another.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Tokens {
    access: String,
    refresh: String,
    expires: u64,
}

/// Who Lemur is signed in as, and in which workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub name: String,
    pub workspace: String,
}

/// An issue, as the panel shows it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    /// Its key, `LAFT-3184`.
    pub identifier: String,
    pub title: String,
    pub url: String,
    /// The branch Linear suggests for it.
    pub branch_name: String,
    pub priority_label: String,
    /// Markdown; only asked for one issue at a time.
    #[serde(default)]
    pub description: Option<String>,
    pub state: State,
    pub assignee: Option<Person>,
}

/// Where an issue is in its team's workflow.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct State {
    pub name: String,
    /// `#rrggbb`.
    pub color: String,
    /// backlog, unstarted, started, completed, canceled or triage.
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Person {
    pub name: String,
}

/// A sign-in under way: listening for Linear's answer, with what proves
/// the answer is for it.
pub struct SignIn {
    callback: Callback,
    verifier: String,
    challenge: String,
    state: String,
}

impl SignIn {
    /// The page that asks you to approve Lemur, to open in the browser.
    pub fn url(&self) -> String {
        format!(
            "{AUTHORIZE}?client_id={CLIENT_ID}&redirect_uri={}&response_type=code&scope=read\
             &state={}&code_challenge={}&code_challenge_method=S256",
            encode(&self.callback.redirect_uri()),
            self.state,
            self.challenge
        )
    }
}

impl Default for Linear {
    fn default() -> Self {
        Linear::new()
    }
}

impl Linear {
    /// Linear itself, with the sign-in in the login keychain. Tests never
    /// sign in to it.
    pub fn new() -> Linear {
        Linear {
            api: API.into(),
            #[cfg(not(test))]
            store: Store::Keychain(None),
            #[cfg(test)]
            store: Store::Nowhere,
            kept: Default::default(),
        }
    }

    /// A stand-in for Linear at `api`, keeping its sign-in in `store`.
    #[cfg(test)]
    pub fn at(api: &str, store: Store) -> Linear {
        Linear {
            api: api.into(),
            store,
            kept: Default::default(),
        }
    }

    /// Keeps a sign-in with `access` as its token, good for an hour: a
    /// test's start.
    #[cfg(test)]
    pub fn sign_in_as(&self, access: &str) -> Result<()> {
        let tokens = Tokens {
            access: access.into(),
            refresh: "r".into(),
            expires: now() + 3600,
        };
        self.keep(&mut self.kept(), tokens)
    }

    /// Whether a sign-in is kept; Linear may not take it any more.
    pub fn signed_in(&self) -> Result<bool> {
        Ok(self.load(&mut self.kept())?.is_some())
    }

    fn kept(&self) -> MutexGuard<'_, Option<Option<Tokens>>> {
        self.kept.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The sign-in kept, read from the store the first time.
    fn load(&self, kept: &mut Option<Option<Tokens>>) -> Result<Option<Tokens>> {
        if kept.is_none() {
            *kept = Some(self.store.load()?);
        }
        Ok(kept.clone().flatten())
    }

    /// Keeps `tokens`: here first, so they're used even if the store fails,
    /// as the refresh token they replace is retired.
    fn keep(&self, kept: &mut Option<Option<Tokens>>, tokens: Tokens) -> Result<()> {
        *kept = Some(Some(tokens.clone()));
        self.store.save(&tokens)
    }

    /// Forgets the sign-in; forgotten here even if the store fails.
    fn forget(&self, kept: &mut Option<Option<Tokens>>) -> Result<()> {
        *kept = Some(None);
        self.store.clear()
    }

    /// Starts signing in: listens for Linear's answer, on `PORT`. Open
    /// `url()` in the browser, then `finish_sign_in`.
    pub fn start_sign_in(&self) -> Result<SignIn> {
        self.start_sign_in_on(PORT)
    }

    fn start_sign_in_on(&self, port: u16) -> Result<SignIn> {
        let callback = Callback::listen(port)?;
        let verifier = URL_SAFE_NO_PAD.encode(random_bytes::<32>()?);
        Ok(SignIn {
            challenge: challenge(&verifier)?,
            verifier,
            state: URL_SAFE_NO_PAD.encode(random_bytes::<16>()?),
            callback,
        })
    }

    /// Waits for you to approve Lemur in the browser, until `cancelled`,
    /// then keeps the sign-in and says who it's for.
    pub fn finish_sign_in(&self, sign_in: SignIn, cancelled: &AtomicBool) -> Result<Account> {
        let code = sign_in
            .callback
            .wait(&sign_in.state, cancelled, SIGN_IN_TIME)?;
        let tokens = self.token(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &sign_in.callback.redirect_uri()),
            ("client_id", CLIENT_ID),
            ("code_verifier", &sign_in.verifier),
        ])?;
        self.keep(&mut self.kept(), tokens)?;
        self.account()
    }

    /// Forgets the sign-in, and tells Linear so; forgotten even when
    /// Linear can't be reached.
    pub fn sign_out(&self) -> Result<()> {
        let mut kept = self.kept();
        let Some(tokens) = self.load(&mut kept)? else {
            return Ok(());
        };
        self.forget(&mut kept)?;
        let body = form(&[
            ("token", &tokens.refresh),
            ("token_type_hint", "refresh_token"),
        ]);
        curl(&self.endpoint("/oauth/revoke"), FORM, &body, None)?;
        Ok(())
    }

    /// Who Lemur is signed in as.
    pub fn account(&self) -> Result<Account> {
        let data = self.query("query { viewer { name } organization { name } }", json!({}))?;
        Ok(Account {
            name: text(&data["viewer"]["name"]),
            workspace: text(&data["organization"]["name"]),
        })
    }

    /// The issues assigned to you that aren't done or canceled, the ones
    /// changed last first.
    pub fn mine(&self) -> Result<Vec<Issue>> {
        let data = self.query(
            &format!(
                "query {{ viewer {{ assignedIssues(first: 50, orderBy: updatedAt, \
                 filter: {{ state: {{ type: {{ nin: [\"completed\", \"canceled\"] }} }} }}) \
                 {{ nodes {{ {FIELDS} }} }} }} }}"
            ),
            json!({}),
        )?;
        issues(&data["viewer"]["assignedIssues"]["nodes"])
    }

    /// Issues that match `term`, the best first; a key or a link to an
    /// issue finds that one.
    pub fn search(&self, term: &str) -> Result<Vec<Issue>> {
        if let Some(key) = key_of(term) {
            return match self.issue(&key) {
                Ok(issue) => Ok(vec![issue]),
                Err(e) if format!("{e:#}").to_lowercase().contains("not found") => Ok(vec![]),
                Err(e) => Err(e),
            };
        }
        let data = self.query(
            &format!(
                "query($term: String!) {{ searchIssues(term: $term, first: 20) \
                 {{ nodes {{ {FIELDS} }} }} }}"
            ),
            json!({ "term": term }),
        )?;
        issues(&data["searchIssues"]["nodes"])
    }

    /// The issue `key`, with its description.
    pub fn issue(&self, key: &str) -> Result<Issue> {
        let data = self.query(
            &format!("query($id: String!) {{ issue(id: $id) {{ {FULL} }} }}"),
            json!({ "id": key }),
        )?;
        serde_json::from_value(data["issue"].clone()).context("Linear's answer")
    }

    /// Asks the API, signed in; refreshes the sign-in once it's due, or
    /// once Linear turns it down.
    fn query(&self, query: &str, variables: Value) -> Result<Value> {
        let body = json!({ "query": query, "variables": variables }).to_string();
        let url = self.endpoint("/graphql");
        let access = self.access(None)?;
        let (mut status, mut out) = curl(&url, JSON, &body, Some(&access))?;
        if turned_down(status, &out) {
            let again = self.access(Some(&access))?;
            (status, out) = curl(&url, JSON, &body, Some(&again))?;
            if turned_down(status, &out) {
                self.forget(&mut self.kept())?;
                return Err(SignedOut.into());
            }
        }
        let answer: Value = serde_json::from_str(&out)
            .with_context(|| format!("Linear answered {status}: {}", brief(&out)))?;
        if let Some(e) = answer["errors"].get(0) {
            bail!("Linear: {}", text(&e["message"]));
        }
        if status != 200 {
            bail!("Linear answered {status}");
        }
        Ok(answer["data"].clone())
    }

    /// A token for the API: the one kept, or a fresh one when it's due or
    /// `stale` (one Linear turned down) is still kept. A refresh Linear
    /// refuses signs Lemur out.
    fn access(&self, stale: Option<&str>) -> Result<String> {
        let mut kept = self.kept();
        let mut tokens = self.load(&mut kept)?.ok_or(SignedOut)?;
        let due = |t: &Tokens| t.expires <= now() + 60 || stale == Some(t.access.as_str());
        if !due(&tokens) {
            return Ok(tokens.access);
        }
        // Another Lemur (the app, and one built here) may have refreshed it
        // since: its refresh token is the one Linear still takes.
        if let Some(stored) = self.stored_since(&tokens) {
            *kept = Some(Some(stored.clone()));
            if !due(&stored) {
                return Ok(stored.access);
            }
            tokens = stored;
        }
        match self.token(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &tokens.refresh),
            ("client_id", CLIENT_ID),
        ]) {
            Ok(fresh) => {
                let access = fresh.access.clone();
                self.keep(&mut kept, fresh)?;
                Ok(access)
            }
            Err(e) if e.downcast_ref::<Refused>().is_some() => {
                // Another Lemur may have just used the same refresh token,
                // and kept what it got for it, which forgetting would delete.
                if let Some(stored) = self.stored_since(&tokens) {
                    let access = stored.access.clone();
                    *kept = Some(Some(stored));
                    return Ok(access);
                }
                self.forget(&mut kept)?;
                Err(SignedOut.into())
            }
            Err(e) => Err(e),
        }
    }

    /// What the store holds, if it's newer than `tokens`. Older, it's what a
    /// save that failed left there, whose refresh token is retired. Not
    /// being let read it, what's here can still be refreshed.
    fn stored_since(&self, tokens: &Tokens) -> Option<Tokens> {
        let stored = self.store.load().ok().flatten()?;
        (stored.expires > tokens.expires).then_some(stored)
    }

    /// Trades `form` at the token endpoint for tokens.
    fn token(&self, fields: &[(&str, &str)]) -> Result<Tokens> {
        let (status, out) = curl(&self.endpoint("/oauth/token"), FORM, &form(fields), None)?;
        let answer: Value = serde_json::from_str(&out)
            .with_context(|| format!("Linear answered {status}: {}", brief(&out)))?;
        if status != 200 {
            let why = answer["error_description"]
                .as_str()
                .or(answer["error"].as_str())
                .unwrap_or("no reason given");
            // Linear turned it down, as a used or revoked refresh token, a
            // code already traded, or Lemur's own OAuth app no longer taken.
            // Anything else, as being asked too often, leaves the sign-in as
            // it is.
            if matches!(
                answer["error"].as_str(),
                Some("invalid_grant" | "invalid_client")
            ) {
                return Err(anyhow!(Refused).context(format!("Linear refused: {why}")));
            }
            bail!("Linear answered {status}: {why}");
        }
        let access = answer["access_token"].as_str().context("no access token")?;
        let refresh = answer["refresh_token"]
            .as_str()
            .context("no refresh token")?;
        Ok(Tokens {
            access: access.into(),
            refresh: refresh.into(),
            expires: now() + answer["expires_in"].as_u64().unwrap_or(86_399),
        })
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.api)
    }
}

/// Linear turned the token endpoint's request down.
#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("refused")
    }
}

impl std::error::Error for Refused {}

/// Whether the API turned the token down.
fn turned_down(status: u16, out: &str) -> bool {
    status == 401
        || serde_json::from_str::<Value>(out).is_ok_and(|v| {
            v["errors"].as_array().is_some_and(|errors| {
                errors
                    .iter()
                    .any(|e| e["extensions"]["code"] == "AUTHENTICATION_ERROR")
            })
        })
}

fn issues(nodes: &Value) -> Result<Vec<Issue>> {
    serde_json::from_value(nodes.clone()).context("Linear's answer")
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// The start of an answer that isn't JSON, for an error.
fn brief(out: &str) -> String {
    out.chars().take(200).collect()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

const JSON: &str = "application/json";
const FORM: &str = "application/x-www-form-urlencoded";

/// Posts `body` to `url`; Linear's HTTP status and answer. With `bearer`,
/// the token goes to curl on stdin and `body` on its command line (`body`
/// mustn't start with `@`); without, `body` goes on stdin, as it holds a
/// code or a token.
fn curl(url: &str, content_type: &str, body: &str, bearer: Option<&str>) -> Result<(u16, String)> {
    let mut command = Command::new("/usr/bin/curl");
    command.args([
        "-sS",
        "--max-time",
        "20",
        "-X",
        "POST",
        "-H",
        &format!("Content-Type: {content_type}"),
        "-w",
        "\n%{http_code}",
    ]);
    let input = match bearer {
        Some(token) => {
            command.args(["-H", "@-", "--data-binary", body]);
            format!("Authorization: Bearer {token}\n")
        }
        None => {
            command.args(["--data-binary", "@-"]);
            body.to_string()
        }
    };
    let mut child = command
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running curl")?;
    child
        .stdin
        .take()
        .context("curl's stdin")?
        .write_all(input.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "couldn't reach Linear: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let out = String::from_utf8_lossy(&out.stdout);
    let (answer, status) = out.rsplit_once('\n').context("curl's answer")?;
    Ok((
        status.trim().parse().context("curl's status")?,
        answer.to_string(),
    ))
}

impl Store {
    fn load(&self) -> Result<Option<Tokens>> {
        let kept = match self {
            Store::Keychain(path) => {
                match keychain(path)?.find_generic_password(SERVICE, ACCOUNT) {
                    Ok((password, _)) => password.to_vec(),
                    Err(e) if e.code() == NOT_FOUND => return Ok(None),
                    Err(e) => bail!("couldn't read Lemur's Linear sign-in from the Keychain: {e}"),
                }
            }
            #[cfg(test)]
            Store::Nowhere => return Ok(None),
            #[cfg(test)]
            Store::File(path) => match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.into()),
            },
        };
        Ok(Some(
            serde_json::from_slice(&kept).context("the kept Linear sign-in")?,
        ))
    }

    fn save(&self, tokens: &Tokens) -> Result<()> {
        let json = serde_json::to_vec(tokens)?;
        match self {
            Store::Keychain(path) => keychain(path)?
                .set_generic_password(SERVICE, ACCOUNT, &json)
                .map_err(|e| anyhow!("couldn't keep Lemur's Linear sign-in in the Keychain: {e}")),
            #[cfg(test)]
            Store::Nowhere => Ok(()),
            #[cfg(test)]
            Store::File(path) => Ok(std::fs::write(path, json)?),
        }
    }

    fn clear(&self) -> Result<()> {
        match self {
            Store::Keychain(path) => {
                let keychain = keychain(path)?;
                let gone = |keychain: &SecKeychain| match keychain
                    .find_generic_password(SERVICE, ACCOUNT)
                {
                    Ok((_, item)) => Ok(Some(item)),
                    Err(e) if e.code() == NOT_FOUND => Ok(None),
                    Err(e) => Err(anyhow!(
                        "couldn't remove Lemur's Linear sign-in from the Keychain: {e}"
                    )),
                };
                if let Some(item) = gone(&keychain)? {
                    item.delete();
                }
                // Deleting doesn't say whether it did.
                match gone(&keychain)? {
                    None => Ok(()),
                    Some(_) => bail!("couldn't remove Lemur's Linear sign-in from the Keychain"),
                }
            }
            #[cfg(test)]
            Store::Nowhere => Ok(()),
            #[cfg(test)]
            Store::File(path) => match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            },
        }
    }
}

/// The login keychain, or the one at `path`.
fn keychain(path: &Option<PathBuf>) -> Result<SecKeychain> {
    match path {
        None => SecKeychain::default(),
        Some(path) => SecKeychain::open(path),
    }
    .map_err(|e| anyhow!("couldn't open the Keychain: {e}"))
}

fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .context("reading /dev/urandom")?;
    Ok(bytes)
}

/// PKCE's challenge for `verifier`: its SHA-256, in base64url.
fn challenge(verifier: &str) -> Result<String> {
    let mut child = Command::new("/usr/bin/openssl")
        .args(["dgst", "-sha256", "-binary"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("running openssl")?;
    child
        .stdin
        .take()
        .context("openssl's stdin")?
        .write_all(verifier.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() || out.stdout.len() != 32 {
        bail!("openssl couldn't hash the sign-in's verifier");
    }
    Ok(URL_SAFE_NO_PAD.encode(out.stdout))
}

/// Listens on this Mac only, for the browser coming back from Linear.
struct Callback {
    listeners: Vec<TcpListener>,
    port: u16,
}

impl Callback {
    fn listen(port: u16) -> Result<Callback> {
        let v4 = TcpListener::bind(("127.0.0.1", port)).with_context(|| {
            format!("can't listen on port {port} for Linear's answer: is another sign-in waiting?")
        })?;
        let port = v4.local_addr()?.port();
        let mut listeners = vec![v4];
        // A browser may try localhost's IPv6 address first.
        if let Ok(v6) = TcpListener::bind(("::1", port)) {
            listeners.push(v6);
        }
        for l in &listeners {
            l.set_nonblocking(true)?;
        }
        Ok(Callback { listeners, port })
    }

    fn redirect_uri(&self) -> String {
        format!("http://localhost:{}{CALLBACK}", self.port)
    }

    /// Linear's code, once the browser brings it, for the sign-in `state`.
    fn wait(&self, state: &str, cancelled: &AtomicBool, timeout: Duration) -> Result<String> {
        let deadline = Instant::now() + timeout;
        loop {
            for l in &self.listeners {
                match l.accept() {
                    Ok((stream, _)) => {
                        if let Some(answer) = answer(stream, state) {
                            return answer;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.into()),
                }
            }
            if cancelled.load(Ordering::Relaxed) {
                return Err(Cancelled.into());
            }
            if Instant::now() >= deadline {
                bail!(
                    "Linear didn't answer within {} minutes",
                    timeout.as_secs() / 60
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Reads the browser's request and answers it: Linear's code, or why
/// there's none; nothing for a request that isn't Linear's answer, as a
/// favicon.
fn answer(mut stream: TcpStream, state: &str) -> Option<Result<String>> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut request = vec![];
    let mut buf = [0; 4096];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 16384 {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => request.extend_from_slice(&buf[..n]),
        }
    }
    let request = String::from_utf8_lossy(&request);
    let target = request.lines().next()?.split(' ').nth(1)?.to_string();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    if path != CALLBACK {
        respond(&mut stream, "404 Not Found", "Not found.");
        return None;
    }
    let params = parse_query(query);
    let got = match (params.get("error"), params.get("state"), params.get("code")) {
        (Some(error), ..) => Err(anyhow!(
            "Linear didn't sign Lemur in: {}",
            params.get("error_description").unwrap_or(error)
        )),
        (_, Some(s), _) if s != state => Err(anyhow!("that answer wasn't for this sign-in")),
        (_, Some(_), Some(code)) => Ok(code.clone()),
        _ => Err(anyhow!("Linear's answer had no code")),
    };
    match &got {
        Ok(_) => respond(
            &mut stream,
            "200 OK",
            "Lemur is signed in to Linear. You can close this tab.",
        ),
        Err(e) => respond(
            &mut stream,
            "400 Bad Request",
            &format!("Lemur couldn't sign in to Linear: {e}"),
        ),
    }
    Some(got)
}

fn respond(stream: &mut TcpStream, status: &str, message: &str) {
    let message = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let page = format!(
        "<!doctype html><meta charset=utf-8><title>Lemur</title>\
         <body style=\"font: 15px -apple-system, sans-serif; margin: 3em\">{message}</body>"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            Some((decode(k)?, decode(v)?))
        })
        .collect()
}

/// `s` with every byte but the unreserved ones percent-encoded.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn decode(s: &str) -> Option<String> {
    let mut bytes = vec![];
    let mut it = s.bytes();
    while let Some(b) = it.next() {
        match b {
            b'+' => bytes.push(b' '),
            b'%' => {
                let hex = [it.next()?, it.next()?];
                bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
            }
            _ => bytes.push(b),
        }
    }
    String::from_utf8(bytes).ok()
}

fn form(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// The issue a link to Linear points at: `LAFT-3184` in
/// `https://linear.app/laft/issue/LAFT-3184/fix-rounding`.
pub fn issue_in(text: &str) -> Option<String> {
    text.match_indices("linear.app/").find_map(|(at, found)| {
        let rest = &text[at + found.len()..];
        let (_, after) = rest.split_once('/')?;
        let after = after.strip_prefix("issue/")?;
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .unwrap_or(after.len());
        // The key, before the title's words.
        let mut parts = after[..end].splitn(3, '-');
        let key = format!("{}-{}", parts.next()?, parts.next()?);
        is_key(&key).then(|| key.to_uppercase())
    })
}

/// `term` as an issue's key: `laft-3184`, or a link to the issue.
pub fn key_of(term: &str) -> Option<String> {
    let term = term.trim();
    issue_in(term).or_else(|| is_key(term).then(|| term.to_uppercase()))
}

/// Whether `s` reads as an issue's key: a team's key, a dash, a number.
fn is_key(s: &str) -> bool {
    s.split_once('-').is_some_and(|(team, number)| {
        team.starts_with(|c: char| c.is_ascii_alphabetic())
            && team.chars().all(|c| c.is_ascii_alphanumeric())
            && !number.is_empty()
            && number.chars().all(|c| c.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeLinear, TempDir};

    fn issue_json(key: &str, title: &str) -> Value {
        json!({
            "identifier": key,
            "title": title,
            "url": format!("https://linear.app/laft/issue/{key}/x"),
            "branchName": format!("magnus/{}", key.to_lowercase()),
            "priorityLabel": "High",
            "state": { "name": "In Progress", "color": "#f2c94c", "type": "started" },
            "assignee": { "name": "Magnus" },
        })
    }

    #[test]
    fn links_and_keys_name_issues() {
        let link = "see https://linear.app/laft/issue/LAFT-3184/fix-lease-rounding.";
        assert_eq!(issue_in(link).as_deref(), Some("LAFT-3184"));
        assert_eq!(
            issue_in("(https://linear.app/laft/issue/ENG-7)").as_deref(),
            Some("ENG-7")
        );
        assert_eq!(issue_in("https://linear.app/laft/project/x-1"), None);
        assert_eq!(issue_in("https://example.com/laft/issue/LAFT-1"), None);
        assert_eq!(key_of(" laft-3184 ").as_deref(), Some("LAFT-3184"));
        assert_eq!(key_of(link).as_deref(), Some("LAFT-3184"));
        assert_eq!(key_of("lease rounding"), None);
        assert_eq!(key_of("LAFT-"), None);
        assert_eq!(key_of("3184-12"), None);
    }

    #[test]
    fn the_challenge_is_the_verifiers_sha256() {
        // SHA-256 of "abc", base64url.
        assert_eq!(
            challenge("abc").unwrap(),
            "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0"
        );
    }

    #[test]
    fn queries_and_forms_round_trip() {
        assert_eq!(
            encode("http://localhost:1/a b"),
            "http%3A%2F%2Flocalhost%3A1%2Fa%20b"
        );
        let q = parse_query("code=a%2Fb&state=x+y&error");
        assert_eq!(q["code"], "a/b");
        assert_eq!(q["state"], "x y");
        assert_eq!(q["error"], "");
        assert_eq!(form(&[("a", "1 2"), ("b", "&")]), "a=1%202&b=%26");
    }

    /// The browser comes back with a code for this sign-in; a stray
    /// request first is answered and waited past.
    #[test]
    fn a_sign_in_takes_the_code_the_browser_brings() {
        let api = FakeLinear::new(|r| {
            match r.path.as_str() {
            "/oauth/token" => (
                200,
                json!({ "access_token": "a1", "refresh_token": "r1", "expires_in": 86399 })
                    .to_string(),
            ),
            _ => (
                200,
                json!({ "data": { "viewer": { "name": "Magnus" }, "organization": { "name": "Laft" } } })
                    .to_string(),
            ),
        }
        });
        let dir = TempDir::new("linear-sign-in");
        let linear = Linear::at(&api.url, Store::File(dir.join("tokens")));
        let sign_in = linear.start_sign_in_on(0).unwrap();
        let url = sign_in.url();
        let port = sign_in.callback.port;
        assert!(
            url.starts_with("https://linear.app/oauth/authorize?client_id="),
            "{url}"
        );
        assert!(url.contains("&code_challenge_method=S256"), "{url}");
        assert!(
            url.contains(&format!(
                "redirect_uri=http%3A%2F%2Flocalhost%3A{port}%2Flinear%2Fcallback"
            )),
            "{url}"
        );
        let state = sign_in.state.clone();
        let browser = std::thread::spawn(move || {
            let get = |path: &str| {
                Command::new("/usr/bin/curl")
                    .args([
                        "-s",
                        "-w",
                        " %{http_code}",
                        &format!("http://127.0.0.1:{port}{path}"),
                    ])
                    .output()
                    .unwrap()
            };
            let stray = get("/favicon.ico");
            let back = get(&format!("{CALLBACK}?code=c%2F1&state={state}"));
            (
                String::from_utf8_lossy(&stray.stdout).into_owned(),
                String::from_utf8_lossy(&back.stdout).into_owned(),
            )
        });
        let account = linear
            .finish_sign_in(sign_in, &AtomicBool::new(false))
            .unwrap();
        let (stray, back) = browser.join().unwrap();
        assert!(stray.ends_with(" 404"), "{stray}");
        assert!(
            back.contains("signed in to Linear") && back.ends_with(" 200"),
            "{back}"
        );
        assert_eq!(
            account,
            Account {
                name: "Magnus".into(),
                workspace: "Laft".into()
            }
        );
        let asked = api.requests();
        assert_eq!(asked[0].path, "/oauth/token");
        let traded = parse_query(&asked[0].body);
        assert_eq!(traded["code"], "c/1");
        assert_eq!(traded["grant_type"], "authorization_code");
        assert_eq!(traded["client_id"], CLIENT_ID);
        assert_eq!(
            traded["redirect_uri"],
            format!("http://localhost:{port}{CALLBACK}")
        );
        assert_eq!(
            challenge(&traded["code_verifier"]).unwrap(),
            url_param(&url, "code_challenge")
        );
        assert_eq!(asked[1].authorization.as_deref(), Some("Bearer a1"));
        assert!(linear.signed_in().unwrap());
    }

    fn url_param(url: &str, name: &str) -> String {
        parse_query(url.split_once('?').unwrap().1)[name].clone()
    }

    /// An answer for another sign-in, or Linear saying no, fails it; so
    /// does cancelling it.
    #[test]
    fn a_sign_in_fails_on_a_wrong_state_or_a_cancel() {
        let linear = Linear::at("http://127.0.0.1:9", Store::Nowhere);
        let sign_in = linear.start_sign_in_on(0).unwrap();
        let port = sign_in.callback.port;
        let browser = std::thread::spawn(move || {
            Command::new("/usr/bin/curl")
                .args([
                    "-s",
                    &format!("http://127.0.0.1:{port}{CALLBACK}?code=x&state=other"),
                ])
                .output()
                .unwrap()
        });
        let e = linear
            .finish_sign_in(sign_in, &AtomicBool::new(false))
            .unwrap_err();
        browser.join().unwrap();
        assert!(e.to_string().contains("wasn't for this sign-in"), "{e}");

        let sign_in = linear.start_sign_in_on(0).unwrap();
        let e = linear
            .finish_sign_in(sign_in, &AtomicBool::new(true))
            .unwrap_err();
        assert!(is_cancelled(&e), "{e}");
    }

    /// A token due to expire is refreshed first, and the new pair kept;
    /// one Linear turns down is refreshed once; a refresh Linear refuses
    /// signs Lemur out.
    #[test]
    fn tokens_are_refreshed_and_kept() {
        let api = FakeLinear::new(|r| match (r.path.as_str(), r.authorization.as_deref()) {
            ("/oauth/token", _) if r.body.contains("refresh_token=r-dead") => {
                (400, json!({ "error": "invalid_grant" }).to_string())
            }
            ("/oauth/token", _) if r.body.contains("refresh_token=r-busy") => {
                (429, json!({ "error": "rate_limited" }).to_string())
            }
            (_, Some("Bearer a3")) => (
                200,
                json!({ "data": { "issue": issue_json("LAFT-1", "One") } }).to_string(),
            ),
            ("/oauth/token", _) => (
                200,
                json!({ "access_token": "a2", "refresh_token": "r2", "expires_in": 86399 })
                    .to_string(),
            ),
            (_, Some("Bearer a2")) => (
                200,
                json!({ "data": { "issue": issue_json("LAFT-1", "One") } }).to_string(),
            ),
            _ => (
                401,
                json!({ "errors": [{ "message": "authentication required" }] }).to_string(),
            ),
        });
        let dir = TempDir::new("linear-refresh");
        let store = Store::File(dir.join("tokens"));
        let linear = Linear::at(&api.url, store.clone());
        let keep = |access: &str, refresh: &str, expires: u64| {
            let tokens = Tokens {
                access: access.into(),
                refresh: refresh.into(),
                expires,
            };
            linear.keep(&mut linear.kept(), tokens).unwrap()
        };
        // Due: refreshed before asking.
        keep("a1", "r1", now() + 10);
        assert_eq!(linear.issue("LAFT-1").unwrap().title, "One");
        let paths: Vec<String> = api.requests().iter().map(|r| r.path.clone()).collect();
        assert_eq!(paths, ["/oauth/token", "/graphql"]);
        assert_eq!(store.load().unwrap().unwrap().refresh, "r2");

        // Turned down while not due: refreshed once, then asked again.
        keep("a-old", "r1", now() + 3600);
        assert_eq!(linear.issue("LAFT-1").unwrap().title, "One");
        assert_eq!(api.requests().len(), 5);

        // Another Lemur refreshed it meanwhile: its tokens are used, as
        // the refresh token here is retired.
        keep("a1", "r1", now() + 10);
        store
            .save(&Tokens {
                access: "a3".into(),
                refresh: "r3".into(),
                expires: now() + 3600,
            })
            .unwrap();
        let asked = api.requests().len();
        assert_eq!(linear.issue("LAFT-1").unwrap().title, "One");
        assert_eq!(api.requests().len(), asked + 1, "no refresh");

        // Asked too often: an error, but still signed in.
        keep("a-old", "r-busy", now() + 10);
        let e = linear.issue("LAFT-1").unwrap_err();
        assert!(!is_signed_out(&e), "{e}");
        assert!(linear.signed_in().unwrap());

        // Refused: signed out.
        keep("a-old", "r-dead", now() + 3600);
        let e = linear.issue("LAFT-1").unwrap_err();
        assert!(is_signed_out(&e), "{e}");
        assert!(!linear.signed_in().unwrap());
    }

    /// Tokens the store fails to keep are still used here: the refresh
    /// token they replaced is retired.
    #[test]
    fn fresh_tokens_are_used_though_the_store_fails() {
        let api = FakeLinear::new(|r| match (r.path.as_str(), r.authorization.as_deref()) {
            ("/oauth/token", _) => (
                200,
                json!({ "access_token": "a2", "refresh_token": "r2", "expires_in": 86399 })
                    .to_string(),
            ),
            (_, Some("Bearer a2")) => (
                200,
                json!({ "data": { "issue": issue_json("LAFT-1", "One") } }).to_string(),
            ),
            _ => (401, json!({ "errors": [{ "message": "no" }] }).to_string()),
        });
        let dir = TempDir::new("linear-store-fails");
        // No such folder: every save fails.
        let linear = Linear::at(&api.url, Store::File(dir.join("gone/tokens")));
        let due = Tokens {
            access: "a1".into(),
            refresh: "r1".into(),
            expires: now() + 10,
        };
        assert!(linear.keep(&mut linear.kept(), due).is_err());
        assert!(linear.issue("LAFT-1").is_err(), "the save failed");
        assert_eq!(linear.issue("LAFT-1").unwrap().title, "One");
        let refreshed = api
            .requests()
            .iter()
            .filter(|r| r.path == "/oauth/token")
            .count();
        assert_eq!(refreshed, 1, "r1 isn't used again");
    }

    fn tokens(access: &str, refresh: &str, expires: u64) -> Tokens {
        Tokens {
            access: access.into(),
            refresh: refresh.into(),
            expires,
        }
    }

    /// Tokens only kept here, as their save failed, stand against what the
    /// store still holds: its refresh token is retired, and Linear refusing
    /// it would sign Lemur out.
    #[test]
    fn tokens_kept_here_are_newer_than_a_failed_saves() {
        use std::os::unix::fs::PermissionsExt;
        let api = FakeLinear::new(|r| match r.path.as_str() {
            "/oauth/token" if r.body.contains("refresh_token=r2") => (
                200,
                json!({ "access_token": "a3", "refresh_token": "r3", "expires_in": 86399 })
                    .to_string(),
            ),
            "/oauth/token" => (400, json!({ "error": "invalid_grant" }).to_string()),
            _ if r.authorization.as_deref() == Some("Bearer a3") => (
                200,
                json!({ "data": { "issue": issue_json("LAFT-1", "One") } }).to_string(),
            ),
            _ => (401, json!({ "errors": [{ "message": "no" }] }).to_string()),
        });
        let dir = TempDir::new("linear-older");
        let file = dir.join("tokens");
        let store = Store::File(file.clone());
        store.save(&tokens("a1", "r1", now() - 100)).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o400)).unwrap();
        let linear = Linear::at(&api.url, store);
        let due = tokens("a2", "r2", now() + 10);
        assert!(
            linear.keep(&mut linear.kept(), due).is_err(),
            "the save fails"
        );
        // Refreshed with r2, though its own save fails again.
        assert!(linear.issue("LAFT-1").is_err());
        assert_eq!(linear.issue("LAFT-1").unwrap().title, "One");
        assert!(linear.signed_in().unwrap());
    }

    /// The store not letting itself be read leaves what's here to refresh.
    #[test]
    fn a_store_that_cant_be_read_doesnt_stop_a_refresh() {
        let api = FakeLinear::new(|r| match r.path.as_str() {
            "/oauth/token" => (
                200,
                json!({ "access_token": "a2", "refresh_token": "r2", "expires_in": 86399 })
                    .to_string(),
            ),
            _ => (
                200,
                json!({ "data": { "issue": issue_json("LAFT-1", "One") } }).to_string(),
            ),
        });
        let dir = TempDir::new("linear-unread");
        // A folder: neither read nor written.
        let linear = Linear::at(&api.url, Store::File(dir.to_path_buf()));
        let due = tokens("a1", "r1", now() + 10);
        assert!(linear.keep(&mut linear.kept(), due).is_err());
        assert!(linear.issue("LAFT-1").is_err(), "the save failed");
        assert_eq!(linear.issue("LAFT-1").unwrap().title, "One");
        let refreshed = api
            .requests()
            .iter()
            .filter(|r| r.path == "/oauth/token")
            .count();
        assert_eq!(refreshed, 1);
    }

    /// Two Lemurs refreshing at once: the one Linear refuses takes what the
    /// other kept, rather than deleting it and signing out.
    #[test]
    fn a_refresh_lost_to_another_lemur_takes_its_tokens() {
        let dir = TempDir::new("linear-race");
        let file = dir.join("tokens");
        let winner = Store::File(file.clone());
        let api = FakeLinear::new(move |r| match r.path.as_str() {
            "/oauth/token" => {
                // The other Lemur got there first.
                winner.save(&tokens("a2", "r2", now() + 86399)).unwrap();
                (400, json!({ "error": "invalid_grant" }).to_string())
            }
            _ if r.authorization.as_deref() == Some("Bearer a2") => (
                200,
                json!({ "data": { "issue": issue_json("LAFT-1", "One") } }).to_string(),
            ),
            _ => (401, json!({ "errors": [{ "message": "no" }] }).to_string()),
        });
        let store = Store::File(file.clone());
        store.save(&tokens("a1", "r1", now() + 10)).unwrap();
        let linear = Linear::at(&api.url, store);
        assert_eq!(linear.issue("LAFT-1").unwrap().title, "One");
        assert!(linear.signed_in().unwrap());
        assert!(file.exists(), "what the other kept stays");
    }

    /// Linear no longer taking Lemur's own OAuth app signs it out: every
    /// call would fail, with no way to sign in. Being asked too often doesn't.
    #[test]
    fn only_what_linear_turns_down_signs_out() {
        for (error, signed_in) in [
            ("invalid_client", false),
            ("invalid_grant", false),
            ("rate_limited", true),
        ] {
            let api = FakeLinear::new(move |r| match r.path.as_str() {
                "/oauth/token" => (401, json!({ "error": error }).to_string()),
                _ => (401, json!({ "errors": [{ "message": "no" }] }).to_string()),
            });
            let dir = TempDir::new("linear-client");
            let store = Store::File(dir.join("tokens"));
            store.save(&tokens("a1", "r1", now() + 10)).unwrap();
            let linear = Linear::at(&api.url, store);
            assert!(linear.issue("LAFT-1").is_err(), "{error}");
            assert_eq!(linear.signed_in().unwrap(), signed_in, "{error}");
        }
    }

    /// Search asks Linear's search, or for the issue when the term is its
    /// key; Linear's errors are said.
    #[test]
    fn issues_are_searched_and_listed() {
        let api = FakeLinear::new(|r| {
            let body: Value = serde_json::from_str(&r.body).unwrap();
            let query = body["query"].as_str().unwrap();
            if query.contains("searchIssues") {
                assert_eq!(body["variables"]["term"], "lease");
                (200, json!({ "data": { "searchIssues": { "nodes": [issue_json("LAFT-2", "Lease")] } } }).to_string())
            } else if query.contains("assignedIssues") {
                (200, json!({ "data": { "viewer": { "assignedIssues": { "nodes": [issue_json("LAFT-3", "Mine")] } } } }).to_string())
            } else if body["variables"]["id"] == "LAFT-404" {
                (
                    200,
                    json!({ "errors": [{ "message": "Entity not found: Issue" }], "data": null })
                        .to_string(),
                )
            } else if body["variables"]["id"] == "LAFT-500" {
                (
                    200,
                    json!({ "errors": [{ "message": "Rate limit exceeded" }], "data": null })
                        .to_string(),
                )
            } else {
                let mut issue = issue_json(body["variables"]["id"].as_str().unwrap(), "By key");
                issue["description"] = json!("Fix *it*");
                (200, json!({ "data": { "issue": issue } }).to_string())
            }
        });
        let dir = TempDir::new("linear-search");
        let store = Store::File(dir.join("tokens"));
        store
            .save(&Tokens {
                access: "a".into(),
                refresh: "r".into(),
                expires: now() + 3600,
            })
            .unwrap();
        let linear = Linear::at(&api.url, store);
        let found = linear.search("lease").unwrap();
        assert_eq!(found[0].identifier, "LAFT-2");
        // Read once: the Keychain asks a build it doesn't know yet once.
        std::fs::remove_file(dir.join("tokens")).unwrap();
        assert_eq!(found[0].state.name, "In Progress");
        assert_eq!(found[0].description, None);
        let by_key = linear
            .search("https://linear.app/laft/issue/LAFT-7/x")
            .unwrap();
        assert_eq!(by_key[0].identifier, "LAFT-7");
        assert_eq!(by_key[0].description.as_deref(), Some("Fix *it*"));
        assert!(linear.search("laft-404").unwrap().is_empty());
        let e = linear.search("LAFT-500").unwrap_err();
        assert_eq!(e.to_string(), "Linear: Rate limit exceeded");
        assert_eq!(linear.mine().unwrap()[0].title, "Mine");
        let asked = api.requests();
        assert!(
            asked
                .iter()
                .all(|r| r.authorization.as_deref() == Some("Bearer a"))
        );
    }

    /// The sign-in is kept in a keychain, updated in place, and removed;
    /// only the program that saved it reads it without asking, not the
    /// `security` command. A keychain of the test's own, never the login
    /// keychain.
    #[test]
    fn only_lemur_reads_the_sign_in_from_the_keychain() {
        let dir = TempDir::new("linear-keychain");
        let keychain = dir.join("test.keychain-db");
        let made = Command::new("/usr/bin/security")
            .args(["create-keychain", "-p", "test"])
            .arg(&keychain)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        let store = Store::Keychain(Some(keychain.clone()));
        let result = std::panic::catch_unwind(|| {
            assert_eq!(store.load().unwrap(), None);
            let tokens = |access: &str| Tokens {
                access: access.into(),
                refresh: "r".into(),
                expires: 1,
            };
            store.save(&tokens("a1")).unwrap();
            store.save(&tokens("a2")).unwrap();
            assert_eq!(store.load().unwrap(), Some(tokens("a2")));
            // Who may read it without asking: its access list, no secrets.
            let dumped = Command::new("/usr/bin/security")
                .args(["dump-keychain", "-a"])
                .arg(&keychain)
                .output()
                .unwrap();
            let acl = String::from_utf8_lossy(&dumped.stdout);
            let me = std::env::current_exe().unwrap();
            let readers: Vec<&str> = acl
                .lines()
                .map(str::trim)
                .filter(|l| l.starts_with("0: /"))
                .collect();
            assert_eq!(readers, [format!("0: {} (OK)", me.display())], "{acl}");
            assert!(!acl.contains("/usr/bin/security"), "{acl}");
            store.clear().unwrap();
            assert_eq!(store.load().unwrap(), None);
            store.clear().unwrap();
        });
        let _ = Command::new("/usr/bin/security")
            .arg("delete-keychain")
            .arg(&keychain)
            .output();
        result.unwrap();
    }
}

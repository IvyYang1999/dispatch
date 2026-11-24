//! Grok Bot: xAI and Cursor's always-on agents. The bots run on cloud computers; the desktop
//! app on this Mac only holds the signed-in session. Dispatch talks to the backend the app
//! itself talks to (`https://api2.cursor.sh/aiserver.v1.GrokBotService/<Method>`, JSON over
//! Connect), with the app's own sign-in:
//!
//! - `ListGrokBotAgents`: the bots (and the group chats, which are skipped);
//! - `SendGrokBotUserMessage`: a message to a bot, as if typed in the app;
//! - `GetGrokBotSendStatus`: whether the backend accepted it. That is the receipt.
//!
//! The sign-in is the access token the app keeps in `sand-secrets.json`, encrypted the way
//! Electron's safeStorage does it, with a password in the Keychain ("Grok Bot Safe Storage").
//! Reading that asks the person at the Mac once. The token stays in memory: it is never
//! logged, stored, put in an error, or sent anywhere but that backend.
//!
//! A bot's conversation is the user's private record and is never read: there is no call here
//! that fetches it, so "answered" is only ever known from the bot replying through Dispatch.
//!
//! None of this is a published interface. When an update of the app changes it, the calls are
//! refused; that case is told apart from a network failure or an expired sign-in, so the user
//! hears "Grok Bot was updated and Dispatch has to follow" and not just "unreachable".
//!
//! What the bot gets is a direct message from its user: the docs say it takes priority over
//! background work and may redirect the current turn, so the reach text says so.

use crate::i18n::t;
use crate::model::{home_dir, now_ms};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bot {
    pub id: String,
    pub name: String,
    pub description: String,
    pub is_group: bool,
    /// Its face in the Grok Bot app: a colour name and a shape name (`orange`, `blob`). A bot
    /// that never had one chosen has neither.
    pub avatar_color: Option<String>,
    pub avatar_shape: Option<String>,
}

/// One section of the Grok Bot app's sidebar: its name and the bots filed under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub bots: Vec<String>,
}

/// The desktop app, in /Applications or ~/Applications.
pub fn app_dir() -> Option<PathBuf> {
    [home_dir().join("Applications/Grok Bot.app"), PathBuf::from("/Applications/Grok Bot.app")].into_iter().find(|p| p.is_dir())
}

/// Where the app keeps its session (`sand-secrets.json`, `desktop-status.json`).
pub fn data_dir() -> PathBuf {
    home_dir().join("Library/Application Support/Grok Bot")
}

pub fn signed_in() -> bool {
    data_dir().join("sand-secrets.json").is_file()
}

/// The version of the app whose sign-in is used: what the running app wrote down, else what its
/// bundle says. The backend turns away a client that names a version it considers too old.
pub fn app_version() -> Option<String> {
    let valid = |v: &str| !v.is_empty() && v.len() <= 40 && v.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) && v.chars().all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c));
    let written = std::fs::read_to_string(data_dir().join("desktop-status.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["appVersion"].as_str().map(str::to_string));
    if let Some(v) = written.filter(|v| valid(v)) {
        return Some(v);
    }
    let plist = std::fs::read_to_string(app_dir()?.join("Contents/Info.plist")).ok()?;
    let at = plist.find("<key>CFBundleShortVersionString</key>")?;
    let rest = &plist[at..];
    let v = rest[rest.find("<string>")? + 8..rest.find("</string>")?].trim().to_string();
    valid(&v).then_some(v)
}

/// Why a call did not work, each told to the user in its own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// No sign-in to read: the app is not signed in, or keeps it in a way this build cannot read.
    SignIn(String),
    /// The Keychain did not hand over the password (nobody allowed it, or nobody answered).
    Keychain(String),
    /// The backend could not be reached at all.
    Network(String),
    /// The backend answered no: the HTTP status and what it said.
    Refused { status: u16, message: String },
}

impl Fault {
    /// The sign-in was turned away: the app has to be opened so it signs in again.
    pub fn expired(&self) -> bool {
        matches!(self, Fault::Refused { status: 401 | 403, .. })
    }

    /// Whether this looks like the interface having changed under us: refused for something
    /// other than the sign-in, by an app version that is not the one the calls last worked with.
    fn changed(&self, version: Option<&str>, worked_with: Option<&str>) -> bool {
        matches!(self, Fault::Refused { .. }) && !self.expired() && version.is_some() && worked_with.is_some() && version != worked_with
    }

    /// In the user's words. `version` is the app's version now, `worked_with` the one the calls
    /// last worked with.
    pub fn explain(&self, version: Option<&str>, worked_with: Option<&str>) -> String {
        match self {
            Fault::SignIn(why) => t("grok.signin").replace("{why}", why),
            Fault::Keychain(why) => t("grok.keychain").replace("{why}", why),
            Fault::Network(why) => t("grok.network").replace("{why}", why),
            f if f.expired() => t("grok.expired").into(),
            f if f.changed(version, worked_with) => t("grok.changed").replace("{v}", version.unwrap_or("?")),
            Fault::Refused { status, message } => t("grok.refused").replace("{status}", &status.to_string()).replace("{message}", message),
        }
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.explain(app_version().as_deref(), worked_with().as_deref()))
    }
}

impl std::error::Error for Fault {}

/// The app version the calls last worked with.
fn worked_with() -> Option<String> {
    crate::settings::get("grok_worked_with").and_then(|v| v.as_str().map(str::to_string))
}

fn note_worked(version: Option<&str>) {
    if let Some(v) = version {
        if worked_with().as_deref() != Some(v) {
            let _ = crate::settings::set("grok_worked_with", json!(v));
        }
    }
}

// ---- the app's sign-in ----

/// The sign-in to call the backend with. Kept in memory only.
struct Login {
    token: String,
    team: Option<String>,
}

/// The active account's encrypted access token and team, as the app stores them (base64).
fn stored_account(secrets: &str) -> Result<(String, Option<String>), Fault> {
    let v: Value = serde_json::from_str(secrets).map_err(|_| Fault::SignIn("sand-secrets.json is not JSON".into()))?;
    if !(v["version"].is_null() || v["version"] == 1) {
        return Err(Fault::SignIn(format!("account storage version {}", v["version"])));
    }
    // the accounts are a JSON text inside the JSON; only the one the user has selected is read
    let accounts: Value = v["cursor-accounts"].as_str().and_then(|a| serde_json::from_str(a).ok()).unwrap_or(Value::Null);
    let active = accounts["active"].as_str().ok_or_else(|| Fault::SignIn("no active account".into()))?;
    let account = &accounts["accounts"][active];
    let token = account["cursor-access-token"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| Fault::SignIn("the active account has no access token".into()))?;
    Ok((token.to_string(), account["cursor-selected-team-id"].as_str().map(str::to_string)))
}

/// The key Electron's safeStorage derives from the Keychain password on macOS.
fn storage_key(password: &str) -> [u8; 16] {
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password.as_bytes(), b"saltysalt", 1003, &mut key);
    key
}

/// One safeStorage value (base64 of `v10` + AES-128-CBC, the IV sixteen spaces) as text.
fn decrypt(encrypted_base64: &str, key: &[u8; 16]) -> Result<String, Fault> {
    use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(encrypted_base64.trim()).map_err(|_| Fault::SignIn("stored value is not base64".into()))?;
    if bytes.len() <= 3 || &bytes[..3] != b"v10" {
        return Err(Fault::SignIn("stored value is in a format this build cannot read".into()));
    }
    let plain = cbc::Decryptor::<aes::Aes128>::new(key.into(), &[b' '; 16].into()).decrypt_padded_vec_mut::<Pkcs7>(&bytes[3..]).map_err(|_| Fault::Keychain("the Keychain password does not open the stored sign-in".into()))?;
    String::from_utf8(plain).map(|s| s.trim().to_string()).map_err(|_| Fault::Keychain("the Keychain password does not open the stored sign-in".into()))
}

/// The password the app's storage is locked with. macOS asks the person at the Mac whether
/// `security` may read it; with nobody there the question times out.
#[cfg(target_os = "macos")]
fn keychain_password() -> Result<String, Fault> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new("/usr/bin/security").args(["find-generic-password", "-w", "-s", "Grok Bot Safe Storage"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| Fault::Keychain(e.to_string()))?;
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        match child.try_wait() {
            Ok(Some(exit)) => {
                let mut out = String::new();
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_string(&mut out);
                }
                let password = out.trim_end_matches(['\n', '\r']).to_string();
                return if exit.success() && !password.is_empty() { Ok(password) } else { Err(Fault::Keychain(t("grok.keychain_denied").into())) };
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Fault::Keychain(t("grok.keychain_waiting").into()));
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn keychain_password() -> Result<String, Fault> {
    Err(Fault::SignIn(t("grok.mac_only").into()))
}

// Metadata only: when the app signs in again or refreshes its token the file changes.
fn auth_stamp() -> Option<(std::time::SystemTime, u64)> {
    let m = std::fs::metadata(data_dir().join("sand-secrets.json")).ok()?;
    Some((m.modified().ok()?, m.len()))
}

type Stamp = Option<(std::time::SystemTime, u64)>;
static LOGIN: Mutex<Option<(Stamp, Arc<Login>)>> = Mutex::new(None);
/// The storage key, kept so the Keychain is asked once a run and not on every refreshed token.
static KEY: Mutex<Option<[u8; 16]>> = Mutex::new(None);
/// When the Keychain last said no (or nobody answered), and what it said: a person who turned
/// the question down is not asked again every minute.
static TURNED_DOWN: Mutex<Option<(Instant, Fault)>> = Mutex::new(None);
const ASK_AGAIN_AFTER: Duration = Duration::from_secs(15 * 60);

fn login() -> Result<Arc<Login>, Fault> {
    let stamp = auth_stamp();
    if let Some((at, login)) = LOGIN.lock().unwrap().as_ref() {
        if *at == stamp && stamp.is_some() {
            return Ok(login.clone());
        }
    }
    let secrets = std::fs::read_to_string(data_dir().join("sand-secrets.json")).map_err(|_| Fault::SignIn(t("grok.not_signed_in").into()))?;
    let (token, team) = stored_account(&secrets)?;
    let open = |key: &[u8; 16]| -> Result<Login, Fault> {
        let team = match &team {
            Some(t) => Some(decrypt(t, key)?).filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_digit())),
            None => None,
        };
        Ok(Login { token: decrypt(&token, key)?, team })
    };
    let remembered = *KEY.lock().unwrap();
    let login = match remembered.map(|k| open(&k)) {
        Some(Ok(l)) => l,
        // no key yet, or the app's storage was locked anew: ask the Keychain
        _ => {
            if let Some((at, fault)) = TURNED_DOWN.lock().unwrap().as_ref() {
                if at.elapsed() < ASK_AGAIN_AFTER {
                    return Err(fault.clone());
                }
            }
            let password = keychain_password().inspect_err(|f| *TURNED_DOWN.lock().unwrap() = Some((Instant::now(), f.clone())))?;
            *TURNED_DOWN.lock().unwrap() = None;
            let key = storage_key(&password);
            let l = open(&key)?;
            *KEY.lock().unwrap() = Some(key);
            l
        }
    };
    if login.token.is_empty() {
        return Err(Fault::SignIn("the stored access token is empty".into()));
    }
    let login = Arc::new(login);
    *LOGIN.lock().unwrap() = Some((stamp, login.clone()));
    Ok(login)
}

// ---- the backend ----

const BACKEND: &str = "https://api2.cursor.sh";

/// What this process sends as its platform, in the words the app uses.
fn client_os() -> &'static str {
    if cfg!(target_os = "macos") { "darwin" } else if cfg!(windows) { "win32" } else { "linux" }
}

/// One call: POST `<base>/aiserver.v1.GrokBotService/<method>` with a JSON body. The request
/// runs on a thread of its own, so it may be made from anywhere (the blocking client must not
/// run on an async runtime's thread).
fn call_at(base: &str, login: &Login, version: &str, method: &str, body: &Value) -> Result<Value, Fault> {
    let url = format!("{}/aiserver.v1.GrokBotService/{method}", base.trim_end_matches('/'));
    let (token, team, version, body) = (login.token.clone(), login.team.clone(), version.to_string(), body.clone());
    let local = url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost");
    let scrub = move |text: String, token: &str| if token.is_empty() { text } else { text.replace(token, "<token>") };
    std::thread::spawn(move || -> Result<Value, Fault> {
        // rustls is built without a provider of its own in this tree: name ring, once
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut builder = reqwest::blocking::Client::builder().timeout(Duration::from_secs(30)).connect_timeout(Duration::from_secs(12));
        if local {
            builder = builder.no_proxy();
        }
        let client = builder.build().map_err(|e| Fault::Network(scrub(e.to_string(), &token)))?;
        let mut req = client
            .post(&url)
            .header("content-type", "application/json")
            .header("connect-protocol-version", "1")
            .header("authorization", format!("Bearer {token}"))
            .header("x-cursor-client-type", "sand")
            .header("x-cursor-client-version", &version)
            .header("x-sand-box-namespace", "prod")
            .header("x-cursor-client-os", client_os())
            .header("x-cursor-client-source", "sand-desktop")
            .header("x-request-id", uuid::Uuid::new_v4().to_string());
        if let Some(team) = &team {
            req = req.header("x-cursor-team-id", team);
        }
        let res = req.body(body.to_string()).send().map_err(|e| Fault::Network(scrub(e.without_url().to_string(), &token)))?;
        let status = res.status().as_u16();
        let text = res.text().map_err(|e| Fault::Network(scrub(e.without_url().to_string(), &token)))?;
        if !(200..300).contains(&status) {
            // what the backend says about its refusal, in a line: its `message` when it gave one
            let said = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["message"].as_str().map(str::to_string)).unwrap_or(text);
            return Err(Fault::Refused { status, message: scrub(said.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect(), &token) });
        }
        serde_json::from_str(&text).map_err(|_| Fault::Refused { status, message: "the answer was not JSON".into() })
    })
    .join()
    .unwrap_or_else(|_| Err(Fault::Network("the request could not be made".into())))
}

fn call(method: &str, body: Value) -> Result<Value, Fault> {
    let login = login()?;
    let version = app_version();
    let out = call_at(BACKEND, &login, version.as_deref().unwrap_or("1.0.0"), method, &body);
    if out.is_ok() {
        note_worked(version.as_deref());
    }
    out
}

/// What this Mac's Grok Bot can do right now. Listing the bots is a call to the backend, so
/// it is remembered for a minute; `state_now` always asks again.
#[derive(Debug, Clone)]
pub struct State {
    pub app: bool,
    pub signed_in: bool,
    pub bots: Vec<Bot>,
    pub error: Option<String>,
    /// The app version that the calls stopped working with, when that is what `error` is about:
    /// the app was updated and Dispatch has to follow.
    pub changed: Option<String>,
    pub at: i64,
}

impl State {
    /// Everything in place to deliver: app, sign-in, and the backend answered.
    pub fn ready(&self) -> bool {
        self.app && self.signed_in && self.error.is_none()
    }

    pub fn bot(&self, id_or_name: &str) -> Option<&Bot> {
        self.bots.iter().find(|b| b.id.eq_ignore_ascii_case(id_or_name)).or_else(|| self.bots.iter().find(|b| b.name.eq_ignore_ascii_case(id_or_name)))
    }
}

#[derive(Clone)]
struct CachedState {
    state: State,
    auth: Option<(std::time::SystemTime, u64)>,
}

static STATE: crate::snapshot::Snapshot<CachedState> = crate::snapshot::Snapshot::new();
const STATE_TTL: Duration = Duration::from_secs(60);

fn refresh(previous: Option<CachedState>) -> CachedState {
    let before = auth_stamp();
    let state = state_now();
    let after = auth_stamp();
    retain_bots(CachedState { state, auth: after.filter(|_| before == after) }, previous)
}

fn retain_bots(mut next: CachedState, previous: Option<CachedState>) -> CachedState {
    if next.state.app && next.state.signed_in && next.state.error.is_some() && next.auth.is_some() {
        if let Some(old) = previous.filter(|p| p.auth == next.auth) {
            next.state.bots = old.state.bots;
        }
    }
    next
}

pub fn state() -> State {
    STATE.get(state_ttl(), refresh).state
}

pub(crate) fn state_snapshot() -> Option<State> {
    STATE.read(state_ttl(), refresh).filter(|s| s.auth == auth_stamp()).map(|s| s.state)
}

fn state_ttl() -> Duration {
    if STATE.peek().map(|s| s.auth != auth_stamp()).unwrap_or(false) { Duration::ZERO } else { STATE_TTL }
}

/// The app version the calls stopped working with, when the last look at the bots found the
/// interface changed. From memory: this never asks the backend.
pub fn changed_version() -> Option<String> {
    STATE.peek().and_then(|s| s.state.changed)
}

pub fn state_now() -> State {
    let mut st = State { app: app_dir().is_some(), signed_in: signed_in(), bots: vec![], error: None, changed: None, at: now_ms() };
    if !st.app || !st.signed_in {
        return st;
    }
    match call("ListGrokBotAgents", json!({})) {
        Ok(v) => st.bots = parse_agents(&v),
        Err(f) => {
            let (version, worked) = (app_version(), worked_with());
            st.changed = version.clone().filter(|_| f.changed(version.as_deref(), worked.as_deref()));
            st.error = Some(f.explain(version.as_deref(), worked.as_deref()));
        }
    }
    st
}

/// The bots and group chats in what `ListGrokBotAgents` answers.
pub fn parse_agents(v: &Value) -> Vec<Bot> {
    let text = |a: &Value, k: &str| a[k].as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    v["agents"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    let id = text(a, "agentId").or_else(|| text(a, "legacyAgentId"))?.to_lowercase();
                    Some(Bot {
                        name: text(a, "name").unwrap_or_else(|| id.clone()),
                        description: text(a, "description").unwrap_or_default(),
                        // a group chat is a room with members: nothing to write to as one session
                        is_group: a["kind"].as_str().map(|k| k.ends_with("_ROOM")).unwrap_or(false) || a["memberAgentIds"].as_array().map(|m| !m.is_empty()).unwrap_or(false),
                        avatar_color: text(a, "avatarColor"),
                        avatar_shape: text(a, "avatarShape"),
                        id,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// How the sidebar sections of the Grok Bot app are used to file its bots: `start` (the
/// default: they start out filed as in Grok Bot, and what the user moves stays moved), `follow`
/// (always as in Grok Bot) or `off` (the sections are not read at all).
pub fn sections_mode() -> &'static str {
    match crate::settings::get("grok_sections").as_ref().and_then(|v| v.as_str()) {
        Some("follow") => "follow",
        Some("off") => "off",
        _ => "start",
    }
}

pub fn set_sections_mode(mode: &str) -> Result<&'static str> {
    let mode = match mode {
        "follow" => "follow",
        "off" => "off",
        "start" => "start",
        other => return Err(anyhow!("unknown mode: {other}")),
    };
    crate::settings::set("grok_sections", Value::String(mode.into()))?;
    *SECTIONS.lock().unwrap() = None;
    Ok(mode)
}

/// RFC 4648 Base32 without padding, either case: how the app names its cache files.
fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let (mut bits, mut have, mut out) = (0u32, 0u32, vec![]);
    for c in text.bytes() {
        let v = match c.to_ascii_lowercase() {
            c @ b'a'..=b'z' => c - b'a',
            c @ b'2'..=b'7' => c - b'2' + 26,
            b'=' => continue,
            _ => return None,
        };
        bits = (bits << 5) | v as u32;
        have += 5;
        if have >= 8 {
            have -= 8;
            out.push((bits >> have) as u8);
            bits &= (1 << have) - 1;
        }
    }
    Some(out)
}

/// The sections in the app's cached sidebar: wherever in the JSON there is a list of things
/// with a name and a list of agent ids. The cache is the app's own and its layout may change, so
/// nothing more is assumed; anything else reads as "no sections".
pub fn sections_in(text: &str) -> Vec<Section> {
    fn find(v: &Value) -> Option<Vec<Section>> {
        if let Some(list) = v.as_array() {
            let found: Vec<Section> = list
                .iter()
                .filter_map(|x| {
                    let bots: Vec<String> = x["agentIds"].as_array()?.iter().filter_map(|i| i.as_str().map(str::to_string)).collect();
                    Some(Section { name: x["name"].as_str()?.trim().to_string(), bots })
                })
                .collect();
            if !found.is_empty() {
                return Some(found);
            }
            return list.iter().find_map(find);
        }
        v.as_object()?.values().find_map(find)
    }
    serde_json::from_str::<Value>(text).ok().as_ref().and_then(find).unwrap_or_default()
}

/// The sidebar sections of the Grok Bot app on this machine, from the cache it keeps of them
/// (the file whose name, read back from Base32, ends in `.sidebar.last-sections`). Read-only,
/// and only this one file is opened: nothing that holds the session.
fn read_sections() -> Vec<Section> {
    let Ok(rd) = std::fs::read_dir(data_dir().join("sand-client-persistence")) else { return vec![] };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(stem) = name.strip_suffix(".blob") else { continue };
        let key = base32_decode(stem).and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
        if !key.ends_with(".sidebar.last-sections") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(e.path()) {
            let found = sections_in(&text);
            if !found.is_empty() {
                return found;
            }
        }
    }
    vec![]
}

static SECTIONS: Mutex<Option<(Instant, Vec<Section>)>> = Mutex::new(None);

/// The sections, read at most once a minute; none when the user turned them off.
pub fn sections() -> Vec<Section> {
    if sections_mode() == "off" {
        return vec![];
    }
    let mut kept = SECTIONS.lock().unwrap();
    if let Some((at, list)) = kept.as_ref() {
        if at.elapsed() < Duration::from_secs(60) {
            return list.clone();
        }
    }
    let list = read_sections();
    *kept = Some((Instant::now(), list.clone()));
    list
}

/// The named section a bot is filed under in the app, if any.
pub fn section_of(sections: &[Section], bot_id: &str) -> Option<String> {
    sections.iter().find(|s| !s.name.is_empty() && s.bots.iter().any(|b| b.eq_ignore_ascii_case(bot_id))).map(|s| s.name.clone())
}

/// A bot's face as the app draws it (`{ bot_shape, bot_color }`), when it has one. From the
/// list already in memory: this never asks the gateway.
pub fn look(bot_id: &str) -> Option<Value> {
    let st = STATE.peek().filter(|s| s.auth == auth_stamp())?;
    let b = st.state.bots.iter().find(|b| b.id.eq_ignore_ascii_case(bot_id))?;
    let shape = b.avatar_shape.clone().filter(|s| !s.is_empty())?;
    Some(serde_json::json!({ "bot_shape": shape, "bot_color": b.avatar_color.clone().filter(|c| !c.is_empty()) }))
}

pub fn list_bots() -> Result<Vec<Bot>> {
    Ok(parse_agents(&call("ListGrokBotAgents", json!({}))?))
}

#[derive(Debug, Clone)]
pub struct SendOutcome {
    /// The id this message was sent under: what `send_status` is asked about.
    pub message_id: String,
    /// The backend said it accepted the message into the bot's conversation; false when it had
    /// taken the message but not said so yet.
    pub accepted: bool,
}

/// Whether the backend took a message: `{"dispatched": true, …}`. Anything else is a no, with
/// what it said about the delivery.
fn dispatched(v: &Value) -> Result<(), Fault> {
    if v["dispatched"] == true {
        return Ok(());
    }
    let why = v["delivery"].as_str().or_else(|| v["mode"].as_str()).unwrap_or("not dispatched");
    Err(Fault::Refused { status: 200, message: why.chars().take(120).collect() })
}

/// `accepted`, `pending` or whatever else the backend calls it, lowercased without its prefix.
fn status_word(v: &Value) -> String {
    v["status"].as_str().unwrap_or("").trim_start_matches("GROK_BOT_SEND_STATUS_").to_lowercase()
}

/// Where a sent message stands at the backend: `accepted`, `pending`, or its own word.
pub fn send_status(bot_id: &str, message_id: &str) -> Result<String, Fault> {
    Ok(status_word(&call("GetGrokBotSendStatus", json!({ "agentId": bot_id, "messageId": message_id }))?))
}

/// Send a message to a bot (by its id), as a direct message from its user, and wait a few
/// seconds for the backend to say it accepted it.
pub fn send(bot_id: &str, text: &str) -> Result<SendOutcome, Fault> {
    let message_id = uuid::Uuid::new_v4().to_string();
    let answer = call("SendGrokBotUserMessage", json!({ "agentId": bot_id, "messageId": message_id, "text": text, "sentAtMs": now_ms().to_string() }))?;
    dispatched(&answer)?;
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut accepted = false;
    while !accepted && Instant::now() < deadline {
        match send_status(bot_id, &message_id) {
            Ok(s) if s == "accepted" => accepted = true,
            Ok(s) if s == "pending" || s.is_empty() => std::thread::sleep(Duration::from_millis(1500)),
            // a word we do not know, or the question failed: the message itself was taken
            _ => break,
        }
    }
    Ok(SendOutcome { message_id, accepted })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn a_failed_refresh_keeps_bots_only_for_the_same_login_and_keeps_the_error() {
        let stamp = (std::time::UNIX_EPOCH, 1);
        let bot = Bot { id: "fixture".into(), name: "Fixture".into(), description: String::new(), is_group: false, avatar_color: None, avatar_shape: None };
        let previous = CachedState { auth: Some(stamp), state: State { app: true, signed_in: true, bots: vec![bot.clone()], error: None, changed: None, at: 0 } };
        let failed = CachedState { auth: Some(stamp), state: State { bots: vec![], error: Some("test timeout".into()), ..previous.state.clone() } };
        let retained = retain_bots(failed.clone(), Some(previous.clone()));
        assert_eq!(retained.state.bots, vec![bot]);
        assert_eq!(retained.state.error.as_deref(), Some("test timeout"));
        assert!(!retained.state.ready());
        let mut changed = failed.clone();
        changed.auth = Some((std::time::UNIX_EPOCH, 2));
        assert!(retain_bots(changed, Some(previous.clone())).state.bots.is_empty());
        let mut logged_out = failed.clone();
        logged_out.state.signed_in = false;
        logged_out.auth = None;
        assert!(retain_bots(logged_out, Some(previous.clone())).state.bots.is_empty());
        let empty_success = CachedState { state: State { error: None, ..failed.state }, ..failed };
        assert!(retain_bots(empty_success, Some(previous)).state.bots.is_empty(), "an authoritative empty list replaces old bots");
    }

    // "fixture.access.token" locked with the password "fixture-password", as the app would store it
    const STORED: &str = "djEwqfuFc538onB1TrK9hOr9NPlzE9cCrEQTp0v7jZRWXzM=";

    #[test]
    fn the_apps_stored_sign_in_opens_with_the_keychain_password() {
        let key = storage_key("fixture-password");
        assert_eq!(decrypt(STORED, &key).unwrap(), "fixture.access.token");
        // the wrong password is the Keychain's fault, an unknown format the storage's
        assert!(matches!(decrypt(STORED, &storage_key("another")), Err(Fault::Keychain(_))));
        assert!(matches!(decrypt("djExAAAA", &key), Err(Fault::SignIn(_))));
        assert!(matches!(decrypt("not base64 !", &key), Err(Fault::SignIn(_))));
    }

    #[test]
    fn only_the_selected_account_is_read() {
        let accounts = json!({ "active": "b", "accounts": { "a": { "cursor-access-token": "AAAA" }, "b": { "cursor-access-token": STORED, "cursor-refresh-token": "x", "cursor-selected-team-id": "TEAM" } } }).to_string();
        let secrets = json!({ "version": 1, "cursor-accounts": accounts }).to_string();
        assert_eq!(stored_account(&secrets).unwrap(), (STORED.to_string(), Some("TEAM".to_string())));
        let none = json!({ "version": 1, "cursor-accounts": json!({ "accounts": {} }).to_string() }).to_string();
        assert!(matches!(stored_account(&none), Err(Fault::SignIn(_))));
        assert!(matches!(stored_account(&json!({ "version": 7 }).to_string()), Err(Fault::SignIn(_))));
    }

    #[test]
    fn bots_and_group_chats_are_told_apart() {
        let v = json!({ "agents": [
            { "id": "abc1234", "agentId": "524FE60F-0010-4E9F-BFC1-1936EA44B486", "legacyAgentId": "524fe60f-0010-4e9f-bfc1-1936ea44b486", "name": "Researcher", "description": "reads", "avatarShape": "blob", "avatarColor": "orange", "kind": "GROK_BOT_AGENT_KIND_AGENT", "updatedAtMs": "1795000000000" },
            { "agentId": "11111111-0010-4e9f-bfc1-1936ea44b486", "name": "Launch", "kind": "GROK_BOT_AGENT_KIND_ROOM", "memberAgentIds": ["a", "b"] },
            { "agentId": "22222222-0010-4e9f-bfc1-1936ea44b486", "kind": "GROK_BOT_AGENT_KIND_AGENT" },
            { "name": "no id" },
        ], "slackConnections": [] });
        let bots = parse_agents(&v);
        assert_eq!(bots.len(), 3);
        assert_eq!((bots[0].id.as_str(), bots[0].name.as_str(), bots[0].is_group), ("524fe60f-0010-4e9f-bfc1-1936ea44b486", "Researcher", false));
        assert_eq!((bots[0].avatar_shape.as_deref(), bots[0].avatar_color.as_deref()), (Some("blob"), Some("orange")));
        assert!(bots[1].is_group);
        assert_eq!((bots[2].name.as_str(), bots[2].avatar_shape.clone()), ("22222222-0010-4e9f-bfc1-1936ea44b486", None), "a bot never named goes by its id");
        assert!(parse_agents(&json!({})).is_empty());
    }

    /// A backend that answers one request with `status` and `body`, and hands back what it was sent.
    fn backend_once(status: u16, body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                seen.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&seen).to_string();
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = text[..head_end].lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                    if seen.len() >= head_end + 4 + length || n == 0 {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let _ = write!(stream, "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
            String::from_utf8_lossy(&seen).to_string()
        });
        (base, server)
    }

    #[test]
    fn a_call_carries_the_apps_sign_in_and_version_and_nothing_leaks_from_a_refusal() {
        let login = Login { token: "SECRET.TOKEN".into(), team: Some("42".into()) };
        let (base, server) = backend_once(200, r#"{"agents":[]}"#);
        let answer = call_at(&base, &login, "1.2.3", "ListGrokBotAgents", &json!({})).unwrap();
        assert_eq!(answer, json!({ "agents": [] }));
        let sent = server.join().unwrap().to_lowercase();
        assert!(sent.starts_with("post /aiserver.v1.grokbotservice/listgrokbotagents "), "{sent}");
        for header in ["authorization: bearer secret.token", "connect-protocol-version: 1", "x-cursor-client-version: 1.2.3", "x-cursor-client-type: sand", "x-cursor-team-id: 42", "content-type: application/json"] {
            assert!(sent.contains(header), "missing {header}");
        }
        assert!(!sent.contains("x-ghost-mode"), "the user's privacy mode is not ours to set");
        assert!(sent.ends_with("{}"));

        // a refusal that repeats the token back does not carry it into the error
        let (base, server) = backend_once(404, r#"{"code":"not_found","message":"The request could not be routed for SECRET.TOKEN"}"#);
        let fault = call_at(&base, &login, "1.2.3", "ListGrokBotAgents", &json!({})).unwrap_err();
        server.join().unwrap();
        assert_eq!(fault, Fault::Refused { status: 404, message: "The request could not be routed for <token>".into() });
        // nothing listening: the network's fault, not a refusal
        assert!(matches!(call_at("http://127.0.0.1:1", &login, "1.2.3", "ListGrokBotAgents", &json!({})), Err(Fault::Network(_))));
    }

    #[test]
    fn a_refusal_after_an_app_update_is_named_as_that() {
        let routed = Fault::Refused { status: 404, message: "The request could not be routed".into() };
        // the app is the version the calls worked with: just a refusal
        assert!(!routed.changed(Some("1.2.3"), Some("1.2.3")));
        assert!(routed.explain(Some("1.2.3"), Some("1.2.3")).contains("404"));
        // a new version, and refused: the interface moved
        assert!(routed.changed(Some("1.3.0"), Some("1.2.3")));
        assert_eq!(routed.explain(Some("1.3.0"), Some("1.2.3")), t("grok.changed").replace("{v}", "1.3.0"));
        // never worked yet: nothing to compare with
        assert!(!routed.changed(Some("1.3.0"), None));
        // a sign-in turned away is its own thing, whatever the version
        let expired = Fault::Refused { status: 401, message: "unauthenticated".into() };
        assert!(expired.expired() && !expired.changed(Some("1.3.0"), Some("1.2.3")));
        assert_eq!(expired.explain(Some("1.3.0"), Some("1.2.3")), t("grok.expired"));
        // the network being down is not the interface changing
        assert!(!Fault::Network("timeout".into()).changed(Some("1.3.0"), Some("1.2.3")));
    }

    #[test]
    fn a_send_is_taken_only_when_the_backend_says_dispatched() {
        assert!(dispatched(&json!({ "dispatched": true, "delivery": "GROK_BOT_USER_MESSAGE_DELIVERY_ACCEPTED_TEMPORAL" })).is_ok());
        assert_eq!(dispatched(&json!({ "delivery": "GROK_BOT_USER_MESSAGE_DELIVERY_REFUSED" })), Err(Fault::Refused { status: 200, message: "GROK_BOT_USER_MESSAGE_DELIVERY_REFUSED".into() }));
        assert!(dispatched(&json!({})).is_err());
        assert_eq!(status_word(&json!({ "status": "GROK_BOT_SEND_STATUS_ACCEPTED", "echoEntryId": "t4u" })), "accepted");
        assert_eq!(status_word(&json!({ "status": "GROK_BOT_SEND_STATUS_PENDING" })), "pending");
        assert_eq!(status_word(&json!({})), "");
    }

    #[test]
    fn sections_are_found_wherever_the_cache_keeps_them() {
        let wrapped = r#"{"schemaVersion":3,"value":{"sections":[{"id":"s1","name":"管理","agentIds":["b1","b2"]},{"id":"s2","name":"","agentIds":["b9"]}]}}"#;
        let bare = r#"{"schemaVersion":3,"value":[{"id":"s1","name":"运营","agentIds":["b3"]}]}"#;
        let s = sections_in(wrapped);
        assert_eq!(s.len(), 2);
        assert_eq!(section_of(&s, "B1").as_deref(), Some("管理"));
        assert_eq!(section_of(&s, "b9"), None, "a section without a name files nothing");
        assert_eq!(section_of(&sections_in(bare), "b3").as_deref(), Some("运营"));
        assert!(sections_in("not json").is_empty());
        assert!(sections_in(r#"{"schemaVersion":4,"value":{"something":"else"}}"#).is_empty(), "a layout this does not know reads as no sections");
    }

    #[test]
    fn cache_file_names_read_back_from_base32() {
        // "sidebar.last-sections" in unpadded lowercase Base32
        assert_eq!(base32_decode("onuwizlcmfzc43dbon2c243fmn2gs33oom").and_then(|b| String::from_utf8(b).ok()).as_deref(), Some("sidebar.last-sections"));
        assert_eq!(base32_decode("MZXW6").as_deref(), Some(&b"foo"[..]));
        assert_eq!(base32_decode("not base32!"), None);
    }

}

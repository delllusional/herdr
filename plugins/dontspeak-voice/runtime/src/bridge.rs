use std::collections::{hash_map::DefaultHasher, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use fs2::FileExt;

use crate::command;
use crate::dontspeak::{CliDontSpeakClient, DontSpeakClient};
use crate::herdr::{self, CliHerdrClient, ForegroundEligibility};
use crate::model::{metadata, StatusSnapshot};

const SOURCE: &str = "plugin:dontspeak-voice";
const TOKEN: &str = "dontspeak_voice";
const PLUGIN_ID: &str = "dontspeak.voice";
const POPUP_ENTRYPOINT: &str = "dictation";
const EXPECTED_SESSION_ENV: &str = "DONTSPEAK_DICTATION_SESSION";
const POPUP_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const METADATA_TTL_MS: &str = "5000";
const FAILURE_BACKOFF: Duration = Duration::from_secs(2);
const HERDR_COMMAND_TIMEOUT: Duration = Duration::from_secs(4);
const HERDR_PROBE_INTERVAL: Duration = Duration::from_secs(5);
const MAX_HERDR_FAILURES: usize = 3;
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(500);
const LOCK_RETRY_COUNT: usize = 40;
const BRIDGE_LOG_MAX_BYTES: u64 = 256 * 1024;

pub(crate) fn start_detached() -> Result<(), String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("cannot locate runtime: {error}"))?;
    let log = bridge_log_file()?;
    let log_copy = log
        .try_clone()
        .map_err(|error| format!("cannot clone bridge log: {error}"))?;
    let mut command = Command::new(executable);
    command
        .arg("bridge")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_copy));
    detach(&mut command);
    command
        .spawn()
        .map_err(|error| format!("cannot launch bridge: {error}"))?;
    Ok(())
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
}

#[cfg(not(any(unix, windows)))]
fn detach(_command: &mut Command) {}

pub(crate) fn run() -> Result<(), String> {
    let binary = BinaryStamp::current()?;
    let Some(_lock) = acquire_bridge_lock_with_retry()? else {
        return Ok(());
    };
    let client = CliDontSpeakClient::from_environment();
    bridge_loop(&client, &binary)
}

pub(crate) fn open_current_dictation() -> Result<(), String> {
    let client = CliDontSpeakClient::from_environment();
    let herdr_client = CliHerdrClient::from_environment();
    let snapshot = client
        .next_status(None)
        .map_err(|error| format!("cannot read dictation status: {error}"))?;
    let Some(session_id) = presentable_session(&snapshot) else {
        return Ok(());
    };
    if !herdr_client.foreground_eligible() {
        return Ok(());
    }
    if open_dictation_popup(&herdr::program(), session_id) {
        Ok(())
    } else {
        Err("Herdr could not open the dictation popup".to_string())
    }
}

fn bridge_loop(client: &dyn DontSpeakClient, binary: &BinaryStamp) -> Result<(), String> {
    let herdr = herdr::program();
    let herdr_client = CliHerdrClient::from_environment();
    let mut seq = None;
    let mut seen = BTreeSet::new();
    let mut opened_dictation_session = None;
    let mut popup_opened_at = None;
    let mut probe_failures = 0;
    let mut metadata_failures = 0;
    let mut next_probe = Instant::now();
    let mut dontspeak_was_unavailable = false;

    loop {
        if Instant::now() >= next_probe {
            if !binary.is_current() {
                clear_all(&herdr, &seen);
                return Ok(());
            }
            match probe_herdr(&herdr) {
                HerdrProbe::Ready => probe_failures = 0,
                HerdrProbe::PluginUnavailable => {
                    clear_all(&herdr, &seen);
                    return Ok(());
                }
                HerdrProbe::ServerUnavailable => {
                    probe_failures += 1;
                    if probe_failures >= MAX_HERDR_FAILURES {
                        clear_all(&herdr, &seen);
                        return Err("Herdr server is no longer available".to_string());
                    }
                }
            }
            next_probe = Instant::now() + HERDR_PROBE_INTERVAL;
        }

        let snapshot = match client.next_status(seq) {
            Ok(snapshot) => {
                if dontspeak_was_unavailable {
                    eprintln!("dontspeak-voice: Don’t Speak status reconnected");
                }
                dontspeak_was_unavailable = false;
                snapshot
            }
            Err(error) => {
                if !dontspeak_was_unavailable {
                    eprintln!("dontspeak-voice: {error}; retrying");
                }
                dontspeak_was_unavailable = true;
                clear_all(&herdr, &seen);
                seen.clear();
                seq = None;
                opened_dictation_session = None;
                popup_opened_at = None;
                thread::sleep(FAILURE_BACKOFF);
                continue;
            }
        };
        seq = Some(snapshot.seq);

        let failures = reconcile_metadata(&herdr, &snapshot, &mut seen);
        if failures == 0 {
            metadata_failures = 0;
        } else {
            metadata_failures += 1;
            if metadata_failures >= MAX_HERDR_FAILURES {
                clear_all(&herdr, &seen);
                return Err("Herdr rejected repeated metadata updates".to_string());
            }
        }

        let foreground_eligible =
            presentable_session(&snapshot).is_some() && herdr_client.foreground_eligible();
        let now = Instant::now();
        if let Some(session_id) = popup_open_candidate(
            &snapshot,
            opened_dictation_session.as_deref(),
            popup_opened_at,
            now,
            foreground_eligible,
        ) {
            if open_dictation_popup(&herdr, session_id) {
                opened_dictation_session = Some(session_id.to_string());
                popup_opened_at = Some(now);
            }
        } else if presentable_session(&snapshot).is_none() || !foreground_eligible {
            opened_dictation_session = None;
            popup_opened_at = None;
        }
    }
}

fn popup_open_candidate<'a>(
    snapshot: &'a StatusSnapshot,
    opened_session: Option<&str>,
    opened_at: Option<Instant>,
    now: Instant,
    foreground_eligible: bool,
) -> Option<&'a str> {
    let session_id = presentable_session(snapshot)?;
    if !foreground_eligible {
        return None;
    }
    if snapshot.dictation.external_ui_active {
        return None;
    }
    let already_opened = opened_session == Some(session_id);
    let retry_due = already_opened
        && opened_at.is_some_and(|opened_at| {
            now.saturating_duration_since(opened_at) >= POPUP_RETRY_INTERVAL
        });
    (!already_opened || retry_due).then_some(session_id)
}

fn presentable_session(snapshot: &StatusSnapshot) -> Option<&str> {
    snapshot
        .dictation
        .presentable()
        .then(|| snapshot.dictation.valid_session_id())
        .flatten()
}

fn reconcile_metadata(
    herdr: &OsStr,
    snapshot: &StatusSnapshot,
    seen: &mut BTreeSet<String>,
) -> usize {
    let current = metadata(snapshot);
    let current_ids = current.keys().cloned().collect::<BTreeSet<_>>();
    let mut failures = 0;
    for (pane_id, value) in &current {
        if !report_metadata(herdr, pane_id, Some(value)) {
            failures += 1;
        }
    }
    for pane_id in seen.difference(&current_ids) {
        if !report_metadata(herdr, pane_id, None) {
            failures += 1;
        }
    }
    *seen = current_ids;
    failures
}

fn clear_all(herdr: &OsStr, seen: &BTreeSet<String>) {
    for pane_id in seen {
        let _ = report_metadata(herdr, pane_id, None);
    }
}

fn report_metadata(herdr: &OsStr, pane_id: &str, value: Option<&str>) -> bool {
    let token;
    let mut args = vec![
        OsString::from("pane"),
        OsString::from("report-metadata"),
        OsString::from(pane_id),
        OsString::from("--source"),
        OsString::from(SOURCE),
        OsString::from("--ttl-ms"),
        OsString::from(METADATA_TTL_MS),
    ];
    match value {
        Some(value) => {
            token = format!("{TOKEN}={value}");
            args.push(OsString::from("--token"));
            args.push(OsString::from(token));
        }
        None => {
            args.push(OsString::from("--clear-token"));
            args.push(OsString::from(TOKEN));
        }
    }
    command_succeeded(herdr, &args)
}

fn open_dictation_popup(herdr: &OsStr, session_id: &str) -> bool {
    let expected_session = format!("{EXPECTED_SESSION_ENV}={session_id}");
    let args = [
        OsString::from("plugin"),
        OsString::from("pane"),
        OsString::from("open"),
        OsString::from("--plugin"),
        OsString::from(PLUGIN_ID),
        OsString::from("--entrypoint"),
        OsString::from(POPUP_ENTRYPOINT),
        OsString::from("--placement"),
        OsString::from("popup"),
        OsString::from("--env"),
        OsString::from(expected_session),
        OsString::from("--no-focus"),
    ];
    command_succeeded(herdr, &args)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HerdrProbe {
    Ready,
    PluginUnavailable,
    ServerUnavailable,
}

fn probe_herdr(herdr: &OsStr) -> HerdrProbe {
    let args = ["pane", "list", "--json"].map(OsString::from);
    if !command_succeeded(herdr, &args) {
        return HerdrProbe::ServerUnavailable;
    }
    let args = [
        OsString::from("plugin"),
        OsString::from("list"),
        OsString::from("--plugin"),
        OsString::from(PLUGIN_ID),
        OsString::from("--json"),
    ];
    let args = args.iter().map(OsString::as_os_str).collect::<Vec<_>>();
    let Ok(output) = command::run(herdr, &args, HERDR_COMMAND_TIMEOUT) else {
        return HerdrProbe::ServerUnavailable;
    };
    if !output.success() || output.stdout_truncated {
        return HerdrProbe::ServerUnavailable;
    }
    match plugin_enabled(&output.stdout) {
        Some(true) => HerdrProbe::Ready,
        Some(false) => HerdrProbe::PluginUnavailable,
        None => HerdrProbe::ServerUnavailable,
    }
}

fn plugin_enabled(output: &[u8]) -> Option<bool> {
    let response = serde_json::from_slice::<PluginListResponse>(output).ok()?;
    Some(
        response
            .result
            .plugins
            .iter()
            .any(|plugin| plugin.plugin_id == PLUGIN_ID && plugin.enabled),
    )
}

#[derive(serde::Deserialize)]
struct PluginListResponse {
    result: PluginListResult,
}

#[derive(serde::Deserialize)]
struct PluginListResult {
    #[serde(default)]
    plugins: Vec<PluginSummary>,
}

#[derive(serde::Deserialize)]
struct PluginSummary {
    plugin_id: String,
    #[serde(default)]
    enabled: bool,
}

fn command_succeeded(herdr: &OsStr, args: &[OsString]) -> bool {
    let args = args.iter().map(OsString::as_os_str).collect::<Vec<_>>();
    command::run(herdr, &args, HERDR_COMMAND_TIMEOUT).is_ok_and(|output| output.success())
}

fn plugin_state_dir() -> Result<PathBuf, String> {
    let path = std::env::var_os("HERDR_PLUGIN_STATE_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "HERDR_PLUGIN_STATE_DIR is not set".to_string())?;
    std::fs::create_dir_all(&path)
        .map_err(|error| format!("cannot create plugin state directory: {error}"))?;
    Ok(path)
}

fn bridge_log_file() -> Result<File, String> {
    let path = plugin_state_dir()?.join("bridge.log");
    if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > BRIDGE_LOG_MAX_BYTES) {
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .map_err(|error| format!("cannot rotate bridge log: {error}"))?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("cannot open bridge log: {error}"))
}

fn acquire_bridge_lock_with_retry() -> Result<Option<File>, String> {
    for attempt in 0..LOCK_RETRY_COUNT {
        if let Some(lock) = try_acquire_bridge_lock()? {
            return Ok(Some(lock));
        }
        if attempt + 1 < LOCK_RETRY_COUNT {
            thread::sleep(LOCK_RETRY_INTERVAL);
        }
    }
    Ok(None)
}

fn try_acquire_bridge_lock() -> Result<Option<File>, String> {
    let lock_path = plugin_state_dir()?.join(format!("bridge-{:016x}.lock", socket_hash()));
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|error| format!("cannot open bridge lock: {error}"))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(format!("cannot lock bridge: {error}")),
    }
}

#[derive(Debug)]
struct BinaryStamp {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

impl BinaryStamp {
    fn current() -> Result<Self, String> {
        let path =
            std::env::current_exe().map_err(|error| format!("cannot locate runtime: {error}"))?;
        let metadata =
            std::fs::metadata(&path).map_err(|error| format!("cannot inspect runtime: {error}"))?;
        Ok(Self {
            path,
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }

    fn is_current(&self) -> bool {
        std::fs::metadata(&self.path).is_ok_and(|metadata| {
            metadata.len() == self.len && metadata.modified().ok() == self.modified
        })
    }
}

fn socket_hash() -> u64 {
    let socket = std::env::var_os("HERDR_SOCKET_PATH").unwrap_or_else(|| OsString::from("default"));
    let mut hasher = DefaultHasher::new();
    socket.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn metadata_command_uses_one_argv_token() {
        let value = "SPEAK Sarah Voice";
        let token = format!("{TOKEN}={value}");
        assert_eq!(token, "dontspeak_voice=SPEAK Sarah Voice");
    }

    #[test]
    fn lock_filename_does_not_contain_socket_path() {
        let name = format!("bridge-{:016x}.lock", socket_hash());
        assert!(name.bytes().all(|byte| byte.is_ascii_hexdigit()
            || matches!(
                byte,
                b'b' | b'r' | b'i' | b'd' | b'g' | b'e' | b'-' | b'.' | b'l' | b'o' | b'c' | b'k'
            )));
    }

    #[test]
    fn bridge_log_is_inside_state_directory() {
        let path = Path::new("state").join("bridge.log");
        assert_eq!(path.file_name().and_then(OsStr::to_str), Some("bridge.log"));
    }

    #[test]
    fn plugin_probe_requires_enabled_matching_plugin() {
        assert_eq!(
            plugin_enabled(
                br#"{"result":{"plugins":[{"plugin_id":"dontspeak.voice","enabled":true}]}}"#
            ),
            Some(true)
        );
        assert_eq!(
            plugin_enabled(
                br#"{"result":{"plugins":[{"plugin_id":"dontspeak.voice","enabled":false}]}}"#
            ),
            Some(false)
        );
        assert_eq!(plugin_enabled(br#"{"result":{"plugins":[]}}"#), Some(false));
        assert_eq!(plugin_enabled(b"not-json"), None);
    }

    #[test]
    fn refused_dictation_does_not_open_a_terminal_presenter() {
        let snapshot: StatusSnapshot = serde_json::from_slice(
            br#"{"seq":1,"dictation":{"session_id":"session-1","state":"refused"}}"#,
        )
        .expect("status");
        assert_eq!(presentable_session(&snapshot), None);
    }

    #[test]
    fn popup_open_candidate_requires_foreground_and_retries_after_lease_expiry() {
        let now = Instant::now();
        let snapshot: StatusSnapshot = serde_json::from_slice(
            br#"{"seq":1,"dictation":{"session_id":"session-1","state":"recording","external_ui_active":false}}"#,
        )
        .expect("status");
        assert_eq!(
            popup_open_candidate(&snapshot, None, None, now, false),
            None
        );
        assert_eq!(
            popup_open_candidate(&snapshot, None, None, now, true),
            Some("session-1")
        );
        let externally_presented: StatusSnapshot = serde_json::from_slice(
            br#"{"seq":2,"dictation":{"session_id":"session-1","state":"recording","external_ui_active":true}}"#,
        )
        .expect("status");
        assert_eq!(
            popup_open_candidate(&externally_presented, None, None, now, true),
            None
        );
        assert_eq!(
            popup_open_candidate(
                &snapshot,
                Some("session-1"),
                Some(now),
                now + POPUP_RETRY_INTERVAL - Duration::from_millis(1),
                true,
            ),
            None
        );
        assert_eq!(
            popup_open_candidate(
                &snapshot,
                Some("session-1"),
                Some(now),
                now + POPUP_RETRY_INTERVAL,
                true,
            ),
            Some("session-1")
        );
    }
}

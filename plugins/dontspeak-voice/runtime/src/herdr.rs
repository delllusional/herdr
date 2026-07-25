use std::ffi::{OsStr, OsString};
use std::time::Duration;

use serde::Deserialize;

use crate::command;

const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) trait ForegroundEligibility {
    fn foreground_eligible(&self) -> bool;
}

pub(crate) struct CliHerdrClient {
    program: OsString,
}

impl CliHerdrClient {
    pub(crate) fn from_environment() -> Self {
        Self { program: program() }
    }
}

impl ForegroundEligibility for CliHerdrClient {
    fn foreground_eligible(&self) -> bool {
        let args = [OsStr::new("api"), OsStr::new("snapshot")];
        let Ok(output) = command::run(&self.program, &args, SNAPSHOT_TIMEOUT) else {
            return false;
        };
        output.success() && !output.stdout_truncated && snapshot_foreground_eligible(&output.stdout)
    }
}

pub(crate) fn program() -> OsString {
    std::env::var_os("HERDR_BIN_PATH")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("herdr"))
}

fn snapshot_foreground_eligible(output: &[u8]) -> bool {
    serde_json::from_slice::<SnapshotResponse>(output)
        .ok()
        .filter(|response| response.result.response_type == "session_snapshot")
        .and_then(|response| response.result.snapshot.outer_terminal_focus)
        == Some(true)
}

#[derive(Deserialize)]
struct SnapshotResponse {
    result: SnapshotResult,
}

#[derive(Deserialize)]
struct SnapshotResult {
    #[serde(rename = "type")]
    response_type: String,
    snapshot: Snapshot,
}

#[derive(Deserialize)]
struct Snapshot {
    #[serde(default)]
    outer_terminal_focus: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_requires_explicit_outer_terminal_focus() {
        assert!(snapshot_foreground_eligible(
            br#"{"result":{"type":"session_snapshot","snapshot":{"outer_terminal_focus":true}}}"#
        ));
        assert!(!snapshot_foreground_eligible(
            br#"{"result":{"type":"session_snapshot","snapshot":{"outer_terminal_focus":false}}}"#
        ));
        assert!(!snapshot_foreground_eligible(
            br#"{"result":{"type":"session_snapshot","snapshot":{}}}"#
        ));
        assert!(!snapshot_foreground_eligible(
            br#"{"result":{"type":"other","snapshot":{"outer_terminal_focus":true}}}"#
        ));
        assert!(!snapshot_foreground_eligible(b"not-json"));
    }
}

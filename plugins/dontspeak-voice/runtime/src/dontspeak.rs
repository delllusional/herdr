use std::ffi::OsString;
use std::fmt;
use std::time::Duration;

use serde::Deserialize;

use crate::command;
use crate::model::{valid_opaque_id, StatusSnapshot};

const STATUS_LONG_POLL_MS: u64 = 1_000;
const STATUS_COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const PRESENTER_COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) trait DontSpeakClient {
    fn next_status(&self, since: Option<u64>) -> Result<StatusSnapshot, ClientError>;

    fn presenter_acquire(
        &self,
        presenter_id: &str,
        session_id: &str,
        ttl_ms: u64,
    ) -> Result<PresenterLease, ClientError>;

    fn presenter_ready(&self, lease_id: &str, session_id: &str) -> Result<(), ClientError>;

    fn presenter_renew(
        &self,
        lease_id: &str,
        session_id: &str,
        ttl_ms: u64,
    ) -> Result<(), ClientError>;

    fn presenter_release(&self, lease_id: &str, session_id: &str) -> Result<(), ClientError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PresenterLease {
    pub(crate) lease_id: String,
    pub(crate) ttl_ms: u64,
}

pub(crate) struct CliDontSpeakClient {
    program: OsString,
}

impl CliDontSpeakClient {
    pub(crate) fn from_environment() -> Self {
        let program = std::env::var_os("DONTSPEAK_BIN_PATH")
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| OsString::from("dontspeak"));
        Self { program }
    }

    fn status_args(since: Option<u64>) -> Vec<OsString> {
        let mut args = vec![OsString::from("status"), OsString::from("--json")];
        if let Some(since) = since {
            args.push(OsString::from("--since"));
            args.push(OsString::from(since.to_string()));
            args.push(OsString::from("--timeout-ms"));
            args.push(OsString::from(STATUS_LONG_POLL_MS.to_string()));
        }
        args
    }

    fn run_presenter(&self, args: &[OsString]) -> Result<command::Output, ClientError> {
        let args = args.iter().map(OsString::as_os_str).collect::<Vec<_>>();
        checked_output(command::run(
            &self.program,
            &args,
            PRESENTER_COMMAND_TIMEOUT,
        ))
    }
}

impl DontSpeakClient for CliDontSpeakClient {
    fn next_status(&self, since: Option<u64>) -> Result<StatusSnapshot, ClientError> {
        let args = Self::status_args(since);
        let args = args.iter().map(OsString::as_os_str).collect::<Vec<_>>();
        let output = checked_output(command::run(&self.program, &args, STATUS_COMMAND_TIMEOUT))?;
        serde_json::from_slice(&output.stdout).map_err(|_| ClientError::Invalid)
    }

    fn presenter_acquire(
        &self,
        presenter_id: &str,
        session_id: &str,
        ttl_ms: u64,
    ) -> Result<PresenterLease, ClientError> {
        let args = presenter_acquire_args(presenter_id, session_id, ttl_ms)?;
        let output = self.run_presenter(&args)?;
        let response: LeaseResponse =
            serde_json::from_slice(&output.stdout).map_err(|_| ClientError::Invalid)?;
        let lease = response.into_lease();
        validate_opaque_id(&lease.lease_id)?;
        if lease.ttl_ms == 0 {
            return Err(ClientError::Invalid);
        }
        Ok(lease)
    }

    fn presenter_ready(&self, lease_id: &str, session_id: &str) -> Result<(), ClientError> {
        self.presenter_lease_command("ready", lease_id, session_id, None)
    }

    fn presenter_renew(
        &self,
        lease_id: &str,
        session_id: &str,
        ttl_ms: u64,
    ) -> Result<(), ClientError> {
        self.presenter_lease_command("renew", lease_id, session_id, Some(ttl_ms))
    }

    fn presenter_release(&self, lease_id: &str, session_id: &str) -> Result<(), ClientError> {
        self.presenter_lease_command("release", lease_id, session_id, None)
    }
}

impl CliDontSpeakClient {
    fn presenter_lease_command(
        &self,
        command_name: &str,
        lease_id: &str,
        session_id: &str,
        ttl_ms: Option<u64>,
    ) -> Result<(), ClientError> {
        let args = presenter_lease_args(command_name, lease_id, session_id, ttl_ms)?;
        self.run_presenter(&args).map(|_| ())
    }
}

fn presenter_acquire_args(
    presenter_id: &str,
    session_id: &str,
    ttl_ms: u64,
) -> Result<Vec<OsString>, ClientError> {
    validate_presenter_id(presenter_id)?;
    validate_opaque_id(session_id)?;
    Ok(vec![
        "presenter".into(),
        "acquire".into(),
        "--id".into(),
        presenter_id.into(),
        "--session".into(),
        session_id.into(),
        "--ttl-ms".into(),
        ttl_ms.to_string().into(),
    ])
}

fn presenter_lease_args(
    command_name: &str,
    lease_id: &str,
    session_id: &str,
    ttl_ms: Option<u64>,
) -> Result<Vec<OsString>, ClientError> {
    validate_opaque_id(lease_id)?;
    validate_opaque_id(session_id)?;
    let mut args = vec![
        "presenter".into(),
        command_name.into(),
        "--lease".into(),
        lease_id.into(),
        "--session".into(),
        session_id.into(),
    ];
    if let Some(ttl_ms) = ttl_ms {
        args.push("--ttl-ms".into());
        args.push(ttl_ms.to_string().into());
    }
    Ok(args)
}

fn checked_output(
    output: std::io::Result<command::Output>,
) -> Result<command::Output, ClientError> {
    let output = output.map_err(|_| ClientError::Unavailable)?;
    if output.timed_out {
        return Err(ClientError::Timeout);
    }
    if !output.success() {
        return Err(ClientError::Rejected(output.status.code()));
    }
    if output.stdout_truncated {
        return Err(ClientError::Oversized);
    }
    Ok(output)
}

fn validate_presenter_id(value: &str) -> Result<(), ClientError> {
    if !value.is_empty()
        && value.len() <= 120
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        Ok(())
    } else {
        Err(ClientError::Invalid)
    }
}

fn validate_opaque_id(value: &str) -> Result<(), ClientError> {
    if valid_opaque_id(value) {
        Ok(())
    } else {
        Err(ClientError::Invalid)
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LeaseResponse {
    Direct(LeasePayload),
    Wrapped { result: LeasePayload },
}

impl LeaseResponse {
    fn into_lease(self) -> PresenterLease {
        let payload = match self {
            Self::Direct(payload) | Self::Wrapped { result: payload } => payload,
        };
        PresenterLease {
            lease_id: payload.lease_id,
            ttl_ms: payload.ttl_ms,
        }
    }
}

#[derive(Debug, Deserialize)]
struct LeasePayload {
    lease_id: String,
    ttl_ms: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ClientError {
    Unavailable,
    Timeout,
    Rejected(Option<i32>),
    Oversized,
    Invalid,
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("Don’t Speak is unavailable"),
            Self::Timeout => formatter.write_str("Don’t Speak command timed out"),
            Self::Rejected(Some(code)) => {
                write!(formatter, "Don’t Speak command exited with code {code}")
            }
            Self::Rejected(None) => formatter.write_str("Don’t Speak command was terminated"),
            Self::Oversized => formatter.write_str("Don’t Speak response exceeded the size limit"),
            Self::Invalid => formatter.write_str("Don’t Speak returned an invalid response"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_poll_arguments_contain_only_status_sequence() {
        let args = CliDontSpeakClient::status_args(Some(42));
        let args = args
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            ["status", "--json", "--since", "42", "--timeout-ms", "1000"]
        );
    }

    #[test]
    fn lease_response_accepts_direct_and_wrapped_json() {
        for input in [
            r#"{"lease_id":"lease-1","ttl_ms":3500}"#,
            r#"{"result":{"lease_id":"lease-1","ttl_ms":3500}}"#,
        ] {
            let response: LeaseResponse = serde_json::from_str(input).expect("lease");
            assert_eq!(
                response.into_lease(),
                PresenterLease {
                    lease_id: "lease-1".to_string(),
                    ttl_ms: 3_500,
                }
            );
        }
    }

    #[test]
    fn presenter_arguments_match_the_cli_contract() {
        let acquire =
            presenter_acquire_args("herdr.voice", "session-1", 3_500).expect("acquire args");
        let ready =
            presenter_lease_args("ready", "lease-1", "session-1", None).expect("ready args");
        let renew =
            presenter_lease_args("renew", "lease-1", "session-1", Some(3_500)).expect("renew args");
        let release =
            presenter_lease_args("release", "lease-1", "session-1", None).expect("release args");
        let strings = |args: Vec<OsString>| {
            args.into_iter()
                .map(|value| value.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            strings(acquire),
            [
                "presenter",
                "acquire",
                "--id",
                "herdr.voice",
                "--session",
                "session-1",
                "--ttl-ms",
                "3500",
            ]
        );
        assert_eq!(
            strings(ready),
            [
                "presenter",
                "ready",
                "--lease",
                "lease-1",
                "--session",
                "session-1",
            ]
        );
        assert_eq!(
            strings(renew),
            [
                "presenter",
                "renew",
                "--lease",
                "lease-1",
                "--session",
                "session-1",
                "--ttl-ms",
                "3500",
            ]
        );
        assert_eq!(
            strings(release),
            [
                "presenter",
                "release",
                "--lease",
                "lease-1",
                "--session",
                "session-1",
            ]
        );
    }
}

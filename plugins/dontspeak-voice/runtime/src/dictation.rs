use std::io::{self, Write};
use std::thread;
use std::time::Duration;

use crate::dontspeak::{CliDontSpeakClient, DontSpeakClient};
use crate::herdr::{CliHerdrClient, ForegroundEligibility};

const PRESENTER_ID: &str = "herdr.voice";
const PRESENTER_TTL_MS: u64 = 3_500;
const EXPECTED_SESSION_ENV: &str = "DONTSPEAK_DICTATION_SESSION";
const RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_CONSECUTIVE_FAILURES: usize = 3;

pub(crate) fn run() -> Result<(), String> {
    let client = CliDontSpeakClient::from_environment();
    let herdr = CliHerdrClient::from_environment();
    let Ok(expected_session) = std::env::var(EXPECTED_SESSION_ENV) else {
        return Ok(());
    };
    render_loop(&client, &herdr, &expected_session, &mut io::stdout())
}

fn render_loop(
    client: &dyn DontSpeakClient,
    foreground: &dyn ForegroundEligibility,
    expected_session: &str,
    output: &mut impl Write,
) -> Result<(), String> {
    let Some(initial) = first_status(client) else {
        return Ok(());
    };
    if !initial.dictation.presentable() {
        return Ok(());
    }
    let Some(session_id) = initial.dictation.valid_session_id() else {
        return Ok(());
    };
    if expected_session != session_id {
        return Ok(());
    }
    if !foreground.foreground_eligible() {
        return Ok(());
    }
    let session_id = session_id.to_string();
    let Ok(lease) = client.presenter_acquire(PRESENTER_ID, &session_id, PRESENTER_TTL_MS) else {
        return Ok(());
    };
    let lease = LeaseGuard::new(client, lease.lease_id, session_id.clone());

    draw(output, &initial.dictation.text)?;
    if !foreground.foreground_eligible() {
        return Ok(());
    }
    if client
        .presenter_ready(lease.lease_id(), &session_id)
        .is_err()
    {
        return Ok(());
    }

    let mut seq = Some(initial.seq);
    let mut previous_text = initial.dictation.text;
    loop {
        let snapshot = match client.next_status(seq) {
            Ok(snapshot) => snapshot,
            Err(_) => return Ok(()),
        };
        seq = Some(snapshot.seq);
        if !snapshot.dictation.presentable()
            || snapshot.dictation.valid_session_id() != Some(session_id.as_str())
        {
            return Ok(());
        }
        if !foreground.foreground_eligible() {
            return Ok(());
        }
        if previous_text != snapshot.dictation.text {
            draw(output, &snapshot.dictation.text)?;
            previous_text = snapshot.dictation.text;
        }
        if client
            .presenter_renew(lease.lease_id(), &session_id, PRESENTER_TTL_MS)
            .is_err()
        {
            return Ok(());
        }
    }
}

fn first_status(client: &dyn DontSpeakClient) -> Option<crate::model::StatusSnapshot> {
    for attempt in 0..MAX_CONSECUTIVE_FAILURES {
        if let Ok(snapshot) = client.next_status(None) {
            return Some(snapshot);
        }
        if attempt + 1 < MAX_CONSECUTIVE_FAILURES {
            thread::sleep(RETRY_DELAY);
        }
    }
    None
}

fn draw(output: &mut impl Write, text: &str) -> Result<(), String> {
    let text = terminal_safe_text(text);
    write!(output, "\x1b[2J\x1b[H{text}")
        .map_err(|error| format!("could not render dictation: {error}"))?;
    output
        .flush()
        .map_err(|error| format!("could not flush dictation: {error}"))
}

fn terminal_safe_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() && !matches!(character, '\n' | '\t') {
                ' '
            } else {
                character
            }
        })
        .collect()
}

struct LeaseGuard<'a> {
    client: &'a dyn DontSpeakClient,
    lease_id: String,
    session_id: String,
}

impl<'a> LeaseGuard<'a> {
    fn new(client: &'a dyn DontSpeakClient, lease_id: String, session_id: String) -> Self {
        Self {
            client,
            lease_id,
            session_id,
        }
    }

    fn lease_id(&self) -> &str {
        &self.lease_id
    }
}

impl Drop for LeaseGuard<'_> {
    fn drop(&mut self) {
        let _ = self
            .client
            .presenter_release(&self.lease_id, &self.session_id);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use crate::dontspeak::{ClientError, PresenterLease};
    use crate::model::StatusSnapshot;

    use super::*;

    struct FakeClient {
        responses: RefCell<VecDeque<Result<StatusSnapshot, ClientError>>>,
        events: RefCell<Vec<String>>,
    }

    struct FakeForeground {
        responses: RefCell<VecDeque<bool>>,
    }

    impl ForegroundEligibility for FakeForeground {
        fn foreground_eligible(&self) -> bool {
            self.responses
                .borrow_mut()
                .pop_front()
                .expect("fake foreground response")
        }
    }

    impl DontSpeakClient for FakeClient {
        fn next_status(&self, _since: Option<u64>) -> Result<StatusSnapshot, ClientError> {
            self.responses
                .borrow_mut()
                .pop_front()
                .expect("fake status response")
        }

        fn presenter_acquire(
            &self,
            presenter_id: &str,
            session_id: &str,
            ttl_ms: u64,
        ) -> Result<PresenterLease, ClientError> {
            self.events
                .borrow_mut()
                .push(format!("acquire:{presenter_id}:{session_id}:{ttl_ms}"));
            Ok(PresenterLease {
                lease_id: "lease-1".to_string(),
                ttl_ms,
            })
        }

        fn presenter_ready(&self, lease_id: &str, session_id: &str) -> Result<(), ClientError> {
            self.events
                .borrow_mut()
                .push(format!("ready:{lease_id}:{session_id}"));
            Ok(())
        }

        fn presenter_renew(
            &self,
            lease_id: &str,
            session_id: &str,
            ttl_ms: u64,
        ) -> Result<(), ClientError> {
            self.events
                .borrow_mut()
                .push(format!("renew:{lease_id}:{session_id}:{ttl_ms}"));
            Ok(())
        }

        fn presenter_release(&self, lease_id: &str, session_id: &str) -> Result<(), ClientError> {
            self.events
                .borrow_mut()
                .push(format!("release:{lease_id}:{session_id}"));
            Ok(())
        }
    }

    fn status(seq: u64, session_id: Option<&str>, state: &str, text: &str) -> StatusSnapshot {
        serde_json::from_value(serde_json::json!({
            "seq": seq,
            "dictation": {
                "session_id": session_id,
                "state": state,
                "text": text
            }
        }))
        .expect("status")
    }

    #[test]
    fn renderer_obeys_presenter_lease_lifecycle() {
        let client = FakeClient {
            responses: RefCell::new(VecDeque::from([
                Ok(status(1, Some("session-1"), "recording", "hello")),
                Ok(status(2, Some("session-1"), "recording", "hello")),
                Ok(status(
                    3,
                    Some("session-1"),
                    "awaiting_confirm",
                    "hello world",
                )),
                Ok(status(4, Some("session-1"), "hidden", "")),
            ])),
            events: RefCell::new(Vec::new()),
        };
        let foreground = FakeForeground {
            responses: RefCell::new(VecDeque::from([true, true, true, true])),
        };
        let mut output = Vec::new();
        render_loop(&client, &foreground, "session-1", &mut output).expect("render");
        let output = String::from_utf8(output).expect("utf8");
        assert_eq!(output, "\u{1b}[2J\u{1b}[Hhello\u{1b}[2J\u{1b}[Hhello world");
        assert_eq!(
            client.events.borrow().as_slice(),
            [
                "acquire:herdr.voice:session-1:3500",
                "ready:lease-1:session-1",
                "renew:lease-1:session-1:3500",
                "renew:lease-1:session-1:3500",
                "release:lease-1:session-1",
            ]
        );
    }

    #[test]
    fn expected_session_mismatch_fails_closed_without_acquire() {
        let client = FakeClient {
            responses: RefCell::new(VecDeque::from([Ok(status(
                1,
                Some("new-session"),
                "recording",
                "private text",
            ))])),
            events: RefCell::new(Vec::new()),
        };
        let foreground = FakeForeground {
            responses: RefCell::new(VecDeque::new()),
        };
        let mut output = Vec::new();
        render_loop(&client, &foreground, "old-session", &mut output).expect("render");
        assert!(output.is_empty());
        assert!(client.events.borrow().is_empty());
    }

    #[test]
    fn status_failure_releases_presenter_instead_of_showing_stale_text() {
        let client = FakeClient {
            responses: RefCell::new(VecDeque::from([
                Ok(status(1, Some("session-1"), "recording", "hello")),
                Err(ClientError::Timeout),
            ])),
            events: RefCell::new(Vec::new()),
        };
        let foreground = FakeForeground {
            responses: RefCell::new(VecDeque::from([true, true])),
        };
        let mut output = Vec::new();
        render_loop(&client, &foreground, "session-1", &mut output).expect("render");
        assert_eq!(
            client.events.borrow().as_slice(),
            [
                "acquire:herdr.voice:session-1:3500",
                "ready:lease-1:session-1",
                "release:lease-1:session-1",
            ]
        );
    }

    #[test]
    fn background_start_does_not_acquire_or_render() {
        let client = FakeClient {
            responses: RefCell::new(VecDeque::from([Ok(status(
                1,
                Some("session-1"),
                "recording",
                "private text",
            ))])),
            events: RefCell::new(Vec::new()),
        };
        let foreground = FakeForeground {
            responses: RefCell::new(VecDeque::from([false])),
        };
        let mut output = Vec::new();
        render_loop(&client, &foreground, "session-1", &mut output).expect("render");
        assert!(client.events.borrow().is_empty());
        assert!(output.is_empty());
    }

    #[test]
    fn focus_loss_after_ready_releases_without_renewing() {
        let client = FakeClient {
            responses: RefCell::new(VecDeque::from([
                Ok(status(1, Some("session-1"), "recording", "hello")),
                Ok(status(2, Some("session-1"), "recording", "hello world")),
            ])),
            events: RefCell::new(Vec::new()),
        };
        let foreground = FakeForeground {
            responses: RefCell::new(VecDeque::from([true, true, false])),
        };
        let mut output = Vec::new();
        render_loop(&client, &foreground, "session-1", &mut output).expect("render");
        assert_eq!(
            client.events.borrow().as_slice(),
            [
                "acquire:herdr.voice:session-1:3500",
                "ready:lease-1:session-1",
                "release:lease-1:session-1",
            ]
        );
        assert_eq!(
            String::from_utf8(output).expect("utf8"),
            "\u{1b}[2J\u{1b}[Hhello"
        );
    }

    #[test]
    fn renderer_strips_terminal_control_sequences_from_transcript() {
        let mut output = Vec::new();
        draw(&mut output, "hello\u{1b}[31m\nworld\u{8}").expect("render");
        assert_eq!(
            String::from_utf8(output).expect("utf8"),
            "\u{1b}[2J\u{1b}[Hhello [31m\nworld "
        );
    }
}

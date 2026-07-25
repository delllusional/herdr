use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct StatusSnapshot {
    pub(crate) seq: u64,
    #[serde(default)]
    pub(crate) activity: Activity,
    #[serde(default)]
    pub(crate) voice_sessions: Vec<VoiceSession>,
    #[serde(default)]
    pub(crate) dictation: Dictation,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Activity {
    #[serde(default)]
    pub(crate) muted: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct VoiceSession {
    pub(crate) pane_id: Option<String>,
    #[serde(default)]
    pub(crate) speaking: bool,
    #[serde(default)]
    pub(crate) queued: u64,
    #[serde(default)]
    pub(crate) blocked: bool,
    pub(crate) voice: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Dictation {
    #[serde(default)]
    pub(crate) session_id: Option<String>,
    #[serde(default = "hidden_state")]
    pub(crate) state: String,
    #[serde(default)]
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) external_ui_active: bool,
}

impl Default for Dictation {
    fn default() -> Self {
        Self {
            session_id: None,
            state: hidden_state(),
            text: String::new(),
            external_ui_active: false,
        }
    }
}

impl Dictation {
    pub(crate) fn presentable(&self) -> bool {
        matches!(self.state.as_str(), "recording" | "awaiting_confirm")
    }

    pub(crate) fn valid_session_id(&self) -> Option<&str> {
        self.session_id
            .as_deref()
            .filter(|value| valid_opaque_id(value))
    }
}

fn hidden_state() -> String {
    "hidden".to_string()
}

pub(crate) fn metadata(snapshot: &StatusSnapshot) -> BTreeMap<String, String> {
    snapshot
        .voice_sessions
        .iter()
        .filter_map(|session| {
            let pane_id = session
                .pane_id
                .as_deref()
                .filter(|pane_id| valid_pane_id(pane_id))?;
            Some((
                pane_id.to_string(),
                token_for(session, snapshot.activity.muted),
            ))
        })
        .collect()
}

fn valid_pane_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 120
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
}

pub(crate) fn valid_opaque_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 120
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn token_for(session: &VoiceSession, muted: bool) -> String {
    let voice = sanitized_voice(session.voice.as_deref());
    if muted {
        format!("MUTE {voice}")
    } else if session.speaking {
        format!("SPEAK {voice}")
    } else if session.blocked {
        format!("WAIT {} {voice}", session.queued)
    } else if session.queued > 0 {
        format!("Q{} {voice}", session.queued)
    } else {
        voice
    }
}

fn sanitized_voice(value: Option<&str>) -> String {
    let mut output = String::new();
    let mut previous_space = false;
    for ch in value.unwrap_or("voice").chars().take(48) {
        let ch = if ch.is_control() { ' ' } else { ch };
        if ch.is_whitespace() {
            if !previous_space && !output.is_empty() {
                output.push(' ');
            }
            previous_space = true;
        } else {
            output.push(ch);
            previous_space = false;
        }
    }
    let output = output.trim();
    if output.is_empty() {
        "voice".to_string()
    } else {
        output.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> VoiceSession {
        VoiceSession {
            pane_id: Some("w1:p2".to_string()),
            speaking: false,
            queued: 0,
            blocked: false,
            voice: Some("Sarah".to_string()),
        }
    }

    #[test]
    fn token_priority_matches_existing_plugin() {
        let mut row = session();
        assert_eq!(token_for(&row, false), "Sarah");
        row.queued = 2;
        assert_eq!(token_for(&row, false), "Q2 Sarah");
        row.blocked = true;
        assert_eq!(token_for(&row, false), "WAIT 2 Sarah");
        row.speaking = true;
        assert_eq!(token_for(&row, false), "SPEAK Sarah");
        assert_eq!(token_for(&row, true), "MUTE Sarah");
    }

    #[test]
    fn metadata_rejects_invalid_pane_ids_and_sanitizes_labels() {
        let snapshot: StatusSnapshot = serde_json::from_str(
            r#"{
                "seq": 4,
                "activity": {"muted": false},
                "voice_sessions": [
                    {"pane_id":"--bad","voice":"secret\nvoice"},
                    {"pane_id":"w1:p2","voice":"Sarah\tVoice","queued":1}
                ],
                "dictation": {"state":"hidden","text":""}
            }"#,
        )
        .expect("status");
        let rows = metadata(&snapshot);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows["w1:p2"], "Q1 Sarah Voice");
    }

    #[test]
    fn dictation_defaults_hidden_for_older_snapshots() {
        let snapshot: StatusSnapshot = serde_json::from_str(r#"{"seq":1}"#).expect("status");
        assert!(!snapshot.dictation.presentable());
        assert_eq!(snapshot.dictation.valid_session_id(), None);
    }

    #[test]
    fn refused_dictation_remains_native() {
        let snapshot: StatusSnapshot = serde_json::from_str(
            r#"{"seq":1,"dictation":{"session_id":"session-1","state":"refused"}}"#,
        )
        .expect("status");
        assert!(!snapshot.dictation.presentable());
    }

    #[test]
    fn presenter_session_id_uses_the_cli_safe_opaque_grammar() {
        let snapshot: StatusSnapshot = serde_json::from_str(
            r#"{"seq":1,"dictation":{"session_id":"feedface-0000000000000007","state":"recording"}}"#,
        )
        .expect("status");
        assert_eq!(
            snapshot.dictation.valid_session_id(),
            Some("feedface-0000000000000007")
        );
    }
}

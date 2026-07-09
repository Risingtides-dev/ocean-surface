//! Voice interaction mode + the hands-free runtime state machine.
//!
//! The orb operates in one of four modes, picked from a popover menu:
//!
//! - [`VoiceMode::Off`] — the microphone is gated off at the device level. No
//!   audio is captured, ever; a true kill switch. It persists across reloads as
//!   `"off"` only because the user set it that way; the disclosed open-mic mode
//!   never does (see [`VoiceMode::persist_key`]).
//! - [`VoiceMode::Dictate`] — hold the orb to talk (same capture as
//!   PushToTalk), but the transcript is appended to the composer instead of
//!   auto-sent. The user reviews and presses Send manually.
//! - [`VoiceMode::PushToTalk`] — hold the orb to talk (the default, original
//!   behavior). Audio is sent only while the orb is held.
//! - [`VoiceMode::HandsFree`] — open mic. Each VAD-endpointed phrase is sent to
//!   STT and submitted automatically. Disclosed up-front: the mic is streaming
//!   to xAI the whole time it is active.
//!
//! [`HandsFreeState`] is the pure logic for the single hands-free mode: every
//! non-empty utterance is submitted. It owns no audio and no `web-sys`, so the
//! decision rule is unit-tested off the browser. The orb installs it only while
//! [`VoiceMode::HandsFree`] is active.

/// Which interaction mode the orb is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VoiceMode {
    /// Microphone gated off at the device level — a kill switch. No audio is
    /// ever captured; the orb renders muted and clicks open the mode menu.
    Off,
    /// Hold-to-talk that appends transcripts to the composer without sending.
    /// Capture is identical to PushToTalk; the transcript lands in the message
    /// box for review before manual send.
    Dictate,
    /// Hold-to-talk (default). Audio is sent only while the orb is held.
    #[default]
    PushToTalk,
    /// Open mic — each VAD-endpointed phrase is sent for transcription.
    HandsFree,
}

impl VoiceMode {
    /// The next mode when the user cycles via the keyboard chord
    /// (Cmd/Ctrl+Shift+V). Off is included so the chord doubles as a panic
    /// kill; HandsFree cycles back to Off.
    pub fn next(self) -> Self {
        match self {
            VoiceMode::Off => VoiceMode::Dictate,
            VoiceMode::Dictate => VoiceMode::PushToTalk,
            VoiceMode::PushToTalk => VoiceMode::HandsFree,
            VoiceMode::HandsFree => VoiceMode::Off,
        }
    }

    /// Short label for the menu row + the orb hint line.
    pub fn label(self) -> &'static str {
        match self {
            VoiceMode::Off => "Off",
            VoiceMode::Dictate => "Dictate",
            VoiceMode::PushToTalk => "Push to talk",
            VoiceMode::HandsFree => "Hands-free",
        }
    }

    /// CSS modifier so each mode can style its orb distinctly.
    pub fn css_modifier(self) -> &'static str {
        match self {
            VoiceMode::Off => "is-off",
            VoiceMode::Dictate => "is-dictate",
            VoiceMode::PushToTalk => "is-ptt",
            VoiceMode::HandsFree => "is-hands-free",
        }
    }

    /// The localStorage token this mode persists as. Off persists as `"off"`;
    /// Dictate as `"dictate"`; PushToTalk and the disclosed open-mic HandsFree
    /// mode both persist as `"ptt"` so HandsFree is NEVER silently restored
    /// across a reload — the user must opt back into it each session.
    pub fn persist_key(self) -> &'static str {
        match self {
            VoiceMode::Off => "off",
            VoiceMode::Dictate => "dictate",
            VoiceMode::PushToTalk | VoiceMode::HandsFree => "ptt",
        }
    }

    /// Recover a mode from its persisted token. `"off"` restores Off, `"dictate"`
    /// restores Dictate; anything else (unknown, empty, or `"ptt"`) falls back to
    /// the safe default, push-to-talk.
    pub fn from_persisted(s: &str) -> Self {
        match s.trim() {
            "off" => VoiceMode::Off,
            "dictate" => VoiceMode::Dictate,
            _ => VoiceMode::PushToTalk,
        }
    }
}

/// What the orb should do with a freshly transcribed utterance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandsFreeAction {
    /// Do nothing — empty/whitespace utterance.
    Ignore,
    /// Submit this text as a prompt to the agent.
    Submit(String),
}

/// Pure decision logic for the hands-free (open-mic) mode: every non-empty
/// utterance is submitted. Kept as a struct (not a free fn) so the capture
/// layer's `Option<HandsFreeState>` router contract stays intact — push-to-talk
/// and Off leave it unset, hands-free installs one. Owns no audio / `web-sys`,
/// so the rule is unit-tested off the browser.
#[derive(Debug, Clone, Copy)]
pub struct HandsFreeState;

impl HandsFreeState {
    /// A hands-free router. The `mode` arg is accepted so the capture layer can
    /// build one from the current mode unconditionally; only HandsFree actually
    /// installs it.
    #[allow(clippy::needless_pass_by_value)]
    pub fn new(_mode: VoiceMode) -> Self {
        Self
    }

    /// Feed a transcribed utterance; decide what to do with it.
    pub fn on_utterance(&mut self, transcript: &str) -> HandsFreeAction {
        let text = transcript.trim();
        if text.is_empty() {
            HandsFreeAction::Ignore
        } else {
            HandsFreeAction::Submit(text.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_push_to_talk() {
        assert_eq!(VoiceMode::default(), VoiceMode::PushToTalk);
    }

    #[test]
    fn cycle_includes_off_as_panic_kill() {
        assert_eq!(VoiceMode::Off.next(), VoiceMode::Dictate);
        assert_eq!(VoiceMode::Dictate.next(), VoiceMode::PushToTalk);
        assert_eq!(VoiceMode::PushToTalk.next(), VoiceMode::HandsFree);
        assert_eq!(VoiceMode::HandsFree.next(), VoiceMode::Off);
        // Full loop returns home.
        assert_eq!(
            VoiceMode::default().next().next().next().next(),
            VoiceMode::default()
        );
    }

    #[test]
    fn labels_and_modifiers_cover_all_variants() {
        assert_eq!(VoiceMode::Off.label(), "Off");
        assert_eq!(VoiceMode::Dictate.label(), "Dictate");
        assert_eq!(VoiceMode::PushToTalk.label(), "Push to talk");
        assert_eq!(VoiceMode::HandsFree.label(), "Hands-free");

        assert_eq!(VoiceMode::Off.css_modifier(), "is-off");
        assert_eq!(VoiceMode::Dictate.css_modifier(), "is-dictate");
        assert_eq!(VoiceMode::PushToTalk.css_modifier(), "is-ptt");
        assert_eq!(VoiceMode::HandsFree.css_modifier(), "is-hands-free");
    }

    #[test]
    fn persistence_round_trips_off_ptt_and_dictate() {
        assert_eq!(VoiceMode::Off.persist_key(), "off");
        assert_eq!(VoiceMode::Dictate.persist_key(), "dictate");
        assert_eq!(VoiceMode::PushToTalk.persist_key(), "ptt");

        assert_eq!(
            VoiceMode::from_persisted(VoiceMode::Off.persist_key()),
            VoiceMode::Off
        );
        assert_eq!(
            VoiceMode::from_persisted(VoiceMode::Dictate.persist_key()),
            VoiceMode::Dictate
        );
        assert_eq!(
            VoiceMode::from_persisted(VoiceMode::PushToTalk.persist_key()),
            VoiceMode::PushToTalk
        );
    }
    #[test]
    fn hands_free_never_persists_or_restores() {
        // The disclosed open-mic mode must never be silently restored across a
        // reload: it persists as the safe default, and that token decodes back
        // to push-to-talk, never HandsFree.
        assert_eq!(VoiceMode::HandsFree.persist_key(), "ptt");
        assert_ne!(VoiceMode::HandsFree.persist_key(), "is-hands-free");
        assert_eq!(
            VoiceMode::from_persisted(VoiceMode::HandsFree.persist_key()),
            VoiceMode::PushToTalk
        );
    }

    #[test]
    fn unknown_persisted_token_defaults_to_push_to_talk() {
        assert_eq!(VoiceMode::from_persisted(""), VoiceMode::PushToTalk);
        assert_eq!(VoiceMode::from_persisted("garbage"), VoiceMode::PushToTalk);
        // Surrounding whitespace is tolerated for "off".
        assert_eq!(VoiceMode::from_persisted("  off  "), VoiceMode::Off);
    }

    #[test]
    fn hands_free_submits_every_utterance() {
        let mut s = HandsFreeState::new(VoiceMode::HandsFree);
        assert_eq!(
            s.on_utterance("what's the weather"),
            HandsFreeAction::Submit("what's the weather".to_string())
        );
        assert_eq!(
            s.on_utterance("and tomorrow"),
            HandsFreeAction::Submit("and tomorrow".to_string())
        );
    }

    #[test]
    fn hands_free_ignores_empty() {
        let mut s = HandsFreeState::new(VoiceMode::HandsFree);
        assert_eq!(s.on_utterance("   "), HandsFreeAction::Ignore);
        assert_eq!(s.on_utterance(""), HandsFreeAction::Ignore);
    }
}

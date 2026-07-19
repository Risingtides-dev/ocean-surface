//! Capture-bound completion admission for voice.
//!
//! Async voice completions (STT transcripts, recorder status, the hands-free
//! listener handle) resolve long after the capture that produced them began. In
//! between, the user can switch modes or hit Off, so the callbacks/thread-locals
//! present at *completion* time no longer describe the capture. Routing through
//! them lets a Dictate capture auto-send after a switch to push-to-talk, or a
//! stale listener claim mic ownership after Off.
//!
//! The fix stamps every capture with an immutable [`CaptureId`] — the voice
//! [generation](VoiceGen) and mode live when it started — and admits a
//! completion only when that stamp still matches the current generation. Every
//! mode/capture lifecycle transition mints a fresh generation, so any capture
//! from a prior generation is rejected. This module is the pure decision rule:
//! it owns no `web-sys` and no thread-locals, so it is unit-tested off the
//! browser.

use super::mode::VoiceMode;

/// The immutable identity stamped onto a capture at the moment it begins. It is
/// carried through the recorder + upload path and checked, unchanged, when the
/// completion finally lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CaptureId {
    pub generation: u64,
    pub mode: VoiceMode,
}

/// The live voice generation + mode a completion is checked against. Advanced on
/// every mode/capture lifecycle transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceGen {
    pub generation: u64,
    pub mode: VoiceMode,
}

impl From<VoiceGen> for CaptureId {
    fn from(g: VoiceGen) -> Self {
        Self {
            generation: g.generation,
            mode: g.mode,
        }
    }
}

/// Whether a capture-stamped completion may act on the current voice state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// The capture is still current — deliver its transcript / status / handle.
    Admit,
    /// Stale: a mode/capture transition (or Off) has superseded this capture.
    Reject,
}

impl Admission {
    /// True only for [`Admission::Admit`].
    pub fn is_admitted(self) -> bool {
        matches!(self, Admission::Admit)
    }
}

/// Decide whether a completion stamped with `capture` may be admitted against
/// the live `current` generation.
///
/// A completion is admitted only when the capture's generation and mode both
/// still match the live generation, and the live mode is not [`VoiceMode::Off`]
/// — Off is a hard device gate that invalidates every prior capture. Because a
/// fresh generation is minted on every transition, a generation match already
/// implies the mode matches; the explicit mode and Off checks are kept so the
/// rule is self-evidently safe and independently testable.
pub fn admit(capture: CaptureId, current: VoiceGen) -> Admission {
    if current.mode == VoiceMode::Off {
        return Admission::Reject;
    }
    if capture.generation != current.generation {
        return Admission::Reject;
    }
    if capture.mode != current.mode {
        return Admission::Reject;
    }
    Admission::Admit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(generation: u64, mode: VoiceMode) -> CaptureId {
        CaptureId { generation, mode }
    }

    fn cur(generation: u64, mode: VoiceMode) -> VoiceGen {
        VoiceGen { generation, mode }
    }

    #[test]
    fn dictate_to_push_to_talk_switch_mid_upload_rejected() {
        // A Dictate upload started at gen 1 must not auto-send after the user
        // switched to push-to-talk (gen 2).
        let admission = admit(cap(1, VoiceMode::Dictate), cur(2, VoiceMode::PushToTalk));
        assert_eq!(admission, Admission::Reject);
    }

    #[test]
    fn push_to_talk_to_dictate_switch_rejected() {
        // A push-to-talk upload started at gen 1 must not append to the composer
        // after a switch to Dictate (gen 2).
        let admission = admit(cap(1, VoiceMode::PushToTalk), cur(2, VoiceMode::Dictate));
        assert_eq!(admission, Admission::Reject);
    }

    #[test]
    fn any_mode_to_off_rejected() {
        // Off invalidates a pending capture from every prior mode.
        assert_eq!(
            admit(cap(1, VoiceMode::HandsFree), cur(2, VoiceMode::Off)),
            Admission::Reject
        );
        assert_eq!(
            admit(cap(1, VoiceMode::Dictate), cur(2, VoiceMode::Off)),
            Admission::Reject
        );
        assert_eq!(
            admit(cap(1, VoiceMode::PushToTalk), cur(2, VoiceMode::Off)),
            Admission::Reject
        );
    }

    #[test]
    fn old_hands_free_segment_after_restart_rejected() {
        // Hands-free torn down and restarted mints a new generation; a segment
        // uploaded from the old run is same-mode but stale.
        let admission = admit(cap(1, VoiceMode::HandsFree), cur(3, VoiceMode::HandsFree));
        assert_eq!(admission, Admission::Reject);
    }

    #[test]
    fn late_listen_start_after_switch_rejected() {
        // A hands-free listen::start() that resolves after the user left the mode
        // must not install its handle.
        let admission = admit(cap(2, VoiceMode::HandsFree), cur(3, VoiceMode::Off));
        assert_eq!(admission, Admission::Reject);
    }

    #[test]
    fn late_listen_start_still_current_admitted() {
        // If nothing changed by the time listen::start() resolves, the handle is
        // installed.
        let admission = admit(cap(2, VoiceMode::HandsFree), cur(2, VoiceMode::HandsFree));
        assert_eq!(admission, Admission::Admit);
    }

    #[test]
    fn same_mode_new_generation_rejected() {
        // Re-entering the same mode still mints a new generation; a capture from
        // the prior generation is rejected even though the mode matches.
        let admission = admit(cap(1, VoiceMode::PushToTalk), cur(2, VoiceMode::PushToTalk));
        assert_eq!(admission, Admission::Reject);
    }

    #[test]
    fn valid_current_completion_admitted() {
        // The common case: the capture is still the current generation + mode.
        assert_eq!(
            admit(cap(5, VoiceMode::HandsFree), cur(5, VoiceMode::HandsFree)),
            Admission::Admit
        );
        assert_eq!(
            admit(cap(5, VoiceMode::PushToTalk), cur(5, VoiceMode::PushToTalk)),
            Admission::Admit
        );
        assert_eq!(
            admit(cap(5, VoiceMode::Dictate), cur(5, VoiceMode::Dictate)),
            Admission::Admit
        );
    }

    #[test]
    fn off_current_never_admits() {
        // Even a generation-matched capture is rejected while the live gate is
        // Off — nothing may complete against a device-off mic.
        let admission = admit(cap(2, VoiceMode::Off), cur(2, VoiceMode::Off));
        assert_eq!(admission, Admission::Reject);
    }
}

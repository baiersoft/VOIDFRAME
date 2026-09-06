//! Opt-in (`--sound`) audio cue bracketing the real PresentMon capture
//! window, for headless sessions with no visual feedback to watch (spec:
//! user request, single-monitor rig running CS2 fullscreen — needs an
//! audible signal to line capture timing up precisely). Shared by `run` and
//! `calibrate`, not calibrate-specific.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundCue {
    Start,
    Stop,
}

/// Maps an `EngineEvent` to the sound cue it should trigger, if any. Pure —
/// testable without any Win32 call actually happening. `RecordingStarted`/
/// `RecordingStopped` bracket only the real PresentMon capture call itself,
/// on every measure iteration (see `run/execute/scenario.rs`) —
/// `CapturePending`/`CaptureResumed` are deliberately NOT used here: they
/// bracket a wider settle+benchmark-end-wait window meant for the
/// webview-suspend feature, confirmed by live listening to fire at
/// map-start/map-end rather than the real capture boundary.
pub fn sound_cue_for_event(ev: &voidframe_engine::run::EngineEvent) -> Option<SoundCue> {
    use voidframe_engine::run::EngineEvent;
    match ev {
        EngineEvent::RecordingStarted => Some(SoundCue::Start),
        EngineEvent::RecordingStopped => Some(SoundCue::Stop),
        _ => None,
    }
}

#[cfg(windows)]
pub fn play(cue: SoundCue) {
    use windows::Win32::System::Diagnostics::Debug::MessageBeep;
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONASTERISK, MB_ICONHAND};
    let style = match cue {
        SoundCue::Start => MB_ICONASTERISK,
        SoundCue::Stop => MB_ICONHAND,
    };
    // SAFETY: `MessageBeep` takes a plain `MESSAGEBOX_STYLE` value (an
    // enum-like `u32`) and queues a system sound; it has no pointer/handle
    // arguments and no preconditions beyond a valid style constant, which
    // the match above guarantees.
    unsafe {
        let _ = MessageBeep(style);
    }
}

#[cfg(not(windows))]
pub fn play(_cue: SoundCue) {}

#[cfg(test)]
mod tests {
    use super::*;
    use voidframe_engine::run::EngineEvent;

    #[test]
    fn recording_started_maps_to_start() {
        assert_eq!(
            sound_cue_for_event(&EngineEvent::RecordingStarted),
            Some(SoundCue::Start)
        );
    }

    #[test]
    fn recording_stopped_maps_to_stop() {
        assert_eq!(
            sound_cue_for_event(&EngineEvent::RecordingStopped),
            Some(SoundCue::Stop)
        );
    }

    #[test]
    fn capture_pending_and_resumed_no_longer_map_to_a_cue() {
        // CapturePending/CaptureResumed bracket a WIDER window (settle time
        // + the post-capture benchmark-end wait) for the webview-suspend
        // feature -- too imprecise for a sound cue meant to line up with
        // the actual PresentMon recording window (confirmed by live
        // listening: the old mapping fired at map-start/map-end, not at
        // the real capture boundary). RecordingStarted/RecordingStopped
        // replace them here.
        assert_eq!(sound_cue_for_event(&EngineEvent::CapturePending), None);
        assert_eq!(sound_cue_for_event(&EngineEvent::CaptureResumed), None);
    }

    #[test]
    fn an_unrelated_event_maps_to_no_cue() {
        assert_eq!(
            sound_cue_for_event(&EngineEvent::RunComplete {
                run_id: "r1".into()
            }),
            None
        );
    }
}

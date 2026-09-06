//! Classifies PresentMon's `PresentMode` column into whether the desktop
//! compositor (DWM) was involved. This is a real diagnostic, not just
//! descriptive metadata: a capture taken while the game is windowed or
//! borderless (rather than exclusive Fullscreen) adds compositor latency
//! and noise unrelated to whatever tweak is under test. Values and
//! descriptions per PresentMon's own docs
//! (`GameTechDev/PresentMon`, `README-ConsoleApplication.md`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentModeCategory {
    /// Exclusive or near-exclusive screen ownership -- no compositor
    /// overhead. "Hardware: Legacy Flip", "Hardware: Legacy Copy to front
    /// buffer", "Hardware: Independent Flip".
    Exclusive,
    /// DWM-composed, but with a hardware overlay plane -- some compositor
    /// involvement, still hardware-accelerated. "Hardware Composed:
    /// Independent Flip".
    ComposedWithOverlay,
    /// Fully DWM-composed, no hardware overlay. "Composed: Flip",
    /// "Composed: Copy with GPU GDI", "Composed: Copy with CPU GDI" --
    /// usually means the game is windowed or borderless, not exclusive
    /// Fullscreen.
    Composed,
    /// A `PresentMode` string this classifier doesn't recognise (e.g. a
    /// future PresentMon version adds a new mode) -- never silently
    /// treated as fine; surfaces as its own category.
    Unknown,
}

impl PresentModeCategory {
    /// `true` for `Exclusive` and `ComposedWithOverlay` -- both keep at
    /// least a hardware-accelerated, non-fully-composed path. Only
    /// `Composed` (and `Unknown`, conservatively) count as a likely
    /// misconfiguration worth flagging.
    pub fn is_compositor_bypassed(&self) -> bool {
        matches!(
            self,
            PresentModeCategory::Exclusive | PresentModeCategory::ComposedWithOverlay
        )
    }
}

/// Classifies a raw PresentMon `PresentMode` string. An unrecognised value
/// maps to `Unknown`, never silently treated as either good or bad.
pub fn classify_present_mode(raw: &str) -> PresentModeCategory {
    match raw {
        "Hardware: Legacy Flip"
        | "Hardware: Legacy Copy to front buffer"
        | "Hardware: Independent Flip" => PresentModeCategory::Exclusive,
        "Hardware Composed: Independent Flip" => PresentModeCategory::ComposedWithOverlay,
        "Composed: Flip" | "Composed: Copy with GPU GDI" | "Composed: Copy with CPU GDI" => {
            PresentModeCategory::Composed
        }
        _ => PresentModeCategory::Unknown,
    }
}

/// The shared compositor-misconfiguration warning, keyed off a dominant
/// `PresentMode` string. Fires ONLY for `PresentModeCategory::Composed` --
/// a confirmed compositor-composed mode, where the "isn't in exclusive
/// Fullscreen" claim is actually supported. `Unknown` (an unrecognised
/// string, or an empty string from deserializing a legacy `results.json`
/// that predates these fields via `#[serde(default)]`) produces no
/// warning: we can't tell what happened, so we don't claim a fullscreen
/// misconfiguration we have no evidence for. Shared by
/// `capture::parser::aggregate_metrics` (per-frame) and
/// `stats::aggregate_iterations` (per-iteration) so there's exactly one
/// place that owns the warning text and firing condition.
pub fn misconfiguration_warning(dominant_mode: &str) -> Option<String> {
    if classify_present_mode(dominant_mode) == PresentModeCategory::Composed {
        Some(format!(
            "Most frames ({dominant_mode}) went through the desktop compositor -- \
             this usually means CS2 isn't in exclusive Fullscreen mode, which can add latency \
             and measurement noise unrelated to the tweak under test."
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_every_documented_presentmon_mode() {
        assert_eq!(
            classify_present_mode("Hardware: Legacy Flip"),
            PresentModeCategory::Exclusive
        );
        assert_eq!(
            classify_present_mode("Hardware: Legacy Copy to front buffer"),
            PresentModeCategory::Exclusive
        );
        assert_eq!(
            classify_present_mode("Hardware: Independent Flip"),
            PresentModeCategory::Exclusive
        );
        assert_eq!(
            classify_present_mode("Hardware Composed: Independent Flip"),
            PresentModeCategory::ComposedWithOverlay
        );
        assert_eq!(
            classify_present_mode("Composed: Flip"),
            PresentModeCategory::Composed
        );
        assert_eq!(
            classify_present_mode("Composed: Copy with GPU GDI"),
            PresentModeCategory::Composed
        );
        assert_eq!(
            classify_present_mode("Composed: Copy with CPU GDI"),
            PresentModeCategory::Composed
        );
    }

    #[test]
    fn unrecognised_value_is_unknown_not_silently_exclusive() {
        assert_eq!(
            classify_present_mode("Something Future PresentMon Adds"),
            PresentModeCategory::Unknown
        );
    }

    #[test]
    fn compositor_bypassed_is_true_only_for_exclusive_and_overlay() {
        assert!(PresentModeCategory::Exclusive.is_compositor_bypassed());
        assert!(PresentModeCategory::ComposedWithOverlay.is_compositor_bypassed());
        assert!(!PresentModeCategory::Composed.is_compositor_bypassed());
        assert!(!PresentModeCategory::Unknown.is_compositor_bypassed());
    }

    #[test]
    fn misconfiguration_warning_fires_only_for_composed() {
        let warning = misconfiguration_warning("Composed: Flip")
            .expect("Composed dominant mode should produce a warning");
        assert!(warning.contains("compositor"));
        assert!(warning.contains("Composed: Flip"));
    }

    #[test]
    fn misconfiguration_warning_is_silent_for_exclusive_and_overlay() {
        assert_eq!(misconfiguration_warning("Hardware: Independent Flip"), None);
        assert_eq!(
            misconfiguration_warning("Hardware Composed: Independent Flip"),
            None
        );
    }

    #[test]
    fn misconfiguration_warning_is_silent_for_unknown_and_empty_mode() {
        // An unrecognised PresentMode string, or an empty string from
        // deserializing a legacy results.json that predates these fields --
        // neither is evidence of a compositor misconfiguration, so neither
        // should produce the fullscreen-misconfiguration warning.
        assert_eq!(
            misconfiguration_warning("Something Future PresentMon Adds"),
            None
        );
        assert_eq!(misconfiguration_warning(""), None);
    }
}

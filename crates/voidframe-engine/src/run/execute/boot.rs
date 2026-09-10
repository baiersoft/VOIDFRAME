//! Spec §4: turn `SystemController::boot_report` facts into one class.
//! Pure so every combination is unit-tested; the run loop only acts on
//! the class.

use crate::system::BootReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BootClass {
    Clean,
    Bugcheck,
    PowerLoss,
    StuckSafeboot,
    UnexpectedExtraReboot,
}

/// `boot_count_before_this_resume` is `PendingReboot.boot_count` as read
/// from disk *before* this resume increments it: 0 on the first resume
/// after VOIDFRAME's own reboot, >= 1 if the machine rebooted again.
pub(super) fn classify_boot(report: &BootReport, boot_count_before_this_resume: u32) -> BootClass {
    if boot_count_before_this_resume >= 1 {
        return BootClass::UnexpectedExtraReboot;
    }
    if report.safeboot_option.is_some() {
        return BootClass::StuckSafeboot;
    }
    if report.bugcheck_1001 && !report.new_minidumps.is_empty() {
        return BootClass::Bugcheck;
    }
    if report.kernel_power_41 {
        return BootClass::PowerLoss;
    }
    BootClass::Clean
}

pub(super) fn boot_class_reason(class: BootClass) -> &'static str {
    match class {
        BootClass::Clean => "clean boot",
        BootClass::Bugcheck => "bugcheck (event 1001 + minidump) after applying this scenario",
        BootClass::PowerLoss => {
            "power loss / hard reset (event 41 without a bugcheck record) after applying this scenario"
        }
        BootClass::StuckSafeboot => "machine booted with the BCD safeboot option set",
        BootClass::UnexpectedExtraReboot => "the machine rebooted again before VOIDFRAME resumed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn r(k41: bool, b1001: bool, dumps: usize, safeboot: Option<&str>) -> BootReport {
        BootReport {
            kernel_power_41: k41,
            bugcheck_1001: b1001,
            new_minidumps: (0..dumps)
                .map(|i| PathBuf::from(format!("{i}.dmp")))
                .collect(),
            safeboot_option: safeboot.map(str::to_string),
        }
    }

    #[test]
    fn clean_when_nothing_is_reported() {
        assert_eq!(
            classify_boot(&r(false, false, 0, None), 0),
            BootClass::Clean
        );
    }

    #[test]
    fn bugcheck_needs_1001_and_a_minidump() {
        assert_eq!(
            classify_boot(&r(true, true, 1, None), 0),
            BootClass::Bugcheck
        );
        assert_eq!(
            classify_boot(&r(false, true, 1, None), 0),
            BootClass::Bugcheck
        );
        assert_eq!(classify_boot(&r(false, true, 0, None), 0), BootClass::Clean);
    }

    #[test]
    fn kernel_power_41_alone_is_power_loss() {
        assert_eq!(
            classify_boot(&r(true, false, 0, None), 0),
            BootClass::PowerLoss
        );
    }

    #[test]
    fn safeboot_wins_over_everything_else() {
        assert_eq!(
            classify_boot(&r(true, true, 1, Some("Minimal")), 0),
            BootClass::StuckSafeboot
        );
    }

    #[test]
    fn a_second_boot_for_the_same_pending_reboot_is_unexpected() {
        assert_eq!(
            classify_boot(&r(false, false, 0, None), 1),
            BootClass::UnexpectedExtraReboot
        );
    }
}

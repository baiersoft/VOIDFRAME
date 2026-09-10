//! The reboot rule (spec 2026-09-06-m3-autonomous-reboots-design.md D2),
//! evaluated after a scenario's revert: `prev` is the scenario that just
//! reverted, `next` the one about to start (`None` at the end of the run).
//! One pure function so the run loop and the builder's reboot estimate
//! cannot disagree.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// Neither side needs a reboot: continue as today.
    None,
    /// `prev`'s revert needs a boot to take effect and `next` does not need
    /// one (or there is no `next`): reboot first, then continue.
    RebootThenContinue,
    /// `next` needs a boot: apply it now so a single reboot makes both the
    /// revert and the new apply effective.
    ApplyNextThenReboot,
}

pub fn plan_transition(prev_needs_reboot: bool, next_needs_reboot: Option<bool>) -> Transition {
    match (prev_needs_reboot, next_needs_reboot) {
        (_, Some(true)) => Transition::ApplyNextThenReboot,
        (true, Some(false)) | (true, None) => Transition::RebootThenContinue,
        (false, Some(false)) | (false, None) => Transition::None,
    }
}

/// Total reboots for an ordered list of per-scenario reboot needs
/// (baseline first). Every `ApplyNextThenReboot` and `RebootThenContinue`
/// is one reboot; a leading reboot scenario also costs its own apply reboot.
pub fn count_reboots(needs: &[bool]) -> u32 {
    let mut reboots = 0;
    let mut prev = false;
    for (i, &need) in needs.iter().enumerate() {
        if i == 0 && need {
            reboots += 1;
        } else if i > 0 {
            match plan_transition(prev, Some(need)) {
                Transition::None => {}
                Transition::RebootThenContinue | Transition::ApplyNextThenReboot => reboots += 1,
            }
        }
        prev = need;
    }
    if prev {
        reboots += 1; // the final revert's reboot (or the shutdown that replaces it)
    }
    reboots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_reboots_anywhere_is_none() {
        assert_eq!(plan_transition(false, Some(false)), Transition::None);
        assert_eq!(plan_transition(false, None), Transition::None);
    }

    #[test]
    fn a_reboot_scenario_followed_by_a_plain_one_reboots_first() {
        assert_eq!(
            plan_transition(true, Some(false)),
            Transition::RebootThenContinue
        );
    }

    #[test]
    fn a_reboot_scenario_at_the_end_reboots_for_its_revert() {
        assert_eq!(plan_transition(true, None), Transition::RebootThenContinue);
    }

    #[test]
    fn a_reboot_scenario_next_shares_one_reboot_with_the_revert() {
        assert_eq!(
            plan_transition(true, Some(true)),
            Transition::ApplyNextThenReboot
        );
        assert_eq!(
            plan_transition(false, Some(true)),
            Transition::ApplyNextThenReboot
        );
    }

    #[test]
    fn count_reboots_matches_the_rule_on_mixed_matrices() {
        // baseline, HAGS, plain, HAGS, HAGS, plain
        assert_eq!(count_reboots(&[false, true, false, true, true, false]), 5);
        // baseline, plain, plain
        assert_eq!(count_reboots(&[false, false, false]), 0);
        // baseline, HAGS (end): apply reboot + revert reboot
        assert_eq!(count_reboots(&[false, true]), 2);
        // baseline, HAGS, HAGS: apply, shared, final revert
        assert_eq!(count_reboots(&[false, true, true]), 3);
    }
}

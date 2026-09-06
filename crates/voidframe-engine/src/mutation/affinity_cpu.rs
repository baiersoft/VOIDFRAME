//! `affinity_cpu` — derives a process-affinity bitmask from live CPU
//! topology. No journal entry: process affinity has no persisted state
//! (`docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md` §4.3)
//! — it is re-applied at every CS2 launch
//! (`docs/superpowers/plans/2026-09-01-m1-phase-3b-cs2-control-run-loop.md`), not here.

use crate::error::{Error, Result};
use crate::model::module::{AffinityCpuPayload, AffinityMode};
use crate::system::{CpuTopology, SystemController};

fn mask_from_ids(ids: impl IntoIterator<Item = u32>) -> u64 {
    ids.into_iter().fold(
        0u64,
        |acc, id| if id < 64 { acc | (1u64 << id) } else { acc },
    )
}

/// Compute the affinity mask for `mode` against `topo`. Rejects a mask that
/// selects zero cores, all cores, or (M1) spans more than one processor
/// group.
pub fn compute_mask(mode: AffinityMode, mask_hex: Option<&str>, topo: &CpuTopology) -> Result<u64> {
    let all = mask_from_ids(topo.all_logical_ids());
    let mask = match mode {
        AffinityMode::ExcludeCore0 => {
            let drop = mask_from_ids(topo.physical_core0_logical_ids());
            all & !drop
        }
        AffinityMode::PcoreOnly => mask_from_ids(
            topo.cores
                .iter()
                .filter(|c| !c.is_efficiency)
                .flat_map(|c| c.logical_ids.clone()),
        ),
        AffinityMode::Ccd0 => mask_from_ids(
            topo.cores
                .iter()
                .filter(|c| c.ccd == Some(0))
                .flat_map(|c| c.logical_ids.clone()),
        ),
        AffinityMode::ExplicitMask => {
            let s = mask_hex.ok_or_else(|| Error::msg("explicit_mask requires mask_hex".into()))?;
            let s = s.trim_start_matches("0x").trim_start_matches("0X");
            u64::from_str_radix(s, 16)
                .map_err(|_| Error::msg("mask_hex is not valid hex".into()))?
        }
    };
    if mask == 0 {
        return Err(Error::msg("affinity mask selects zero cores".into()));
    }
    if mask == all {
        return Err(Error::msg("affinity mask selects all cores (no-op)".into()));
    }
    if topo.group_count > 1 {
        return Err(Error::msg(
            "multi-processor-group affinity is not supported in M1".into(),
        ));
    }
    Ok(mask)
}

pub async fn apply_to_pid(
    payload: &AffinityCpuPayload,
    pid: u32,
    sys: &dyn SystemController,
) -> Result<()> {
    let topo = sys.cpu_topology().await?;
    let mask = compute_mask(payload.mode, payload.mask_hex.as_deref(), &topo)?;
    sys.set_process_affinity(pid, mask).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::CoreGroup;

    fn topo(smt: bool) -> CpuTopology {
        let cores = (0..4u32)
            .map(|i| {
                let ids = if smt { vec![i * 2, i * 2 + 1] } else { vec![i] };
                CoreGroup {
                    physical_id: i,
                    logical_ids: ids,
                    is_efficiency: i >= 2,
                    ccd: Some(if i < 2 { 0 } else { 1 }),
                }
            })
            .collect();
        CpuTopology {
            cores,
            smt_enabled: smt,
            group_count: 1,
        }
    }

    #[test]
    fn exclude_core0_drops_both_siblings_when_smt() {
        let m = compute_mask(AffinityMode::ExcludeCore0, None, &topo(true)).unwrap();
        assert_eq!(m & 0b11, 0);
        assert_eq!(m, 0b1111_1100);
    }

    #[test]
    fn exclude_core0_drops_only_id0_without_smt() {
        let m = compute_mask(AffinityMode::ExcludeCore0, None, &topo(false)).unwrap();
        assert_eq!(m, 0b1110);
    }

    #[test]
    fn pcore_only_selects_non_efficiency_cores() {
        let m = compute_mask(AffinityMode::PcoreOnly, None, &topo(true)).unwrap();
        assert_eq!(m, 0b0000_1111);
    }

    #[test]
    fn ccd0_selects_ccd_zero_cores() {
        let m = compute_mask(AffinityMode::Ccd0, None, &topo(true)).unwrap();
        assert_eq!(m, 0b0000_1111);
    }

    #[test]
    fn explicit_mask_parses_hex() {
        let m = compute_mask(AffinityMode::ExplicitMask, Some("0x00000004"), &topo(true)).unwrap();
        assert_eq!(m, 4);
    }

    #[test]
    fn rejects_zero_and_full_masks() {
        assert!(compute_mask(AffinityMode::ExplicitMask, Some("0x0"), &topo(false)).is_err());
        assert!(compute_mask(AffinityMode::ExplicitMask, Some("0xF"), &topo(false)).is_err());
    }
}

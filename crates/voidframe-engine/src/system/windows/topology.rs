//! Real CPU topology via `GetLogicalProcessorInformationEx`. Verified
//! against this rig before this file was written
//! (see `docs/superpowers/plans/2026-09-01-m1-phase-3a-windows-controller.md`'s
//! CPU-topology task) —
//! notably, the buffer holds *variable-length* records: walk it using each
//! record's own `.Size` field as the stride, never `size_of::<...>()`.

use crate::error::{Error, Result};
use crate::system::{CoreGroup, CpuTopology};
use windows::Win32::System::SystemInformation::{
    GetLogicalProcessorInformationEx, RelationProcessorCore,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};

fn read_sync() -> Result<CpuTopology> {
    let mut len: u32 = 0;
    // Sizing call: expected to "fail" with the buffer-too-small HRESULT
    // while populating `len` with the real required length — this is not
    // an error condition, it's the documented two-call pattern.
    // SAFETY: `None` buffer with `&mut len` is the documented two-call
    // sizing pattern; the only output is `len`.
    let _ = unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, None, &mut len) };
    if len == 0 {
        return Err(Error::msg(
            "GetLogicalProcessorInformationEx sizing call returned a zero-length buffer".into(),
        ));
    }

    // Vec<u64> instead of Vec<u8>: guarantees 8-byte alignment for the
    // SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX records we're about to cast
    // this buffer's bytes into — a Vec<u8> only guarantees 1-byte alignment,
    // which is technically insufficient even though it works on every real
    // allocator.
    let word_len = (len as usize).div_ceil(8);
    let mut buf: Vec<u64> = vec![0u64; word_len];
    let byte_ptr = buf.as_mut_ptr() as *mut u8;
    let ptr = byte_ptr as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX;
    // SAFETY: `ptr` points into `buf`, a `Vec<u64>` of at least `len` bytes
    // (8-byte aligned, which satisfies the struct's alignment); `buf`
    // outlives this call.
    unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, Some(ptr), &mut len) }
        .map_err(|e| {
            Error::msg(format!(
                "GetLogicalProcessorInformationEx data call failed: {e}"
            ))
        })?;

    // Single pass over the buffer: collect each core's logical ids plus its
    // raw EfficiencyClass. `EfficiencyClass` is a relative ranking where a
    // *higher* value means intrinsically greater performance and less
    // efficiency (a P-core), and a *lower* value (0 being the lowest) means
    // greater efficiency and less performance (an E-core), per Microsoft's
    // docs — there is no single "this is an E-core" bit, so classification
    // needs the *maximum* class across every core, which isn't known until
    // every record has been read. Collecting each core's raw class
    // alongside it here, then classifying in a second, buffer-free pass
    // over `cores` (below), avoids walking the raw FFI buffer twice.
    let mut cores: Vec<(CoreGroup, u32)> = Vec::new();
    let mut max_group_seen: u32 = 0;
    let mut offset: isize = 0;
    while (offset as u32) < len {
        // SAFETY: the loop guard checks `offset < len`, so `byte_ptr.offset
        // (offset)` stays within `buf`'s allocation.
        let record_ptr =
            unsafe { byte_ptr.offset(offset) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX };
        // SAFETY: `record_ptr` points at a record written by the OS in the
        // fill call above, whose `Size` field (used below to advance
        // `offset`) was written by the OS.
        let record = unsafe { &*record_ptr };
        if record.Relationship == RelationProcessorCore {
            // SAFETY: `Relationship == RelationProcessorCore` was just
            // checked above, so the union's `Processor` field is the one
            // the OS actually wrote for this record.
            let proc_rel = unsafe { &record.Anonymous.Processor };
            let mut logical_ids = Vec::new();
            for gi in 0..proc_rel.GroupCount as usize {
                let ga = &proc_rel.GroupMask[gi];
                let group = ga.Group as u32;
                max_group_seen = max_group_seen.max(group);
                let mask: usize = ga.Mask;
                for bit in 0..(usize::BITS as usize) {
                    if (mask >> bit) & 1 == 1 {
                        logical_ids.push(group * 64 + bit as u32);
                    }
                }
            }
            let physical_id = cores.len() as u32;
            cores.push((
                CoreGroup {
                    physical_id,
                    logical_ids,
                    is_efficiency: false, // classified below, once every core is known
                    ccd: None,            // CCD/NUMA-node grouping needs RelationNumaNode,
                                          // out of scope for M1's affinity presets (§4.3:
                                          // pcore_only / ccd0 / exclude_core0 / explicit_mask
                                          // — only ccd0 needs this, and is lower-priority
                                          // than the other three per the catalog).
                },
                proc_rel.EfficiencyClass as u32,
            ));
        }
        offset += record.Size as isize;
    }

    // Classify efficiency cores now that every core's raw class is known:
    // "is this an E-core" reduces to "is this core's class less than the
    // maximum class seen" (uniformly 0 across all 8 cores on this
    // non-hybrid rig — the `true` branch is untested against real hybrid
    // hardware, but the comparison itself has no hybrid-specific logic).
    let max_class = cores.iter().map(|(_, class)| *class).max().unwrap_or(0);
    let cores: Vec<CoreGroup> = cores
        .into_iter()
        .map(|(mut core, class)| {
            core.is_efficiency = class < max_class;
            core
        })
        .collect();

    let smt_enabled = cores.iter().any(|c| c.logical_ids.len() > 1);
    Ok(CpuTopology {
        cores,
        smt_enabled,
        group_count: max_group_seen + 1,
    })
}

pub async fn read() -> Result<CpuTopology> {
    tokio::task::spawn_blocking(read_sync)
        .await
        .map_err(|e| Error::msg(format!("topology read task panicked: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    // This genuinely queries the real machine's topology — there's no way
    // to mock GetLogicalProcessorInformationEx meaningfully, so this test
    // asserts *invariants* that hold on any real Windows machine rather
    // than a fixed expected value. Confirmed concretely on this rig at
    // verification time: 8 physical cores, each SMT with exactly 2 logical
    // siblings, uniform efficiency_class=0, matching
    // available_parallelism() == 16 exactly.
    #[tokio::test]
    async fn topology_is_internally_consistent_on_the_real_machine() {
        let topo = read().await.unwrap();
        assert!(!topo.cores.is_empty(), "must find at least one core");

        let total_logical: usize = topo.cores.iter().map(|c| c.logical_ids.len()).sum();
        let expected = std::thread::available_parallelism().unwrap().get();
        assert_eq!(
            total_logical, expected,
            "sum of every core's logical ids must equal available_parallelism()"
        );

        // Every physical_id is unique and every logical_id across every
        // core is unique (no double-counted logical processor).
        let mut phys_ids: Vec<u32> = topo.cores.iter().map(|c| c.physical_id).collect();
        phys_ids.sort_unstable();
        phys_ids.dedup();
        assert_eq!(
            phys_ids.len(),
            topo.cores.len(),
            "physical_id must be unique per core"
        );

        let mut all_logical = topo.all_logical_ids();
        let before_dedup_len = all_logical.len();
        all_logical.dedup();
        assert_eq!(
            all_logical.len(),
            before_dedup_len,
            "no logical id should appear under two physical cores"
        );

        if topo.smt_enabled {
            assert!(
                topo.cores.iter().any(|c| c.logical_ids.len() > 1),
                "smt_enabled=true must be backed by at least one multi-sibling core"
            );
        }
    }
}

//! Write-ahead journal: every mutation is recorded, with its computed
//! inverse, *before* it is applied. A two-phase record (an `applied: false`
//! line, then a follow-up `applied: true` line) makes a crash mid-mutation
//! recoverable — [`replay`] treats an unconfirmed record defensively.

pub mod live;
pub mod replay;
pub mod restore_script;

use crate::error::Result;
use crate::system::MutationCtx;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    RegistryWrite,
    RegistryDelete,
    PowercfgWrite,
    PowerPlanActivate,
    PowerPlanCreate,
    PowerPlanDelete,
    CustomScriptApply,
    Cs2ConfigApply,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalRecord {
    pub seq: u64,
    pub ts_unix_ms: u128,
    pub run_id: String,
    pub scenario_id: String,
    pub step_index: u32,
    pub op: Op,
    pub target: serde_json::Value,
    pub new: serde_json::Value,
    pub inverse: serde_json::Value,
    #[serde(default)]
    pub applied: bool,
}

#[derive(Deserialize)]
struct AppliedOverlay {
    seq: u64,
    applied: bool,
}

/// An append-only, `fsync`-per-line JSONL journal file.
pub struct Journal {
    file: File,
    next_seq: u64,
    path: PathBuf,
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

impl Journal {
    /// Open (creating if needed) the journal at `path`. `next_seq` continues
    /// from the highest `seq` already present, so re-opening a journal across
    /// a process restart does not reuse sequence numbers.
    pub fn open(path: &Path) -> Result<Journal> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let highest = if path.exists() {
            let f = File::open(path)?;
            BufReader::new(f)
                .lines()
                .map_while(|l| l.ok())
                .filter_map(|l| serde_json::from_str::<JournalRecord>(&l).ok())
                .map(|r| r.seq)
                .max()
                .unwrap_or(0)
        } else {
            0
        };
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Journal {
            file,
            next_seq: highest + 1,
            path: path.to_path_buf(),
        })
    }

    /// Record a mutation *before* it is applied. Returns the new record's `seq`.
    pub fn record(
        &mut self,
        op: Op,
        ctx: &MutationCtx,
        target: serde_json::Value,
        new: serde_json::Value,
        inverse: serde_json::Value,
    ) -> Result<u64> {
        let seq = self.next_seq;
        self.next_seq += 1;
        let rec = JournalRecord {
            seq,
            ts_unix_ms: now_ms(),
            run_id: ctx.run_id.clone(),
            scenario_id: ctx.scenario_id.clone(),
            step_index: ctx.step_index,
            op,
            target,
            new,
            inverse,
            applied: false,
        };
        writeln!(self.file, "{}", serde_json::to_string(&rec)?)?;
        self.file.sync_all()?;
        Ok(seq)
    }

    /// Confirm that the mutation recorded as `seq` succeeded.
    pub fn mark_applied(&mut self, seq: u64) -> Result<()> {
        writeln!(
            self.file,
            "{}",
            serde_json::json!({ "seq": seq, "applied": true })
        )?;
        self.file.sync_all()?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Parse every record in `path`, with later `applied: true` overlay lines
    /// merged in. A record with no matching overlay stays `applied: false`.
    pub fn load_pending(path: &Path) -> Result<Vec<JournalRecord>> {
        if !path.exists() {
            return Ok(vec![]);
        }
        let f = File::open(path)?;
        let mut records: Vec<JournalRecord> = Vec::new();
        for line in BufReader::new(f).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(rec) = serde_json::from_str::<JournalRecord>(&line) {
                records.push(rec);
            } else if let Ok(ov) = serde_json::from_str::<AppliedOverlay>(&line)
                && let Some(r) = records.iter_mut().find(|r| r.seq == ov.seq)
            {
                r.applied = ov.applied;
            }
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::MutationCtx;
    use serde_json::json;

    fn ctx() -> MutationCtx {
        MutationCtx {
            run_id: "r".into(),
            scenario_id: "s".into(),
            step_index: 0,
        }
    }

    #[test]
    fn records_two_phase_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("journal.jsonl");
        let mut j = Journal::open(&p).unwrap();
        let seq = j
            .record(
                Op::PowercfgWrite,
                &ctx(),
                json!({"sub":"sub_processor","setting":"IDLEDISABLE"}),
                json!(1),
                json!({"value":0}),
            )
            .unwrap();
        assert_eq!(seq, 1);
        j.mark_applied(seq).unwrap();
        let recs = Journal::load_pending(&p).unwrap();
        assert_eq!(recs.len(), 1);
        assert!(recs[0].applied);
        assert_eq!(recs[0].seq, 1);
    }

    #[test]
    fn open_continues_seq_after_reload() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("j.jsonl");
        {
            let mut j = Journal::open(&p).unwrap();
            j.record(Op::RegistryWrite, &ctx(), json!({}), json!({}), json!({}))
                .unwrap();
        }
        let mut j2 = Journal::open(&p).unwrap();
        let seq = j2
            .record(Op::RegistryWrite, &ctx(), json!({}), json!({}), json!({}))
            .unwrap();
        assert_eq!(seq, 2);
    }

    #[test]
    fn unapplied_record_reports_applied_false() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("j.jsonl");
        let mut j = Journal::open(&p).unwrap();
        j.record(Op::RegistryWrite, &ctx(), json!({}), json!({}), json!({}))
            .unwrap();
        let recs = Journal::load_pending(&p).unwrap();
        assert!(!recs[0].applied);
    }
}

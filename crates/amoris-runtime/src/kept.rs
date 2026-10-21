//! Shared native/browser snapshot retention and script-preserving restore policy.
use crate::RestoreBundle;
use amoris_contract::{Problem, detail};
use amoris_persist::Snapshot;
use serde_json::{Value, json};
use std::collections::VecDeque;

/// The kept snapshots of one world (docs/spec/server.md, snapshots): one every `every` ticks, the
/// last `keep` of them.
pub struct KeptSnapshots {
    pub(crate) every: u64,
    keep: usize,
    pub(crate) items: VecDeque<Snapshot>,
}

impl KeptSnapshots {
    pub fn new(every: u64, keep: usize) -> KeptSnapshots {
        KeptSnapshots {
            every,
            keep,
            items: VecDeque::new(),
        }
    }

    /// Keeps `s` if its tick is due (or `always`, the world's first state).
    pub fn offer(&mut self, s: &Snapshot, always: bool) {
        let tick = s.header().tick.0;
        let due = self.every > 0 && tick.is_multiple_of(self.every);
        if !(due || always) || self.keep == 0 {
            return;
        }
        if self.items.back().is_some_and(|b| b.header().tick.0 >= tick) {
            self.drop_after(tick.saturating_sub(1));
        }
        if self.items.len() == self.keep {
            self.items.pop_front();
        }
        self.items.push_back(s.clone());
    }

    pub fn at_or_before(&self, tick: u64) -> Option<Snapshot> {
        self.items
            .iter()
            .rev()
            .find(|s| s.header().tick.0 <= tick)
            .cloned()
    }

    pub fn drop_after(&mut self, tick: u64) {
        while self.items.back().is_some_and(|s| s.header().tick.0 > tick) {
            self.items.pop_back();
        }
    }

    pub fn ticks(&self) -> Vec<u64> {
        self.items.iter().map(|s| s.header().tick.0).collect()
    }

    pub fn to_json(&self) -> Value {
        let list: Vec<Value> = self
            .items
            .iter()
            .map(|s| json!({"tick": s.header().tick.0, "world_hash": s.world_hash().to_string()}))
            .collect();
        json!({"every": self.every, "keep": self.keep, "snapshots": list})
    }
    pub fn due(&self, tick: u64) -> bool {
        self.every > 0 && tick.is_multiple_of(self.every)
    }

    /// Restore the retained world's own scripts, then optionally reapply the current bundle.
    /// The caller owns loop pacing, pending replies and publication of the replaced world.
    pub fn restore(
        &mut self,
        game: &mut crate::Game,
        p: &crate::SnapshotsRestoreParams,
    ) -> Result<Value, Problem> {
        let Some(snap) = self.at_or_before(p.tick) else {
            return Err(Problem::new(
                "snapshots.none",
                format!("No kept snapshot is at or before tick {}.", p.tick),
                detail([("tick", json!(p.tick)), ("kept", json!(self.ticks()))]),
            ));
        };
        let applied = game.bundle();
        game.restore_any(&snap)?;
        let at = snap.header().tick.0;
        let kept_with = snap.header().bundle;
        let mut scripts = json!({
            "bundle": kept_with.to_hex(),
            "kept": "snapshot",
            "snapshot_bundle": kept_with.to_hex(),
        });
        if p.bundle == RestoreBundle::Applied {
            if kept_with == applied {
                scripts["kept"] = json!("applied");
            } else {
                match game.swap_bundle(applied) {
                    Ok(_) => {
                        scripts["kept"] = json!("applied");
                        scripts["bundle"] = json!(applied.to_hex());
                        scripts["swapped"] = json!(true);
                    }
                    // The applied scripts do not load on this world: it runs the snapshot's.
                    Err(e) => {
                        scripts["swap_refused"] = serde_json::to_value(&e).unwrap_or(Value::Null);
                    }
                }
            }
        }
        self.drop_after(at);
        Ok(json!({"restored": at, "scripts": scripts}))
    }
}

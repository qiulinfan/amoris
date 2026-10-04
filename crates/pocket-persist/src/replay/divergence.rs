//! The first diverging tick (docs/spec/replay.md 3.1) and lockstep (3.3).

use pocket_contract::Problem;
use pocket_sim::{SimClock, Tick};
use serde::{Deserialize, Serialize};

use super::{RecordedWrite, Stepper, TickRef};
use crate::diff::{DEFAULT_LIMIT, FieldDiff, Side, diff_with};
use crate::hash::{SectionKey, TickHash};
use crate::snapshot::{snapshot, world_hash};
use crate::version::FormatTable;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum DivergenceKind {
    /// The tick hashes differ.
    Hash,
    /// One side refused a write the other applied.
    Write {
        index: u32,
        refused_by: Side,
    },
    EndedEarly {
        side: Side,
    },
    /// A fault, a taint, a missing bundle or data file.
    Stopped {
        error: Problem,
    },
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Divergence {
    pub at: TickRef,
    pub kind: DivergenceKind,
    pub expected: Option<TickHash>,
    pub actual: Option<TickHash>,
    /// Differing sections; empty when unknown.
    pub sections: Vec<SectionKey>,
    /// Empty when unavailable.
    pub fields: Vec<FieldDiff>,
    pub fields_truncated: bool,
    /// Applied at boundary `tick - 1`.
    pub writes: Vec<RecordedWrite>,
    pub reconverged_at: Option<TickRef>,
}

impl Divergence {
    pub(crate) fn new(at: TickRef, kind: DivergenceKind) -> Divergence {
        Divergence {
            at,
            kind,
            expected: None,
            actual: None,
            sections: Vec::new(),
            fields: Vec::new(),
            fields_truncated: false,
            writes: Vec::new(),
            reconverged_at: None,
        }
    }
}

/// The first `TickRef` where the two runs' hashes differ, or where one run ends before the other.
/// Both lists are in recording order (segments, then ticks); a tick hash does not chain earlier
/// ticks, so a difference that later disappears is reported with `reconverged_at`.
pub fn first_divergence(
    expected: &[(TickRef, TickHash)],
    actual: &[(TickRef, TickHash)],
) -> Option<Divergence> {
    let n = expected.len().min(actual.len());
    let first = (0..n).find(|&i| expected[i].0 != actual[i].0 || expected[i].1 != actual[i].1);
    let Some(i) = first else {
        return match expected.len().cmp(&actual.len()) {
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => {
                let mut d = Divergence::new(
                    expected[n].0,
                    DivergenceKind::EndedEarly { side: Side::Actual },
                );
                d.expected = Some(expected[n].1);
                Some(d)
            }
            std::cmp::Ordering::Less => {
                let mut d = Divergence::new(
                    actual[n].0,
                    DivergenceKind::EndedEarly {
                        side: Side::Expected,
                    },
                );
                d.actual = Some(actual[n].1);
                Some(d)
            }
        };
    };
    let (ea, aa) = (expected[i], actual[i]);
    if ea.0 != aa.0 {
        // The runs name different ticks here: one ended its segment early.
        let side = if ea.0 > aa.0 {
            Side::Expected
        } else {
            Side::Actual
        };
        let at = ea.0.min(aa.0);
        return Some(Divergence::new(at, DivergenceKind::EndedEarly { side }));
    }
    let mut d = Divergence::new(ea.0, DivergenceKind::Hash);
    d.expected = Some(ea.1);
    d.actual = Some(aa.1);
    d.reconverged_at = (i + 1..n)
        .find(|&j| expected[j].0 == actual[j].0 && expected[j].1 == actual[j].1)
        .map(|j| expected[j].0);
    Some(d)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LockstepOptions {
    /// The last tick to run.
    pub until: Tick,
    /// Default 50.
    pub field_limit: usize,
    pub stop_at_first: bool,
}

impl LockstepOptions {
    pub fn until(until: Tick) -> LockstepOptions {
        LockstepOptions {
            until,
            field_limit: DEFAULT_LIMIT,
            stop_at_first: true,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct LockstepReport {
    pub ticks_run: u64,
    pub divergence: Option<Divergence>,
}

fn tick_of(s: &dyn Stepper) -> Tick {
    s.world()
        .get_resource::<SimClock>()
        .map_or(Tick(0), |c| c.tick)
}

/// Snapshots both sides and names the differing sections and fields.
pub(crate) fn locate(
    a: &dyn Stepper,
    b: &dyn Stepper,
    d: &mut Divergence,
    limit: usize,
) -> Result<(), Problem> {
    let (sa, sb) = (
        snapshot(a.world(), a.registry())?,
        snapshot(b.world(), b.registry())?,
    );
    let table = FormatTable::of(a.registry(), a.world())?;
    let found = diff_with(&sa, &sb, &table, limit)?;
    d.sections = found.keys();
    d.fields = found.fields;
    d.fields_truncated = found.truncated;
    Ok(())
}

fn stopped(at: TickRef, error: Problem) -> LockstepReport {
    LockstepReport {
        ticks_run: 0,
        divergence: Some(Divergence::new(at, DivergenceKind::Stopped { error })),
    }
}

/// Runs two steppers from the same boundary through `until`: for each tick t, each side applies
/// `writes(t, side)` through the live path, steps and hashes; on a difference both are snapshotted
/// and diffed. Side a is `Expected`. `version.schema_differs` (as a `Stopped` divergence) when the
/// registries differ.
pub fn lockstep(
    a: &mut dyn Stepper,
    b: &mut dyn Stepper,
    writes: &mut dyn FnMut(Tick, Side) -> Vec<RecordedWrite>,
    opts: LockstepOptions,
) -> LockstepReport {
    let at0 = TickRef {
        segment: 0,
        tick: tick_of(a),
    };
    let (sa, sb) = (a.registry().schema(), b.registry().schema());
    if sa != sb {
        let names: Vec<String> = sa
            .iter()
            .filter(|e| !sb.contains(e))
            .chain(sb.iter().filter(|e| !sa.contains(e)))
            .map(|e| e.0.to_string())
            .collect();
        return stopped(at0, crate::error::schema_differs(&names));
    }
    let mut report = LockstepReport {
        ticks_run: 0,
        divergence: None,
    };
    let mut found: Option<Divergence> = None;
    loop {
        let t = Tick(tick_of(a).0 + 1);
        if t > opts.until {
            break;
        }
        let at = TickRef {
            segment: 0,
            tick: t,
        };
        let (wa, wb) = (writes(t, Side::Expected), writes(t, Side::Actual));
        let ra: Vec<bool> = wa.iter().map(|w| a.apply(w, &()).is_ok()).collect();
        let rb: Vec<bool> = wb.iter().map(|w| b.apply(w, &()).is_ok()).collect();
        let refused = ra.iter().zip(&rb).position(|(x, y)| x != y);
        let step = a.step().and_then(|_| b.step());
        report.ticks_run += 1;
        if let Err(e) = step {
            let mut d = Divergence::new(at, DivergenceKind::Stopped { error: e });
            d.writes = wa;
            report.divergence = found.or(Some(d));
            return report;
        }
        let (ha, hb) = (
            world_hash(a.world(), a.registry()),
            world_hash(b.world(), b.registry()),
        );
        let (ha, hb) = match (ha, hb) {
            (Ok(x), Ok(y)) => (x, y),
            (Err(e), _) | (_, Err(e)) => {
                report.divergence = found.or(Some(Divergence::new(
                    at,
                    DivergenceKind::Stopped { error: e },
                )));
                return report;
            }
        };
        if let Some(d) = &mut found {
            if ha == hb && d.reconverged_at.is_none() {
                d.reconverged_at = Some(at);
            }
            continue;
        }
        let kind = match refused {
            Some(i) => DivergenceKind::Write {
                index: u32::try_from(i).unwrap_or(u32::MAX),
                refused_by: if ra[i] { Side::Actual } else { Side::Expected },
            },
            None if ha != hb => DivergenceKind::Hash,
            None => continue,
        };
        let mut d = Divergence::new(at, kind);
        d.expected = Some(ha);
        d.actual = Some(hb);
        d.writes = wa;
        if ha != hb
            && let Err(e) = locate(&*a, &*b, &mut d, opts.field_limit)
        {
            d.kind = DivergenceKind::Stopped { error: e };
        }
        if opts.stop_at_first {
            report.divergence = Some(d);
            return report;
        }
        found = Some(d);
    }
    report.divergence = found;
    report
}

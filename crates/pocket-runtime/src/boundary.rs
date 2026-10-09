//! What every loop over a [`crate::Game`] does at its boundaries, on every target: the native game
//! thread ([`crate::thread`]) and the browser's worker (`pocket-web`'s `worker.rs`) share it, so the
//! two loops keep one rule (docs/spec/threads.md 5.2 and 7.3, docs/spec/player.md 6): which held
//! commands are due, and the players' waits, decision points and pace.

use std::mem;

use pocket_contract::Problem;
use pocket_interface::time::play::PlayPacing;
use pocket_interface::{Pace, Pacing, TimeModel};
use pocket_link::Envelope;
use pocket_sim::{StepReport, Tick};
use serde_json::Value;

use crate::catalog::Command;
use crate::game::{Game, PlayerRun, PlayerStart};

/// Takes from `held` the envelopes due at the boundary before tick `next` and those whose tick has
/// passed, leaving the later ones held (threads.md 5.2: held envelopes are matched by tick like
/// queued ones, so one whose tick a replaced world skipped is answered `command.tick_passed` and
/// its place freed rather than stranded).
pub fn take_held(held: &mut Vec<Envelope>, next: Tick) -> (Vec<Envelope>, Vec<Envelope>) {
    let mut due = Vec::new();
    let mut passed = Vec::new();
    for e in mem::take(held) {
        match e.at {
            Some(t) if t > next => held.push(e),
            Some(t) if t < next => passed.push(e),
            _ => due.push(e),
        }
    }
    (due, passed)
}

/// A players' wait that ended: where its answer goes, and the answer.
pub type Answer<R> = (R, Result<Value, Problem>);

/// The players' waits under way on a loop (docs/spec/player.md 6), each with where its answer goes
/// (the game thread's reply, the worker's source and seq). A stepped wait asks the loop's time
/// model for its ticks ([`PlayerWaits::owed`]) and counts them as they run, so the loop serves its
/// queue between them; a real-time one waits for its seat's decision while the clock runs the
/// ticks; both end at their wall limit. After every tick the loop runs, whatever ran it (a
/// developer's step, real time, a wait), [`PlayerWaits::after_tick`] computes the players'
/// decision points once; for a game with players, the boundary's pace comes from their session
/// ([`PlayerWaits::pace`]): a halt, the episode's end, real time held by a pending decision.
pub struct PlayerWaits<R> {
    runs: Vec<PlayerRun>,
    replies: Vec<R>,
}

impl<R> Default for PlayerWaits<R> {
    fn default() -> Self {
        PlayerWaits {
            runs: Vec::new(),
            replies: Vec::new(),
        }
    }
}

impl<R> PlayerWaits<R> {
    /// Whether no wait is under way.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// The ticks the stepped waits still ask for: the most any one does, since they share the
    /// ticks the model runs (as a developer's step under way does).
    pub fn owed(&self) -> u64 {
        self.runs
            .iter()
            .map(PlayerRun::ticks_left)
            .max()
            .unwrap_or(0)
    }

    /// `player.wait` (time.md, Requests): answered at once (a poisoned world, a refused request, a
    /// decision already pending in real time, no tick to run), or held to run, in which case the
    /// loop asks its model again for what is owed.
    pub fn begin(&mut self, game: &mut Game, cmd: &Command, reply: R) -> Option<Answer<R>> {
        if let Some(poison) = game.sim().poisoned() {
            return Some((reply, Err(pocket_sim::sim::world_poisoned(poison.tick))));
        }
        match game.begin_player_wait(cmd) {
            Ok(PlayerStart::Answered(v)) => Some((reply, Ok(v))),
            Ok(PlayerStart::Running(run)) => {
                self.runs.push(run);
                self.replies.push(reply);
                None
            }
            Err(p) => Some((reply, Err(p))),
        }
    }

    /// At a boundary, before the pace: the waits past their wall limit, and the stepped ones that
    /// cannot take the next tick (the episode ended, a halt), end; their answers go into `ended`.
    /// The loop then asks its model again for what is owed, before [`PlayerWaits::pace`].
    pub fn end_due(&mut self, game: &mut Game, now_ms: f64, ended: &mut Vec<Answer<R>>) {
        let mut done = Vec::new();
        for (i, r) in self.runs.iter_mut().enumerate() {
            if now_ms >= r.wall_deadline() {
                r.stop_at_wall();
                done.push(i);
            } else if !r.real_time() && !r.before_tick(game) {
                done.push(i);
            }
        }
        self.finish(game, done, ended);
    }

    /// The answer at this boundary: for a game with players their session decides, else the
    /// model; a developer's step under way (`stepping`) runs its ticks whatever holds the players.
    /// The earliest wall limit of the waits still under way wakes the loop.
    pub fn pace(
        &self,
        game: &mut Game,
        model: &mut TimeModel,
        stepping: bool,
        now_ms: f64,
    ) -> Pace {
        let pace = if game.has_players() && !stepping {
            game.player_pace(model, now_ms)
        } else {
            model.pace(now_ms)
        };
        let deadline = self
            .runs
            .iter()
            .map(PlayerRun::wall_deadline)
            .reduce(pocket_sim::math::min);
        match (pace, deadline) {
            (Pace::WaitForCommand, Some(d)) => Pace::WaitUntil(d),
            (Pace::WaitUntil(t), Some(d)) => Pace::WaitUntil(pocket_sim::math::min(t, d)),
            (p, _) => p,
        }
    }

    /// After a tick the loop ran: the players' decision points, computed once for the tick, and
    /// each wait's stop; the answers of those that ended go into `ended`.
    pub fn after_tick(
        &mut self,
        game: &mut Game,
        report: &StepReport,
        now_ms: f64,
        ended: &mut Vec<Answer<R>>,
    ) {
        let done = game.player_after_tick(report, &mut self.runs, now_ms);
        self.finish(game, done, ended);
    }

    /// Every wait under way, taken out to be answered with why they cannot go on (their world went
    /// away, or a tick failed).
    pub fn drain(&mut self) -> Vec<R> {
        self.runs.clear();
        mem::take(&mut self.replies)
    }

    /// Answers the waits at these indices (ascending).
    fn finish(&mut self, game: &mut Game, done: Vec<usize>, ended: &mut Vec<Answer<R>>) {
        for i in done.into_iter().rev() {
            let run = self.runs.remove(i);
            let reply = self.replies.remove(i);
            ended.push((reply, game.finish_player_run(run)));
        }
    }
}

/// `player.pacing` on a loop (docs/spec/player.md 6): the players' pacing, and the loop's model
/// to time it: real time runs at the pacing's speed, unpaused; stepped waits for steps and waits.
pub fn player_pacing(
    game: &mut Game,
    model: &mut TimeModel,
    cmd: &Command,
) -> Result<Value, Problem> {
    let (pacing, v) = game.set_player_pacing(cmd)?;
    match pacing {
        PlayPacing::RealTime { speed, .. } => {
            model.set_pacing(Pacing::RealTime { speed });
            model.resume();
        }
        _ => model.set_pacing(Pacing::Stepped),
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use pocket_link::{ReplyTo, Source};
    use serde_json::json;

    use super::*;

    fn envelope(seq: u64, at: Option<u64>) -> Envelope {
        Envelope {
            source: Source::Player(0),
            seq,
            at: at.map(Tick),
            name: "status".into(),
            params: json!({}),
            reply: ReplyTo::none(),
        }
    }

    /// threads.md 5.2: at the boundary before tick 10, a held envelope of tick 10 is due, one of a
    /// later tick stays held, and one of an earlier tick, which a world replaced at a later tick
    /// would otherwise strand, is taken out to be answered `command.tick_passed`.
    #[test]
    fn held_envelopes_whose_tick_passed_are_taken_out() {
        let mut held = vec![
            envelope(1, Some(4)),
            envelope(2, Some(10)),
            envelope(3, Some(12)),
            envelope(4, Some(9)),
        ];
        let (due, passed) = take_held(&mut held, Tick(10));
        let seqs = |v: &[Envelope]| v.iter().map(|e| e.seq).collect::<Vec<_>>();
        assert_eq!(seqs(&due), [2]);
        assert_eq!(seqs(&passed), [1, 4]);
        assert_eq!(seqs(&held), [3]);
        let (due, passed) = take_held(&mut held, Tick(12));
        assert_eq!(
            (seqs(&due), seqs(&passed), held.len()),
            (vec![3], vec![], 0)
        );
    }
}

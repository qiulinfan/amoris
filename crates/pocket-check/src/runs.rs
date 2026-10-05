//! A project under check (docs/spec/checks.md 8.1): its configuration, its game data, its inputs,
//! and the runs every check is made of. The runs drive `Game` synchronously on the caller's thread,
//! hashing every tick.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pocket_contract::{Problem, detail};
use pocket_persist::replay::TickRef;
use pocket_persist::{Divergence, WorldHash};
use pocket_runtime::{Command, Game, GameSetup};
use pocket_sim::Tick;
use serde_json::{Value, json};

use crate::config::{CheckToml, Inputs};

/// How a check builds a game of the project for a seed: `Game::new` for projects, a game with a
/// defect for the Rust-side negative controls (checks.md 8.6).
pub type Build = Arc<dyn Fn(&Arc<GameSetup>, u64) -> Result<Game, Problem> + Send + Sync>;

/// A hash per tick: the start (tick 0) and every tick after it.
pub type Chain = Vec<(TickRef, WorldHash)>;

/// A project under check.
#[derive(Clone)]
pub struct Subject {
    /// The project's directory as given, which problems name.
    pub name: String,
    pub dir: PathBuf,
    pub config: CheckToml,
    pub setup: Arc<GameSetup>,
    pub inputs: Inputs,
    /// The branch's own inputs, their ticks relative to the fork point.
    pub branch: Inputs,
    pub build: Build,
}

impl Subject {
    /// A project read from `dir`: `check.toml`, its inputs, and its scripts compiled (with the lint
    /// off only when `[expect] lint = false`).
    #[cfg(feature = "native")]
    pub fn load(dir: &Path) -> Result<Subject, Problem> {
        let config = CheckToml::load(dir)?;
        let project = pocket_runtime::Project::load(dir)?;
        let lint_off = config.expect.as_ref().is_some_and(|e| !e.lint);
        let setup = Arc::new(project.setup(lint_off)?);
        Subject::new(dir, config, setup)
    }

    /// A subject of an already built setup (the web page, tests).
    pub fn new(dir: &Path, config: CheckToml, setup: Arc<GameSetup>) -> Result<Subject, Problem> {
        let inputs = Inputs::load(dir, config.run.inputs.as_deref())?;
        let branch = Inputs::load(
            dir,
            config
                .fork
                .as_ref()
                .and_then(|f| f.branch_inputs.as_deref()),
        )?;
        Ok(Subject {
            name: dir.display().to_string().replace('\\', "/"),
            dir: dir.to_path_buf(),
            config,
            setup,
            inputs,
            branch,
            build: Arc::new(|setup, seed| Game::new(setup.clone(), seed)),
        })
    }

    /// A game of the project at tick 0.
    pub fn game(&self, seed: u64) -> Result<Game, Problem> {
        (self.build)(&self.setup, seed)
    }

    pub fn ticks(&self) -> u64 {
        self.config.run.ticks
    }
}

/// `check.input_refused {line, error}`: an input the game refused.
pub fn input_refused(line: usize, tick: u64, error: &Problem) -> Problem {
    Problem::new(
        "check.input_refused",
        format!(
            "The input on line {line} (tick {tick}) was refused: {}",
            error.message
        ),
        detail([
            ("line", json!(line)),
            ("tick", json!(tick)),
            ("error", serde_json::to_value(error).unwrap_or(Value::Null)),
        ]),
    )
}

/// Applies the inputs of tick `tick` (at boundary `tick - 1`).
pub fn apply_inputs(game: &mut Game, inputs: &Inputs, tick: u64) -> Result<(), Problem> {
    for i in inputs.at(tick) {
        let cmd = Command::new(i.source, i.seq, &i.name, i.params.clone());
        game.apply(&cmd)
            .map_err(|e| input_refused(i.line, tick, &e))?;
    }
    Ok(())
}

/// The point of tick `tick` in the first segment.
pub fn at(tick: u64) -> TickRef {
    TickRef {
        segment: 0,
        tick: Tick(tick),
    }
}

/// Steps `game` one tick with the inputs of that tick; its hash after.
pub fn advance(game: &mut Game, inputs: &Inputs) -> Result<WorldHash, Problem> {
    let t = game.tick().0 + 1;
    apply_inputs(game, inputs, t)?;
    game.step()?;
    game.world_hash()
}

/// A run of `ticks` ticks of seed `seed` with the project's inputs: its hash chain.
pub fn chain(subject: &Subject, seed: u64, ticks: u64) -> Result<Chain, Problem> {
    let mut g = subject.game(seed)?;
    let mut out = Vec::with_capacity(usize::try_from(ticks).unwrap_or(0) + 1);
    out.push((at(0), g.world_hash()?));
    for t in 1..=ticks {
        out.push((at(t), advance(&mut g, &subject.inputs)?));
    }
    Ok(out)
}

/// A game of seed `seed` run to tick `tick` with the inputs.
pub fn run_to(subject: &Subject, seed: u64, tick: u64) -> Result<Game, Problem> {
    let mut g = subject.game(seed)?;
    while g.tick().0 < tick {
        advance(&mut g, &subject.inputs)?;
    }
    Ok(g)
}

/// A divergence as problem detail.
pub fn divergence_json(d: &Divergence) -> Value {
    serde_json::to_value(d).unwrap_or(Value::Null)
}

/// A divergence of two runs at `tick`, its sections and fields found by diffing their snapshots.
pub fn diverged(
    a: &Game,
    b: &Game,
    tick: u64,
    expected: WorldHash,
    actual: WorldHash,
) -> Divergence {
    let mut d = Divergence {
        at: at(tick),
        kind: pocket_persist::DivergenceKind::Hash,
        expected: Some(expected),
        actual: Some(actual),
        sections: Vec::new(),
        fields: Vec::new(),
        fields_truncated: false,
        writes: Vec::new(),
        reconverged_at: None,
    };
    if let (Ok(sa), Ok(sb)) = (a.snapshot(), b.snapshot())
        && let Ok(table) = pocket_persist::FormatTable::of(a.registry(), a.world())
        && let Ok(found) =
            pocket_persist::diff::diff_with(&sa, &sb, &table, pocket_persist::diff::DEFAULT_LIMIT)
    {
        d.sections = found.keys();
        d.fields = found.fields;
        d.fields_truncated = found.truncated;
    }
    d
}

/// The step's problem for a run that could not finish: it carries what stopped it.
pub fn stopped(check: &str, subject: &Subject, seed: u64, p: Problem) -> Problem {
    if p.code.starts_with("check.") {
        let mut p = p;
        p.detail.insert("project".into(), json!(subject.name));
        p.detail.insert("seed".into(), json!(seed));
        return p;
    }
    Problem::new(
        &format!("{check}.run_failed"),
        format!(
            "The {check} run of {} with seed {seed} stopped: {}",
            subject.name, p.message
        ),
        detail([
            ("project", json!(subject.name)),
            ("seed", json!(seed)),
            ("error", serde_json::to_value(&p).unwrap_or(Value::Null)),
        ]),
    )
}

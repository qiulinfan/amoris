//! Team vision (charter 5.1, Pioneer 2026-10-09; docs/spec/player.md): observers of one team share
//! what they see. A watcher whose view a wall blocks sees a thing its teammate sees past the wall,
//! with the teammate's detail; the sighting reaches the watcher's own ring; a watcher of another
//! team, or of none, does not see it; and each keeps its own memory, so when the teammate looks
//! away the thing is remembered, then forgotten on the watcher's own memory rule.
mod perception_support;

use perception_support::*;
use pocket_interface::perception::Observer;
use pocket_interface::perception::state::Detail;
use pocket_physics::Transform;
use pocket_sim::{EntityId, Name};

fn member(
    sim: &mut pocket_sim::Sim,
    name: &str,
    at: [f64; 3],
    heading: f64,
    team: Option<&str>,
) -> EntityId {
    let mut b = sim.boundary();
    b.spawn((
        Name::new(name).expect("a name"),
        Transform::at_yaw(at, -heading),
        Observer {
            profile: "watch".into(),
            seat: None,
            omniscient: false,
            team: team.map(str::to_owned),
        },
    ))
    .expect("spawns")
}

#[test]
fn a_team_sees_what_any_member_sees_and_each_remembers_alone() {
    let mut sim = sim();
    // The watcher at the origin faces north (-z); a wall 10 m north hides the thing 40 m north.
    let me = member(&mut sim, "Me", [0.0, 0.0, 0.0], 0.0, Some("blue"));
    wall(&mut sim, [0.0, 0.0, -10.0], [5.0, 5.0, 0.5]);
    let target = thing(&mut sim, "Target", [0.0, 0.0, -40.0], 100.0, 1.0);
    // A lookout of the same team 30 m east sees it, facing north-west; a rival of another team
    // stands where the watcher stands; a loner of no team beside it.
    let mate = member(&mut sim, "Mate", [30.0, 0.0, -10.0], 315.0, Some("blue"));
    let rival = member(&mut sim, "Rival", [0.0, 0.0, 1.0], 0.0, Some("red"));
    let loner = member(&mut sim, "Loner", [1.0, 0.0, 1.0], 0.0, None);
    step(&mut sim);
    assert!(remembered(&sim, mate).contains(&target), "the mate sees it");
    let mine = memory(&sim, me);
    let entry = mine.entries.get(&target).expect("seen through the mate");
    assert_eq!(
        entry.seen_tick, mine.updated,
        "seen this tick, not remembered"
    );
    // The mate is 31.6 m from it, inside 50 m of attention: full detail, with the full fact.
    assert_eq!(entry.detail, Detail::Full);
    assert!(entry.facts.contains_key("x_m"), "{:?}", entry.facts);
    assert!(kinds(&sim, me).contains(&"sighted".to_owned()));
    assert!(!remembered(&sim, rival).contains(&target), "another team's");
    assert!(!remembered(&sim, loner).contains(&target), "no team's");
    // The mate turns away: the watcher remembers the thing (the wall hides its place from it),
    // until its own one second of memory runs out.
    turn(&mut sim, mate, 135.0);
    step(&mut sim);
    let mine = memory(&sim, me);
    let entry = mine.entries.get(&target).expect("remembered");
    assert!(entry.seen_tick < mine.updated, "no longer seen");
    steps(&mut sim, 61);
    assert!(
        !remembered(&sim, me).contains(&target),
        "forgotten after memory_s"
    );
}

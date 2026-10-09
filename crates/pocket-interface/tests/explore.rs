//! Explorations of the sailing intents against the slice 1 boat (ignored; run with --ignored
//! --nocapture to see the numbers the tests' limits come from).

mod common;

use common::*;
use serde_json::json;

#[test]
#[ignore]
fn explore_polar() {
    for twa in [45.0f64, 50.0, 55.0, 60.0, 65.0, 70.0, 80.0, 90.0] {
        let heading = 270.0 + twa;
        let (mut sim, id) = world(0.0, 270.0, 6.0, true, 1);
        act(
            &mut sim,
            json!({"actions": [{"do": "start", "intent": "trim_sail", "params": {"keep": true}}]}),
        )
        .unwrap();
        act(&mut sim, json!({"actions": [{"do": "start", "intent": "come_to_heading", "params": {"heading_deg": 0, "keep": true}}]})).unwrap();
        for _ in 0..900 {
            step(&mut sim);
        }
        act(&mut sim, json!({"actions": [{"do": "start", "intent": "come_to_heading", "params": {"heading_deg": heading % 360.0, "keep": true}}]})).unwrap();
        for _ in 0..3600 {
            step(&mut sim);
        }
        let b = boat(&sim, id);
        let vmg = b.speed * pocket_sim::math::cos(twa.to_radians());
        println!(
            "twa {twa}: heading {:.1} speed {:.2} vmg {:.2} awa {:.1} drive {:.2} trim {:?} sheet {:.2}",
            b.heading_deg, b.speed, vmg, b.awa_deg, b.drive, b.trim, b.sheet_now
        );
    }
}

#[test]
#[ignore]
fn explore_tack() {
    let (mut sim, id) = world(0.0, 270.0, 6.0, true, 1);
    act(
        &mut sim,
        json!({"actions": [{"do": "start", "intent": "trim_sail", "params": {"keep": true}}]}),
    )
    .unwrap();
    act(&mut sim, json!({"actions": [{"do": "start", "intent": "come_to_heading", "params": {"heading_deg": 330, "keep": true}}]})).unwrap();
    for _ in 0..2400 {
        step(&mut sim);
    }
    let done = act(&mut sim, json!({"actions": [{"do": "start", "intent": "come_to_heading", "params": {"heading_deg": 210}}]})).unwrap();
    let iid = started(&done);
    for t in 0..3600 {
        step(&mut sim);
        let i = intent(&sim, iid);
        if t % 30 == 0 {
            let b = boat(&sim, id);
            println!(
                "t {t} hdg {:.1} spd {:.2} rud {:.2}/{:.2} awa {:.1} drive {:.2} sheet {:.2}",
                b.heading_deg, b.speed, b.rudder, b.rudder_now, b.awa_deg, b.drive, b.sheet_now
            );
        }
        if i.status != pocket_interface::action::IntentStatus::Active {
            println!("{:?} at {t} {:?}", i.status, i.failure);
            break;
        }
    }
}

#[test]
#[ignore]
fn explore_beat() {
    let (mut sim, id) = world(0.0, 270.0, 6.0, true, 1);
    act(
        &mut sim,
        json!({"actions": [{"do": "start", "intent": "trim_sail", "params": {"keep": true}}]}),
    )
    .unwrap();
    act(&mut sim, json!({"actions": [{"do": "start", "intent": "come_to_heading", "params": {"heading_deg": 0, "keep": true}}]})).unwrap();
    for _ in 0..900 {
        step(&mut sim);
    }
    let at = pos(&sim, id);
    mark(&mut sim, "Mark1", [at[0] - 500.0, 0.0, at[2]]);
    let done = act(&mut sim, json!({"actions": [{"do": "start", "intent": "sail_to", "target": "Mark1", "params": {"timeout_s": 3600}}]})).unwrap();
    let iid = started(&done);
    let mut last_leg = String::new();
    for t in 0..216000 {
        step(&mut sim);
        let i = intent(&sim, iid);
        let leg = i
            .progress
            .iter()
            .find(|(n, _)| n == "leg")
            .map(|(_, v)| format!("{:?}", v))
            .unwrap_or_default();
        if t % 1200 == 0 || leg != last_leg {
            let b = boat(&sim, id);
            let p = pos(&sim, id);
            println!(
                "t {t} pos ({:.0},{:.0}) hdg {:.0} spd {:.2} dist {:?} leg {leg} tacks {:?}",
                p[0],
                p[2],
                b.heading_deg,
                b.speed,
                reading(&i, "distance_m"),
                reading(&i, "tacks")
            );
            last_leg = leg;
        }
        if i.status != pocket_interface::action::IntentStatus::Active {
            println!("{:?} at {t} {:?}", i.status, i.failure);
            break;
        }
    }
}

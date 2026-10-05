//! The sailing simulation (charter 2.4.1; slice 1's acceptance): master's `sailboat` task, the
//! Sloop's points of sail, the rudder's sense, buoyancy at rest, and contacts with crates.

use pocket_physics::probe::{sailboat_world, sailing_world};
use pocket_physics::{Boat, Collider, RigidBody, Shape, Transform, Velocity};
use pocket_sim::{EntityId, NoHooks, Sim, entity};

fn boat(sim: &Sim, id: EntityId) -> (Transform, Velocity, Boat) {
    let e = entity::require(sim.world(), id).unwrap();
    let w = sim.world();
    (
        *w.get::<Transform>(e).unwrap(),
        *w.get::<Velocity>(e).unwrap(),
        *w.get::<Boat>(e).unwrap(),
    )
}

fn line(sim: &Sim, id: EntityId) -> String {
    let (tf, v, b) = boat(sim, id);
    format!(
        "tick {:4} pos ({:7.2} {:5.2} {:7.2}) v ({:5.2} {:5.2} {:5.2}) heading {:6.1} heel {:5.1} \
         speed {:5.2} awa {:6.1} boom {:6.1} drive {:4.2} {:?} afloat {} aground {}",
        sim.clock().tick.0,
        tf.position[0],
        tf.position[1],
        tf.position[2],
        v.linear[0],
        v.linear[1],
        v.linear[2],
        b.heading_deg,
        b.heel_deg,
        b.speed,
        b.awa_deg,
        b.boom_deg,
        b.drive,
        b.trim,
        b.afloat,
        b.aground
    )
}

/// Master's `sailboat_check`: before a steady 6 m/s wind toward +x, with the sail set and no
/// engine, five seconds (300 ticks) carry the Sloop at least 5 units along +x, and it stays
/// afloat. Run with the sail already up and with it hoisted from the first tick.
#[test]
fn the_sailboat_task() {
    for hoisted in [true, false] {
        let (mut sim, id) = sailboat_world(hoisted);
        sim.step(&mut NoHooks).unwrap();
        let x0 = boat(&sim, id).0.position[0];
        for t in 0..300 {
            sim.step(&mut NoHooks).unwrap();
            if t % 30 == 29 {
                println!("{}", line(&sim, id));
            }
        }
        let (tf, _, b) = boat(&sim, id);
        let run = tf.position[0] - x0;
        println!(
            "hoisted {hoisted}: {run:.2} along +x in 300 ticks, afloat {}",
            b.afloat
        );
        assert!(b.afloat, "the Sloop is not afloat at y {}", tf.position[1]);
        assert!(
            run >= 5.0,
            "300 ticks took the Sloop {run:.2} along +x, not 5 or more"
        );
    }
}

/// The scripted course of the sailing scene, every 60 ticks (for reading, with --nocapture).
#[test]
fn the_sailing_scene_course() {
    let mut sim = sailing_world();
    let id = EntityId::new(3).unwrap();
    let mut contacts = Vec::new();
    for t in 1..=1200 {
        let r = sim.step(&mut NoHooks).unwrap();
        // Crates are not land: touching them never puts the Sloop aground.
        assert!(!boat(&sim, id).2.aground, "aground at tick {t}");
        for e in r
            .events
            .iter()
            .filter(|e| e.kind.as_str().starts_with("physics.contact"))
        {
            let other = e.data.get("other").cloned();
            contacts.push(format!("{t} {} {:?} {other:?}", e.kind.as_str(), e.subject));
        }
        if t % 60 == 0 {
            println!("{}", line(&sim, id));
        }
    }
    println!("{}", contacts.join("\n"));
    let (tf, _, b) = boat(&sim, id);
    assert!(b.afloat);
    assert!(tf.position[1].abs() < 1.0);
    // The Sloop meets crates on its way (the forks of `physics.fork` cross such contacts).
    let sloop = format!("{:?}", Some(id));
    assert!(
        contacts
            .iter()
            .any(|c| c.contains("contact_began") && c.contains(&sloop)),
        "{contacts:?}"
    );
}

/// Sailing into an island (a fixed cuboid across its course) puts the Sloop aground, and a boat
/// aground is not afloat (shared/contract/sailing.md: `afloat` is "in the water and not aground").
#[test]
fn an_island_puts_the_sloop_aground() {
    let (mut sim, id) = sailboat_world(true);
    let island = Collider::new(Shape::Cuboid {
        half_extents: [2.0, 1.5, 6.0],
    });
    sim.boundary()
        .spawn((Transform::at([8.0, 0.0, 0.0]), RigidBody::fixed(), island))
        .unwrap();
    let mut was_afloat = false;
    let mut grounded = None;
    for t in 1..=900 {
        sim.step(&mut NoHooks).unwrap();
        let b = boat(&sim, id).2;
        if b.aground {
            assert!(!b.afloat, "{}", line(&sim, id));
            grounded = Some(t);
            break;
        }
        was_afloat |= b.afloat;
    }
    println!("{}", line(&sim, id));
    assert!(was_afloat, "the Sloop was never afloat before the island");
    let t = grounded.expect("the Sloop never went aground");
    let x = boat(&sim, id).0.position[0];
    assert!(
        x > 3.5 && x < 6.0,
        "aground at tick {t} with its origin at x {x}"
    );
}

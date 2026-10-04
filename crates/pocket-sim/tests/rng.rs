//! The RNG's checks (docs/spec/rng.md 10): reference vectors, the empty table at boundaries,
//! isolation between systems, the same streams in a copy of the world and different ones after a
//! reseed, and the distribution of draws.

mod common;

use bevy_ecs::prelude::*;
use common::{Seen, seen, sim};
use pocket_sim::rng::{KeyPart, Pcg32};
use pocket_sim::{EntityId, NoHooks, RngTable, RunCondition, SimClock, TickPhase};

#[test]
fn reference_vectors() {
    pocket_sim::rng::check_vectors().unwrap();
}

#[test]
fn nothing_draws_at_a_boundary_and_the_table_is_empty() {
    let mut s = sim(42);
    s.add_system(
        "test.draw",
        TickPhase::Update,
        RunCondition::Always,
        |mut rng: ResMut<RngTable>| {
            rng.system("test.draw").unwrap().next_u32();
        },
    )
    .unwrap();
    for _ in 0..3 {
        s.step(&mut NoHooks).unwrap();
        assert!(s.world().resource::<RngTable>().is_empty());
        let e = s
            .world_mut()
            .resource_mut::<RngTable>()
            .system("x")
            .unwrap_err();
        assert_eq!(e.code, "rng.outside_tick");
    }
}

/// Two systems recording their draws; `extra` adds draws to the first one only.
fn two_systems(seed: u64, extra: bool) -> pocket_sim::Sim {
    let mut s = sim(seed);
    s.add_system(
        "script.update",
        TickPhase::Update,
        RunCondition::Always,
        move |mut rng: ResMut<RngTable>, c: Res<SimClock>, mut seen: ResMut<Seen>| {
            let r = rng.system("script:a").unwrap();
            if extra {
                r.next_u32();
                r.next_f64();
            }
            let a = r.next_u32();
            let e = rng
                .entity("script:a", EntityId::new(7).unwrap())
                .unwrap()
                .next_u32();
            seen.0.push(format!("{} a {a} e {e}", c.tick.0));
        },
    )
    .unwrap();
    s.add_system(
        "physics.wind",
        TickPhase::Forces,
        RunCondition::Always,
        |mut rng: ResMut<RngTable>, c: Res<SimClock>, mut seen: ResMut<Seen>| {
            let b = rng.system("physics.wind").unwrap().next_u32();
            let n = rng
                .named("physics.wind", &[KeyPart::Str("gust"), KeyPart::Int(2)])
                .unwrap()
                .next_u32();
            let t = rng
                .timeless(&[KeyPart::Str("level"), KeyPart::Int(7)])
                .unwrap()
                .next_u32();
            seen.0.push(format!("{} b {b} n {n} t {t}", c.tick.0));
        },
    )
    .unwrap();
    s
}

fn run(mut s: pocket_sim::Sim, ticks: u64) -> Vec<String> {
    for _ in 0..ticks {
        s.step(&mut NoHooks).unwrap();
    }
    seen(&s)
}

#[test]
fn a_draw_added_to_one_system_changes_no_other() {
    let plain = run(two_systems(5, false), 20);
    let extra = run(two_systems(5, true), 20);
    let of = |v: &[String], tag: &str| -> Vec<String> {
        v.iter().filter(|l| l.contains(tag)).cloned().collect()
    };
    // The other system's draws, all kinds, are identical at every tick.
    assert_eq!(of(&plain, " b "), of(&extra, " b "));
    // The changed system's own system stream differs; its entity stream does not.
    assert_ne!(of(&plain, " a "), of(&extra, " a "));
    let entity = |v: &[String]| -> Vec<String> {
        v.iter()
            .filter(|l| l.contains(" a "))
            .map(|l| l.split(" e ").nth(1).unwrap().to_owned())
            .collect()
    };
    assert_eq!(entity(&plain), entity(&extra));
    // A timeless stream gives the same numbers every tick.
    let t: Vec<&str> = plain.iter().filter_map(|l| l.split(" t ").nth(1)).collect();
    assert!(t.windows(2).all(|w| w[0] == w[1]));
    // System streams change from tick to tick.
    let b: Vec<&str> = plain
        .iter()
        .filter(|l| l.contains(" b "))
        .map(|l| l.split(' ').nth(2).unwrap())
        .collect();
    assert!(b.windows(2).all(|w| w[0] != w[1]));
}

#[test]
fn copies_draw_alike_and_a_reseed_diverges_from_the_next_tick() {
    let a = run(two_systems(11, false), 12);
    let b = run(two_systems(11, false), 12);
    assert_eq!(a, b, "the same seed and writes give the same draws");
    let mut c = two_systems(11, false);
    for _ in 0..5 {
        c.step(&mut NoHooks).unwrap();
    }
    c.boundary().reseed(12).unwrap();
    for _ in 0..7 {
        c.step(&mut NoHooks).unwrap();
    }
    let c = seen(&c);
    assert_eq!(a[..10], c[..10], "ticks 1 to 5 are unchanged");
    for (x, y) in a[10..].iter().zip(&c[10..]) {
        // Timeless streams depend on the seed too; every stream differs after the reseed.
        assert_ne!(x, y);
    }
    assert_eq!(
        sim(1).boundary().reseed(1 << 53).unwrap_err().code,
        "rng.seed_invalid"
    );
}

fn chi_square(counts: &[u64]) -> f64 {
    let n: u64 = counts.iter().sum();
    #[allow(clippy::cast_precision_loss)]
    let expected = n as f64 / counts.len() as f64;
    counts
        .iter()
        .map(|&c| {
            #[allow(clippy::cast_precision_loss)]
            let d = c as f64 - expected;
            d * d / expected
        })
        .sum()
}

#[test]
fn draws_are_uniform() {
    // below(6) over 6,000,000 draws per stream kind: chi-square with 5 degrees of freedom under
    // 20.515, the 0.001 level.
    let mut s = sim(2024);
    s.add_system(
        "test.dist",
        TickPhase::Update,
        RunCondition::Start,
        |mut rng: ResMut<RngTable>, mut seen: ResMut<Seen>| {
            let e = EntityId::new(3).unwrap();
            let parts = [KeyPart::Str("dice")];
            for kind in 0..4 {
                let r = match kind {
                    0 => rng.system("test.dist"),
                    1 => rng.entity("test.dist", e),
                    2 => rng.named("test.dist", &parts),
                    _ => rng.timeless(&parts),
                }
                .unwrap();
                let mut counts = [0u64; 6];
                for _ in 0..6_000_000 {
                    counts[r.below(6).unwrap() as usize] += 1;
                }
                seen.0.push(format!("{}", chi_square(&counts)));
            }
        },
    )
    .unwrap();
    s.step(&mut NoHooks).unwrap();
    for x in seen(&s) {
        let chi: f64 = x.parse().unwrap();
        assert!(chi < 20.515, "chi-square {chi}");
    }
    let mut r = Pcg32::new(99, 1);
    let mut sum = 0.0;
    for _ in 0..1_000_000 {
        sum += r.next_f64();
    }
    let mean = sum / 1_000_000.0;
    assert!((mean - 0.5).abs() < 0.002, "mean {mean}");
}

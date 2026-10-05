//! One use of every entry of tools/clippy-determinism.toml, each of which clippy must report
//! (docs/spec/checks.md 5.3 and 8.6). Nothing here runs; it only has to compile and be linted.

use bevy_ecs::lifecycle::{Add, RemovedComponents};
use bevy_ecs::prelude::*;
use bevy_ecs::query::QueryState;

#[derive(Component)]
pub struct P(pub f64);

pub fn clock_environment_threads() {
    let _ = std::time::Instant::now();
    let _ = std::time::SystemTime::now();
    let _ = std::env::var("POCKET");
    let _ = std::thread::spawn(|| {}).join();
}

pub fn queries(mut q: Query<&mut P>, r: Query<&P>, list: Vec<Entity>) {
    for _ in q.iter() {}
    for _ in q.iter_mut() {}
    for _ in r.iter_many(&list) {}
    let mut many = q.iter_many_mut(&list);
    while many.fetch_next().is_some() {}
    r.par_iter().for_each(|_| {});
    q.par_iter_mut().for_each(|_| {});
}

pub fn query_state(world: &mut World, state: &mut QueryState<&P>, all: &mut QueryState<&mut P>) {
    for _ in state.iter(world) {}
    for _ in all.iter_mut(world) {}
    for _ in world.iter_entities() {}
    world.add_observer(|_: On<Add, P>| {});
}

pub fn change_detection(
    p: Ref<P>,
    added: Query<&P, Added<P>>,
    changed: Query<&P, Changed<P>>,
    removed: RemovedComponents<P>,
) -> bool {
    let _ = (added, changed, removed);
    p.is_changed() || p.is_added()
}

pub fn local_state(mut n: Local<u32>) -> u32 {
    *n += 1;
    *n
}

pub fn maps() {
    let _: std::collections::HashMap<u32, u32> = Default::default();
    let _: std::collections::HashSet<u32> = Default::default();
    let _: hashbrown::HashMap<u32, u32> = Default::default();
    let _: bevy_platform::collections::HashMap<u32, u32> = Default::default();
}

pub fn numeric_f64(x: f64, y: f64) -> f64 {
    let mut s =
        x.sin() + x.cos() + x.tan() + x.sin_cos().0 + x.asin() + x.acos() + x.atan() + x.atan2(y);
    s += x.sinh() + x.cosh() + x.tanh() + x.asinh() + x.acosh() + x.atanh();
    s += x.exp() + x.exp2() + x.exp_m1() + x.ln() + x.log(y) + x.log2() + x.log10() + x.ln_1p();
    s += x.powf(y) + x.powi(3) + x.cbrt() + x.hypot(y) + x.min(y) + x.max(y) + x.mul_add(y, s);
    s
}

pub fn numeric_f32(x: f32, y: f32) -> f32 {
    let mut s =
        x.sin() + x.cos() + x.tan() + x.sin_cos().0 + x.asin() + x.acos() + x.atan() + x.atan2(y);
    s += x.sinh() + x.cosh() + x.tanh() + x.asinh() + x.acosh() + x.atanh();
    s += x.exp() + x.exp2() + x.exp_m1() + x.ln() + x.log(y) + x.log2() + x.log10() + x.ln_1p();
    s += x.powf(y) + x.powi(3) + x.cbrt() + x.hypot(y) + x.min(y) + x.max(y) + x.mul_add(y, s);
    s
}

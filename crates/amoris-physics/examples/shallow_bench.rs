//! The shallow-water spike's bench (docs/spikes/shallow-water.md): the dam break of
//! `shallow::bench_scene` at each size and with each scheme, timed per column and substep, and each
//! scheme's hash chain, which the WebAssembly run (`shallow_web`, `tests/web/shallow.mjs`) must
//! match bit for bit.
//!
//! `cargo run --release -p amoris-physics --example shallow_bench -- [N ...]` (default 128 256 512
//! 1024) prints one JSON line per scheme and size, then each scheme's chain.

#![allow(clippy::disallowed_methods)] // a benchmark reads the wall clock, outside any tick

use std::time::Instant;

use amoris_physics::shallow::{Scheme, bench_scene};

/// Substeps of 1/120 s each; the chain's hash every 60 substeps up to 600.
const DT: f64 = 1.0 / 120.0;

fn main() {
    let sizes: Vec<usize> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let sizes = if sizes.is_empty() {
        vec![128, 256, 512, 1024]
    } else {
        sizes
    };
    for (scheme, name) in [(Scheme::Advect, "advect"), (Scheme::Pipes, "pipes")] {
        for &n in &sizes {
            let mut s = bench_scene(n);
            s.scheme = scheme;
            s.step(DT * 10.0, 10);
            // About 0.3 s of work per size, in rounds of 10 substeps; the median round.
            let rounds = (30_000_000 / (n * n)).clamp(5, 200);
            let mut per = Vec::new();
            for _ in 0..rounds {
                let t = Instant::now();
                s.step(DT * 10.0, 10);
                per.push(t.elapsed().as_secs_f64() / 10.0);
            }
            per.sort_by(f64::total_cmp);
            let median = per[per.len() / 2];
            println!(
                "{{\"target\": \"native\", \"scheme\": \"{name}\", \"n\": {n}, \"columns\": {}, \
                 \"ms_per_substep\": {:.4}, \"ns_per_column\": {:.3}}}",
                n * n,
                median * 1e3,
                median * 1e9 / (n * n) as f64
            );
        }
        let mut s = bench_scene(128);
        s.scheme = scheme;
        let mut chain = Vec::new();
        for _ in 0..10 {
            s.step(DT * 60.0, 60);
            chain.push(format!("\"{:016x}\"", s.hash()));
        }
        println!(
            "{{\"scheme\": \"{name}\", \"chain\": [{}]}}",
            chain.join(", ")
        );
    }
}

//! The frontier water samples played through the real player action catalog on resident GPU
//! water: what a player does decides each outcome, and scripts never move the player.
#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_link::Source;
use amoris_runtime::{Command, Game, Project};
use serde_json::{Value, json};
use std::sync::Arc;

fn game(sample: &str) -> Game {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../samples")
        .join(sample);
    let setup = Arc::new(Project::load(&root).unwrap().setup(false).unwrap());
    water_compute::builder(setup, 1).build().unwrap()
}
fn developer(g: &mut Game, method: &str, params: Value) -> Result<Value, String> {
    g.apply(&Command::new(Source::Developer(9), 1, method, params))
        .map_err(|e| format!("{e:?}"))
}
/// The named entity's components, or `None` once it is gone.
fn read(g: &mut Game, entity: &str, components: &[&str]) -> Option<Value> {
    developer(
        g,
        "world.get",
        json!({"entity": entity, "fields": components}),
    )
    .ok()
    .map(|v| v["fields"].clone())
}
fn step(g: &mut Game, count: usize) {
    for _ in 0..count {
        let r = g.step().unwrap();
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }
}
/// The world's events whose kinds start with `prefix`, since the last call: the world keeps only
/// its latest events, so a run reads them every tick.
struct Events {
    prefix: String,
    cursor: u64,
    seen: Vec<Value>,
}
impl Events {
    fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.into(),
            cursor: 0,
            seen: Vec::new(),
        }
    }
    fn poll(&mut self, g: &mut Game) {
        let answer = developer(
            g,
            "player.events",
            json!({"since": self.cursor, "kinds": [self.prefix], "omniscient": true}),
        )
        .unwrap();
        self.cursor = answer["cursor"].as_u64().unwrap_or(self.cursor);
        self.seen
            .extend(answer["events"].as_array().cloned().unwrap_or_default());
    }
    fn of(&self, kind: &str) -> Vec<&Value> {
        self.seen.iter().filter(|e| e["kind"] == kind).collect()
    }
}
/// An event's reading by name, as the player perceives it.
fn reading(event: &Value, name: &str) -> f64 {
    event["data"]
        .as_array()
        .and_then(|data| data.iter().find(|r| r["name"] == name))
        .and_then(|r| r["value"].as_f64())
        .unwrap_or_else(|| panic!("{name} in {event}"))
}
struct Player {
    sequence: u64,
}
impl Player {
    fn act(&mut self, g: &mut Game, actions: Value) {
        self.sequence += 1;
        g.apply(&Command::new(
            Source::Player(0),
            self.sequence,
            "player.act",
            json!({ "actions": actions }),
        ))
        .unwrap();
    }
    /// Walk or swim toward a point, holding `rise` to stay up in water: yaw 0 looks along -z and
    /// positive yaw turns left.
    fn head(&mut self, g: &mut Game, from: &Value, to: [f64; 2], rise: bool) {
        let (dx, dz) = (
            to[0] - from[0].as_f64().unwrap(),
            to[1] - from[2].as_f64().unwrap(),
        );
        let yaw = (-dx).atan2(-dz);
        self.act(
            g,
            json!([{"do":"set","controls":{"forward":1.0,"yaw":yaw,"rise":rise}}]),
        );
    }
}
fn position(g: &mut Game, entity: &str) -> Option<Value> {
    read(g, entity, &["Transform"]).map(|v| v["Transform"]["position"].clone())
}
fn big_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn pool_party_a_cannonball_raises_corner_waves_and_every_duck_is_collected_by_swimming() {
    big_stack(|| {
        let mut g = game("pool-party");
        let mut player = Player { sequence: 0 };
        step(&mut g, 3);
        // Run west off the east deck and jump from the coping into the deep end.
        player.act(
            &mut g,
            json!([{"do":"set","controls":{"forward":1.0,"yaw":std::f64::consts::FRAC_PI_2,"run":true}}]),
        );
        let mut jumped = false;
        for _ in 0..240 {
            step(&mut g, 1);
            let at = position(&mut g, "Swimmer").unwrap();
            if !jumped && at[0].as_f64().unwrap() < 16.5 {
                player.act(&mut g, json!([{"do":"pulse","control":"jump"}]));
                jumped = true;
            }
            let walker = read(&mut g, "Swimmer", &["Walker"]).unwrap();
            if jumped && walker["Walker"]["in_liquid"] == json!(true) {
                break;
            }
        }
        assert!(jumped, "the swimmer never reached the coping");
        // The rules watch the corners for three seconds after the entry; meanwhile, and after,
        // swim to the nearest duck still floating.
        let mut news = Events::new("party.");
        let trace = std::env::var_os("FRONTIER_TRACE").is_some();
        let mut collected = 0;
        for tick in 0..5400 {
            if tick % 6 == 0 {
                let me = position(&mut g, "Swimmer").unwrap();
                let nearest = (0..8)
                    .filter_map(|n| position(&mut g, &format!("Duck{n}")))
                    .map(|d| {
                        let (x, z) = (d[0].as_f64().unwrap(), d[2].as_f64().unwrap());
                        let far = (x - me[0].as_f64().unwrap()).hypot(z - me[2].as_f64().unwrap());
                        (far, [x, z, d[1].as_f64().unwrap()])
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                if trace && tick % 60 == 0 {
                    eprintln!("{tick}: at {me} nearest {nearest:?} ducks {collected}");
                }
                match nearest {
                    Some((_, duck)) => player.head(&mut g, &me, [duck[0], duck[1]], true),
                    None => break,
                }
            }
            step(&mut g, 1);
            news.poll(&mut g);
            collected = read(&mut g, "Party", &["Party"]).unwrap()["Party"]["ducks"]
                .as_u64()
                .unwrap();
            if collected == 8 {
                break;
            }
        }
        news.poll(&mut g);
        let party = read(&mut g, "Party", &["Party"]).unwrap();
        let splash = news.of("party.splash");
        if trace {
            eprintln!("{:#}", json!(news.seen));
        }
        assert_eq!(splash.len(), 1, "{party}");
        let wave = reading(splash[0], "wave_m");
        assert!((wave - party["Party"]["best_splash_m"].as_f64().unwrap()).abs() < 0.006);
        // Corner gauges 6 to 15 m from the entry see the jump's wave, a centimetre or more.
        assert!(wave > 0.01 && wave < 0.5, "{wave} {party}");
        assert_eq!(collected, 8, "{party}");
        let ducks = news.of("party.duck");
        assert_eq!(ducks.len(), 8);
        assert!(
            ducks
                .iter()
                .enumerate()
                .all(|(n, e)| reading(e, "ducks") == (n + 1) as f64)
        );
        assert_eq!(news.of("party.done").len(), 1);
        // The guests kept cannonballing, and every body stayed in the pool's world.
        for n in 0..3 {
            let guest = read(&mut g, &format!("Guest{n}"), &["Transform", "Guest"]).unwrap();
            let p = &guest["Transform"]["position"];
            assert!(
                (0..3).all(|a| p[a].as_f64().unwrap().is_finite()),
                "{guest}"
            );
        }
        let water = read(&mut g, "PoolWater", &["WaterResident"]).unwrap();
        let speed = water["WaterResident"]["stats"]["max_speed"]
            .as_f64()
            .unwrap();
        assert!(speed.is_finite() && speed < 6.0, "{water}");
    });
}

/// The yaw a heading has, Walker style: 0 along -z, positive to the left.
fn yaw_of(x: f64, z: f64) -> f64 {
    (-x).atan2(-z)
}
fn wrap(a: f64) -> f64 {
    (a + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
}

#[test]
fn river_falls_a_paddled_tube_runs_the_gates_drops_over_the_falls_and_reaches_the_beach() {
    big_stack(|| {
        let mut g = game("river-falls");
        let mut player = Player { sequence: 0 };
        let mut news = Events::new("falls.");
        let trace = std::env::var_os("FRONTIER_TRACE").is_some();
        let gates: Vec<[f64; 2]> = (0..3)
            .map(|n| {
                let gate = read(&mut g, &format!("Gate{n}"), &["Gate"]).unwrap();
                [
                    gate["Gate"]["x"].as_f64().unwrap(),
                    gate["Gate"]["z"].as_f64().unwrap(),
                ]
            })
            .collect();
        let course = read(&mut g, "Course", &["Course"]).unwrap()["Course"].clone();
        let lip = course["lip_x"].as_f64().unwrap();
        let beach = [
            course["beach_x"].as_f64().unwrap(),
            course["beach_z"].as_f64().unwrap(),
        ];
        let mut highest = f64::MIN;
        for tick in 0..6000 {
            if tick % 4 == 0 {
                let tube = read(&mut g, "Tube", &["Transform"]).unwrap();
                let at = &tube["Transform"]["position"];
                let (x, z) = (at[0].as_f64().unwrap(), at[2].as_f64().unwrap());
                let riding =
                    read(&mut g, "Rider", &["Walker"]).unwrap()["Walker"]["mount"].is_number();
                // The next gate ahead, then the middle of the lip, then the beach.
                let goal = gates
                    .iter()
                    .copied()
                    .find(|gate| gate[0] > x + 0.3)
                    .unwrap_or(if x < lip + 1.0 {
                        [lip + 4.0, 15.0]
                    } else {
                        beach
                    });
                if riding {
                    let q = &tube["Transform"]["rotation"];
                    let [qx, qy, qz, qw] = [0, 1, 2, 3].map(|i| q[i].as_f64().unwrap());
                    // The tube's nose, -z in its frame, in the world.
                    let ahead = [
                        -2.0 * (qx * qz + qw * qy),
                        -(1.0 - 2.0 * (qx * qx + qy * qy)),
                    ];
                    let error = wrap(yaw_of(goal[0] - x, goal[1] - z) - yaw_of(ahead[0], ahead[1]));
                    let lean = (-2.0 * error).clamp(-1.0, 1.0);
                    player.act(
                        &mut g,
                        json!([{"do":"set","controls":{"forward":1.0,"strafe":lean}}]),
                    );
                } else {
                    let me = position(&mut g, "Rider").unwrap();
                    player.head(&mut g, &me, beach, true);
                }
                if trace && tick % 120 == 0 {
                    eprintln!(
                        "{tick}: tube [{x:.1}, {:.1}, {z:.1}] riding {riding} goal {goal:?}",
                        at[1].as_f64().unwrap()
                    );
                }
                highest = highest.max(at[1].as_f64().unwrap());
            }
            step(&mut g, 1);
            news.poll(&mut g);
            if !news.of("falls.finished").is_empty() {
                break;
            }
        }
        let run = read(&mut g, "Rider", &["Run"]).unwrap();
        if trace {
            eprintln!("{:#}", json!(news.seen));
        }
        let finished = news.of("falls.finished");
        assert_eq!(finished.len(), 1, "{run}");
        let gates_passed = reading(finished[0], "gates");
        assert!(gates_passed >= 2.0, "{run}");
        assert_eq!(news.of("falls.gate").len() as f64, gates_passed);
        // Over the falls: either it landed in the pool on the tube, or the rider came off.
        let drop = news.of("falls.drop");
        assert!(
            !drop.is_empty() || !news.of("falls.wipeout").is_empty(),
            "{run}"
        );
        for d in drop {
            assert!(reading(d, "drop_m") > 5.0, "{d}");
        }
        assert!(reading(finished[0], "time_s") < 90.0, "{run}");
        let water = read(&mut g, "River", &["WaterResident"]).unwrap();
        let b = &water["WaterResident"]["boundary"];
        // The river runs through: by the finish, water leaves through the outlet about as fast as
        // it enters the lake (how much has left by then depends on how fast the run was).
        let entered = b["entered_total"].as_f64().unwrap();
        let rate_in = b["entered"].as_f64().unwrap();
        let rate_out = b["left"].as_f64().unwrap();
        assert!(entered > 100.0 && rate_out > 0.5 * rate_in, "{b}");
        assert!(
            b["numerical_residual"].as_f64().unwrap().abs() < 0.01,
            "{b}"
        );
    });
}

/// The yaw a body's nose (-z in its frame) points along, Walker style.
fn nose_yaw(q: &Value) -> f64 {
    let [qx, qy, qz, qw] = [0, 1, 2, 3].map(|i| q[i].as_f64().unwrap());
    yaw_of(
        -2.0 * (qx * qz + qw * qy),
        -(1.0 - 2.0 * (qx * qx + qy * qy)),
    )
}

#[test]
fn surf_beach_a_surfer_catches_a_wave_at_the_point_and_rides_it_along_the_reef() {
    big_stack(|| {
        let mut g = game("surf-beach");
        let mut player = Player { sequence: 0 };
        let mut news = Events::new("surf.");
        let trace = std::env::var_os("FRONTIER_TRACE").is_some();
        let surf = developer(
            &mut g,
            "world.get",
            json!({"entity": "Surf", "fields": ["Name"]}),
        )
        .unwrap()["id"]
            .clone();
        // A gauge that rides 12 m seaward of the board: the next crest's height.
        developer(
            &mut g,
            "world.edit",
            json!({"ops": [{"spawn": {"name": "Spotter", "components": {
                "WaterProbe": {"region": surf, "at": [148.0, 20.0]}}}}]}),
        )
        .unwrap();
        let mut stood_at = None;
        // A pop-up asked for and not yet seen: one jump toggles standing once.
        let mut popping = false;
        let mut best = 0.0f64;
        for tick in 0..(150 * 60) {
            let board = read(&mut g, "Board", &["Transform", "Board"]).unwrap();
            let at = &board["Transform"]["position"];
            let (x, z) = (at[0].as_f64().unwrap(), at[2].as_f64().unwrap());
            let standing = board["Board"]["standing"] == json!(true);
            let speed = board["Board"]["speed"].as_f64().unwrap();
            let mounted =
                read(&mut g, "Surfer", &["Walker"]).unwrap()["Walker"]["mount"].is_number();
            if tick % 2 == 0 {
                developer(
                    &mut g,
                    "world.edit",
                    json!({"ops": [{"set": {"entity": "Spotter", "component": "WaterProbe",
                        "value": {"at": [x - 12.0, z]}}}]}),
                )
                .unwrap();
            }
            let crest = read(&mut g, "Spotter", &["WaterProbe"]).unwrap()["WaterProbe"]["surface"]
                .as_f64()
                .unwrap_or(0.0);
            let heading = nose_yaw(&board["Transform"]["rotation"]);
            if standing {
                popping = false;
            }
            if !mounted || (stood_at.is_some() && !standing && !popping) || x > 228.0 {
                // Off the board, or the ride is over: back to the lineup for the next wave.
                player.act(
                    &mut g,
                    json!([{"do":"pulse","control":"secondary"},
                    {"do":"set","controls":{"forward":0.0,"strafe":0.0}}]),
                );
                stood_at = None;
                popping = false;
            } else if standing {
                // Angle along the wave's face toward the peel, north and shoreward.
                let error = wrap(yaw_of(1.0, 0.9) - heading);
                player.act(
                    &mut g,
                    json!([{"do":"set","controls":{
                    "forward":0.0,"strafe":(-2.0 * error).clamp(-1.0, 1.0)}}]),
                );
            } else {
                // Lying: face the beach, and paddle when a crest is coming.
                let error = wrap(yaw_of(1.0, 0.0) - heading);
                let paddle = if crest > 0.4 || speed > 2.0 { 1.0 } else { 0.0 };
                player.act(
                    &mut g,
                    json!([{"do":"set","controls":{
                    "forward":paddle,"strafe":(-2.0 * error).clamp(-1.0, 1.0)}}]),
                );
                if speed > 3.5 && !popping {
                    player.act(&mut g, json!([{"do":"pulse","control":"jump"}]));
                    stood_at = Some(tick);
                    popping = true;
                }
            }
            if trace && (tick % 60 == 0 || (stood_at.is_some() && tick % 5 == 0)) {
                eprintln!(
                    "{tick}: board [{x:.1}, {z:.1}] speed {speed:.2} standing {standing} crest {crest:+.2} mounted {mounted}"
                );
            }
            step(&mut g, 1);
            news.poll(&mut g);
            for ride in news.of("surf.ride") {
                best = best.max(reading(ride, "distance_m"));
            }
            if best >= 15.0 {
                break;
            }
        }
        if trace {
            eprintln!("{:#}", json!(news.seen));
        }
        let session = read(&mut g, "Surfer", &["Session"]).unwrap();
        assert!(best >= 15.0, "longest ride {best} m: {session}");
        let ride = news.of("surf.ride").into_iter().last().unwrap();
        // Faster than paddling: the wave's face carried it.
        assert!(reading(ride, "top_mps") > 4.0, "{ride}");
        assert!(!news.of("surf.popup").is_empty());
    });
}

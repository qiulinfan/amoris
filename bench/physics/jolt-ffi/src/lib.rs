//! Rust stepping Jolt through joltc (github.com/amerkoleci/joltc, MIT): the 24 C functions the
//! prototype needs, declared by hand, and Jolt's Pyramid scene built and stepped across the C ABI.
//! The same code runs natively (src/main.rs, timed like bench/physics/jolt/jolt_bench.cpp) and
//! inside a Rust `wasm32-unknown-unknown` module, the target Amoris's browser build uses
//! (`web` below, driven by bench/physics/scripts/run_jolt_wasm.mjs).
//!
//! The end-of-run hash is jolt_bench's (FNV-1a over positions and rotations, in body order), so equal
//! hashes show that a build simulates exactly what the C++ build does.

use std::ffi::c_void;
use std::sync::Once;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

#[repr(C)]
struct JobSystemThreadPoolConfig {
    max_jobs: u32,
    max_barriers: u32,
    num_threads: i32,
}

#[repr(C)]
struct PhysicsSystemSettings {
    max_bodies: u32,
    num_body_mutexes: u32,
    max_body_pairs: u32,
    max_contact_constraints: u32,
    _padding: u32,
    broad_phase_layer_interface: *mut c_void,
    object_layer_pair_filter: *mut c_void,
    object_vs_broad_phase_layer_filter: *mut c_void,
}

#[repr(C)]
#[derive(Default)]
struct RayCastResult {
    body_id: u32,
    fraction: f32,
    sub_shape_id2: u32,
}

const MOTION_STATIC: u32 = 0;
const MOTION_DYNAMIC: u32 = 2;
const ACTIVATE: u32 = 0;
const DONT_ACTIVATE: u32 = 1;
const NON_MOVING: u32 = 0;
const MOVING: u32 = 1;

unsafe extern "C" {
    fn JPH_Init() -> bool;
    fn JPH_JobSystemThreadPool_Create(config: *const JobSystemThreadPoolConfig) -> *mut c_void;
    fn JPH_JobSystem_Destroy(job_system: *mut c_void);
    fn JPH_TempAllocator_Create(size: u32) -> *mut c_void;
    fn JPH_TempAllocator_Destroy(allocator: *mut c_void);
    fn JPH_ObjectLayerPairFilterTable_Create(num_object_layers: u32) -> *mut c_void;
    fn JPH_ObjectLayerPairFilterTable_EnableCollision(filter: *mut c_void, a: u32, b: u32);
    fn JPH_BroadPhaseLayerInterfaceTable_Create(
        num_object_layers: u32,
        num_bp_layers: u32,
    ) -> *mut c_void;
    fn JPH_BroadPhaseLayerInterfaceTable_MapObjectToBroadPhaseLayer(
        i: *mut c_void,
        object_layer: u32,
        bp_layer: u8,
    );
    fn JPH_ObjectVsBroadPhaseLayerFilterTable_Create(
        bp_interface: *mut c_void,
        num_bp_layers: u32,
        pair_filter: *mut c_void,
        num_object_layers: u32,
    ) -> *mut c_void;
    fn JPH_PhysicsSystem_Create(settings: *const PhysicsSystemSettings) -> *mut c_void;
    fn JPH_PhysicsSystem_Destroy(system: *mut c_void);
    fn JPH_PhysicsSystem_GetBodyInterfaceNoLock(system: *mut c_void) -> *mut c_void;
    fn JPH_PhysicsSystem_GetNarrowPhaseQueryNoLock(system: *const c_void) -> *const c_void;
    fn JPH_PhysicsSystem_OptimizeBroadPhase(system: *mut c_void);
    fn JPH_PhysicsSystem_Update2(
        system: *mut c_void,
        dt: f32,
        collision_steps: i32,
        temp: *mut c_void,
        job_system: *mut c_void,
    ) -> u32;
    fn JPH_BoxShape_Create(half_extent: *const Vec3, convex_radius: f32) -> *mut c_void;
    fn JPH_Shape_Destroy(shape: *mut c_void);
    fn JPH_BodyCreationSettings_Create3(
        shape: *const c_void,
        position: *const Vec3,
        rotation: *const Quat,
        motion_type: u32,
        object_layer: u32,
    ) -> *mut c_void;
    fn JPH_BodyCreationSettings_SetAllowSleeping(settings: *mut c_void, value: bool);
    fn JPH_BodyCreationSettings_Destroy(settings: *mut c_void);
    fn JPH_BodyInterface_CreateAndAddBody(
        bi: *mut c_void,
        settings: *const c_void,
        activation: u32,
    ) -> u32;
    fn JPH_BodyInterface_GetPositionAndRotation(
        bi: *mut c_void,
        id: u32,
        position: *mut Vec3,
        rotation: *mut Quat,
    );
    fn JPH_NarrowPhaseQuery_CastRay(
        query: *const c_void,
        origin: *const Vec3,
        direction: *const Vec3,
        hit: *mut RayCastResult,
        bp_filter: *const c_void,
        object_filter: *const c_void,
        body_filter: *const c_void,
    ) -> bool;
}

/// FNV-1a, jolt_bench's body hash.
pub fn fnv(hash: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *hash ^= u64::from(*b);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// joltc's global state (allocator hooks, the factory, the type registry), set up once per process
/// and never torn down: JPH_Shutdown would pull it from under any other live scene.
fn init_jolt() {
    static INIT: Once = Once::new();
    // SAFETY: JPH_Init has no preconditions; Once runs it a single time.
    INIT.call_once(|| assert!(unsafe { JPH_Init() }));
}

/// Jolt's PyramidScene (PerformanceTest/PyramidScene.h) with `height` layers in its own
/// PhysicsSystem, plus the Raycast scene's 10,000 rays (jolt_bench.cpp, MakeRays).
pub struct Pyramid {
    job_system: *mut c_void,
    temp: *mut c_void,
    system: *mut c_void,
    bi: *mut c_void,
    npq: *const c_void,
    pub ids: Vec<u32>,
    rays: Vec<(Vec3, Vec3)>,
}

impl Pyramid {
    /// `threads` counts the calling thread: 1 means no worker threads (the only choice in wasm).
    pub fn new(height: i32, threads: i32) -> Pyramid {
        init_jolt();
        // SAFETY: the calls follow joltc's documented order (init, allocators, filters, system,
        // bodies); every pointer passed is one joltc returned or a live stack value.
        unsafe {
            let job_system = JPH_JobSystemThreadPool_Create(&JobSystemThreadPoolConfig {
                max_jobs: 2048,
                max_barriers: 8,
                num_threads: threads - 1,
            });
            let temp = JPH_TempAllocator_Create(256 * 1024 * 1024);
            let pairs = JPH_ObjectLayerPairFilterTable_Create(2);
            JPH_ObjectLayerPairFilterTable_EnableCollision(pairs, NON_MOVING, MOVING);
            JPH_ObjectLayerPairFilterTable_EnableCollision(pairs, MOVING, MOVING);
            let bpi = JPH_BroadPhaseLayerInterfaceTable_Create(2, 2);
            JPH_BroadPhaseLayerInterfaceTable_MapObjectToBroadPhaseLayer(bpi, NON_MOVING, 0);
            JPH_BroadPhaseLayerInterfaceTable_MapObjectToBroadPhaseLayer(bpi, MOVING, 1);
            let ovb = JPH_ObjectVsBroadPhaseLayerFilterTable_Create(bpi, 2, pairs, 2);
            let big = height > 15;
            let system = JPH_PhysicsSystem_Create(&PhysicsSystemSettings {
                max_bodies: if big { 65536 } else { 10240 },
                num_body_mutexes: 0,
                max_body_pairs: if big { 262144 } else { 65536 },
                max_contact_constraints: if big { 131072 } else { 20480 },
                _padding: 0,
                broad_phase_layer_interface: bpi,
                object_layer_pair_filter: pairs,
                object_vs_broad_phase_layer_filter: ovb,
            });
            let bi = JPH_PhysicsSystem_GetBodyInterfaceNoLock(system);

            let identity = Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            };
            let floor_shape = JPH_BoxShape_Create(
                &Vec3 {
                    x: 50.0,
                    y: 1.0,
                    z: 50.0,
                },
                0.0,
            );
            let floor = JPH_BodyCreationSettings_Create3(
                floor_shape,
                &Vec3 {
                    x: 0.0,
                    y: -1.0,
                    z: 0.0,
                },
                &identity,
                MOTION_STATIC,
                NON_MOVING,
            );
            let mut ids = vec![JPH_BodyInterface_CreateAndAddBody(bi, floor, DONT_ACTIVATE)];
            JPH_BodyCreationSettings_Destroy(floor);
            let box_shape = JPH_BoxShape_Create(
                &Vec3 {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
                0.0,
            );
            let (size, sep, half, h) = (2.0f32, 0.5f32, 1.0f32, height);
            for i in 0..h {
                for j in i / 2..h - (i + 1) / 2 {
                    for k in i / 2..h - (i + 1) / 2 {
                        let shift = if i & 1 == 1 { half } else { 0.0 };
                        let p = Vec3 {
                            x: -h as f32 + size * j as f32 + shift,
                            y: 1.0 + (size + sep) * i as f32,
                            z: -h as f32 + size * k as f32 + shift,
                        };
                        let s = JPH_BodyCreationSettings_Create3(
                            box_shape,
                            &p,
                            &identity,
                            MOTION_DYNAMIC,
                            MOVING,
                        );
                        JPH_BodyCreationSettings_SetAllowSleeping(s, false);
                        ids.push(JPH_BodyInterface_CreateAndAddBody(bi, s, ACTIVATE));
                        JPH_BodyCreationSettings_Destroy(s);
                    }
                }
            }
            // The bodies hold their own references to the shapes; drop the ones creation returned.
            JPH_Shape_Destroy(floor_shape);
            JPH_Shape_Destroy(box_shape);
            JPH_PhysicsSystem_OptimizeBroadPhase(system);
            let npq = JPH_PhysicsSystem_GetNarrowPhaseQueryNoLock(system);

            let mut rays = Vec::with_capacity(10_000);
            for i in 0..100 {
                for j in 0..100 {
                    let o = Vec3 {
                        x: -148.5 + 3.0 * i as f32,
                        y: 40.0,
                        z: -148.5 + 3.0 * j as f32,
                    };
                    let d = Vec3 {
                        x: 10.0 * (i % 7 - 3) as f32,
                        y: -100.0,
                        z: 10.0 * (j % 5 - 2) as f32,
                    };
                    rays.push((o, d));
                }
            }
            Pyramid {
                job_system,
                temp,
                system,
                bi,
                npq,
                ids,
                rays,
            }
        }
    }

    /// One 1/60 s step with one collision step; panics on Jolt's update errors (full buffers).
    pub fn step(&mut self) {
        // SAFETY: system, temp allocator and job system are live (dropped only in Drop).
        let err = unsafe {
            JPH_PhysicsSystem_Update2(self.system, 1.0 / 60.0, 1, self.temp, self.job_system)
        };
        assert_eq!(err, 0, "physics update error");
    }

    /// Every body's pose, as pocket-physics would read them to write Transforms.
    pub fn read_poses(&self, out: &mut [(Vec3, Quat)]) {
        for (id, pose) in self.ids.iter().zip(out.iter_mut()) {
            // SAFETY: the body ids came from this system; the out pointers are valid.
            unsafe {
                JPH_BodyInterface_GetPositionAndRotation(self.bi, *id, &mut pose.0, &mut pose.1)
            };
        }
    }

    /// The 10,000 closest-hit rays, one C call each; returns (hits, sum of hit fractions).
    pub fn cast_rays(&self) -> (u64, f64) {
        let (mut hits, mut fraction_sum) = (0u64, 0f64);
        for (o, d) in &self.rays {
            let mut hit = RayCastResult::default();
            // SAFETY: npq is the live system's query; null filters mean "everything".
            let found = unsafe {
                JPH_NarrowPhaseQuery_CastRay(
                    self.npq,
                    o,
                    d,
                    &mut hit,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            };
            if found {
                hits += 1;
                fraction_sum += f64::from(hit.fraction);
            }
        }
        (hits, fraction_sum)
    }

    /// jolt_bench's end-of-run hash.
    pub fn hash(&self) -> u64 {
        let mut poses = vec![(Vec3::default(), Quat::default()); self.ids.len()];
        self.read_poses(&mut poses);
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for (p, q) in &poses {
            for v in [p.x, p.y, p.z, q.x, q.y, q.z, q.w] {
                fnv(&mut hash, &v.to_le_bytes());
            }
        }
        hash
    }
}

impl Drop for Pyramid {
    fn drop(&mut self) {
        // SAFETY: each object is destroyed once, after its last use, in reverse order of creation.
        // JPH_PhysicsSystem_Destroy also deletes the three layer tables the system was created with
        // (joltc owns them from JPH_PhysicsSystem_Create on), and the bodies it deletes release the
        // last references to the shapes.
        unsafe {
            JPH_PhysicsSystem_Destroy(self.system);
            JPH_TempAllocator_Destroy(self.temp);
            JPH_JobSystem_Destroy(self.job_system);
        }
    }
}

/// The browser-side entry points: a `wasm32-unknown-unknown` cdylib built with
/// `cargo rustc --lib --crate-type cdylib --target wasm32-unknown-unknown` exports these, and
/// bench/physics/scripts/run_jolt_wasm.mjs drives them under Node (V8), timing each step from
/// JavaScript. One scene at a time; wasm has one thread.
#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;

    use super::Pyramid;

    thread_local! {
        static SCENE: RefCell<Option<Pyramid>> = const { RefCell::new(None) };
    }

    /// Builds the pyramid with `height` layers; returns the body count.
    #[unsafe(no_mangle)]
    pub extern "C" fn pyramid_new(height: i32) -> u32 {
        SCENE.with_borrow_mut(|s| {
            *s = None;
            let p = Pyramid::new(height, 1);
            let n = p.ids.len() as u32;
            *s = Some(p);
            n
        })
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn pyramid_step() {
        SCENE.with_borrow_mut(|s| s.as_mut().expect("pyramid_new first").step());
    }

    /// Casts the 10,000 rays; returns the hit count.
    #[unsafe(no_mangle)]
    pub extern "C" fn pyramid_rays() -> u32 {
        SCENE.with_borrow(|s| s.as_ref().expect("pyramid_new first").cast_rays().0 as u32)
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn pyramid_hash() -> u64 {
        SCENE.with_borrow(|s| s.as_ref().expect("pyramid_new first").hash())
    }

    #[unsafe(no_mangle)]
    pub extern "C" fn pyramid_drop() {
        SCENE.with_borrow_mut(|s| *s = None);
    }
}

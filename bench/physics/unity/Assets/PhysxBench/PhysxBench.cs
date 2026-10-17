// The PhysX side of the R1 physics benchmark (docs/bench/physics-r1.md): the scenes of
// bench/physics/rapier/src/scenes.rs rebuilt with Unity's built-in 3D physics (PhysX), stepped by
// script at 1/60 s, one JSON line per run with the Rapier bench's keys.
//
// Player: PhysxBench.exe -batchmode -nographics -logFile LOG -job-worker-count T-1 -physxbench
//         -scene Pyramid [-steps 500] [-threads T] [-iters P] [-vel-iters V] [-probe]
//         [-sleep-energy E] -out FILE.jsonl
// Editor: Unity.exe -batchmode -nographics -projectPath bench/physics/unity
//         -executeMethod PhysxBenchEditor.Run -physxbench ... (Editor/PhysxBenchEditor.cs)
//
// The solver type (PGS or TGS) is a project setting with no runtime API; each player is built with
// one (PhysxBenchEditor.BuildPlayers) and reads its name from StreamingAssets/physx_solver.txt, so
// the JSON's "solver" is the build's setting, not something PhysX reports (the two builds'
// globalgamemanagers differ in that one byte, docs/bench/physics-r1.md). -probe adds the effective
// settings and each kind of body's mass properties, limits and outcome ("probe", for `r1_bench.py
// check --unity`); it does not change the simulation. On Windows every line also gives the share of
// the machine that other processes used during the steps (steps_cpu_other_pct), which r1_bench.py's
// quiet pairs use in place of its own measure over the player's whole life, start-up included.
//
// Validated on R1 on 2026-10-16 (Unity 6000.3.13f1, whose player log names PhysX SDK 4.1.2): the
// bodies, shapes, masses, materials and limits match Rapier's, while the solver, determinism and
// sleep settings differ; the outcomes agree on the pyramids and the rays and differ on
// ConvexVsMesh (docs/bench/physics-r1.md, "Fidelity").
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using Unity.Jobs.LowLevel.Unsafe;
using UnityEngine;
using Debug = UnityEngine.Debug;

public static class PhysxBench
{
    // Jolt's defaults, which the Rapier side copies (scenes.rs): damping 0.05, 1,000 kg per cubic
    // metre, friction 0.2 in the pyramids.
    const float Damping = 0.05f;
    const float Density = 1000f;
    const float PyramidFriction = 0.2f;
    const float Dt = 1f / 60f;
    // Rapier 0.36's limits (integration_parameters.rs, staged_island_solver/worker.rs), set on
    // every body in place of Unity's defaults: penetration recovery at most 3 m/s
    // (normalized_max_corrective_velocity; Unity's manual gives 10 as the default), at most
    // pi/4 rad of rotation per 1/60 s step (Unity's default angular speed limit is 50 rad/s), and
    // 400 m/s (normalized_max_linear_velocity; Unity has no practical limit).
    const float MaxDepenetrationVelocity = 3f;
    const float MaxAngularVelocity = Mathf.PI / 4f / Dt;
    const float MaxLinearVelocity = 400f;
    // Contacts are generated within the sum of the two colliders' offsets: 0.01 + 0.01 m, Rapier's
    // prediction distance of 0.02 m (normalized_prediction_distance x length_unit 1). Unity's
    // default, set explicitly so the project setting cannot change it.
    const float ContactOffset = 0.01f;
    // Sleeping (ConvexVsMesh; the pyramids never sleep). Rapier 0.36 lets a body sleep after 0.5 s
    // in which, every step, its pose moved its farthest point (translation plus the chord of the
    // rotation) by less than 2 x 0.05 m/s x dt and its angular speed stayed under pi/2 rad/s; its
    // linear velocity is not read (RigidBodyActivation::update_energy, whose `drift * 0.5 <
    // linear_threshold * dt` with the default threshold 0.05 tolerates 0.1 m/s). PhysX lets a body
    // sleep after its wake counter (0.4 s, which Unity does not expose) with the mass-normalized
    // kinetic energy, 0.5 v^2 for translation, below the body's sleepThreshold. Unity's default
    // 0.005 is 0.5 x 0.1^2, Rapier's budget for a translating body, so the bodies keep it; it is
    // set explicitly so that the project setting cannot change it, and -sleep-energy E overrides
    // it. Rotation and timing still differ (docs/bench/physics-r1.md, "Fidelity").
    const float SleepEnergy = 0.5f * 0.1f * 0.1f;

#if ENABLE_IL2CPP
    const string ScriptingBackend = "IL2CPP";
#elif ENABLE_MONO
    const string ScriptingBackend = "Mono";
#else
    const string ScriptingBackend = "unknown";
#endif

    public sealed class Options
    {
        public string scene = "Pyramid";
        public int steps = 500;
        public int threads = 1;
        public int positionIterations = -1; // -1: the project's default (Unity's is 6)
        public int velocityIterations = -1; // -1: the project's default (Unity's is 1)
        public string output;
        public string mode = "player";
        public string solver = "unknown";
        public bool probe;
        public float sleepEnergy = SleepEnergy; // ConvexVsMesh's bodies' sleepThreshold
    }

    [RuntimeInitializeOnLoadMethod(RuntimeInitializeLoadType.AfterSceneLoad)]
    static void PlayerEntry()
    {
        if (Application.isEditor)
            return;
        var o = Parse(Environment.GetCommandLineArgs());
        if (o == null)
            return;
        var solverFile = Path.Combine(Application.streamingAssetsPath, "physx_solver.txt");
        o.solver = File.Exists(solverFile) ? File.ReadAllText(solverFile).Trim() : "unknown";
        int code = 0;
        try
        {
            Run(o);
        }
        catch (Exception e)
        {
            Debug.LogException(e);
            code = 1;
        }
        Application.Quit(code);
    }

    /// The options after -physxbench, or null when the process was started without it.
    public static Options Parse(string[] args)
    {
        int start = Array.IndexOf(args, "-physxbench");
        if (start < 0)
            return null;
        var o = new Options();
        for (int i = start + 1; i < args.Length; i++)
        {
            string Next() => i + 1 < args.Length ? args[++i] : throw new ArgumentException(args[i] + " needs a value");
            switch (args[i])
            {
                case "-scene": o.scene = Next(); break;
                case "-steps": o.steps = int.Parse(Next(), CultureInfo.InvariantCulture); break;
                case "-threads": o.threads = int.Parse(Next(), CultureInfo.InvariantCulture); break;
                case "-iters": o.positionIterations = int.Parse(Next(), CultureInfo.InvariantCulture); break;
                case "-vel-iters": o.velocityIterations = int.Parse(Next(), CultureInfo.InvariantCulture); break;
                case "-out": o.output = Next(); break;
                case "-probe": o.probe = true; break;
                case "-sleep-energy": o.sleepEnergy = float.Parse(Next(), CultureInfo.InvariantCulture); break;
            }
        }
        return o;
    }

    sealed class World
    {
        public readonly List<Rigidbody> bodies = new List<Rigidbody>();
        public readonly List<string> kinds = new List<string>(); // bodies[i]'s shape, for -probe
        public readonly List<Collider> statics = new List<Collider>();
    }

    static PhysicsMaterial Material(float friction, float restitution)
    {
        return new PhysicsMaterial
        {
            staticFriction = friction,
            dynamicFriction = friction,
            bounciness = restitution,
            frictionCombine = PhysicsMaterialCombine.Average,
            bounceCombine = PhysicsMaterialCombine.Average,
        };
    }

    static void Static(World w, string name, Vector3 position, Action<GameObject> addCollider)
    {
        var go = new GameObject(name);
        go.transform.position = position;
        addCollider(go);
        var c = go.GetComponent<Collider>();
        c.contactOffset = ContactOffset;
        w.statics.Add(c);
    }

    /// A dynamic body as scenes.rs's dynamic_at builds it: density 1,000 (mass = density x the
    /// shape's volume, which the callers compute by hand, so the probe's masses only confirm those
    /// volumes; Unity derives the inertia and the centre of mass from the collider at that mass),
    /// damping 0.05, discrete collision detection, Rapier's velocity limits.
    static Rigidbody Dynamic(World w, string kind, Vector3 position, float volume,
                             Action<GameObject> addCollider)
    {
        var go = new GameObject("body");
        go.transform.position = position;
        addCollider(go);
        go.GetComponent<Collider>().contactOffset = ContactOffset;
        var rb = go.AddComponent<Rigidbody>();
        rb.mass = Density * volume;
        rb.linearDamping = Damping;
        rb.angularDamping = Damping;
        rb.maxDepenetrationVelocity = MaxDepenetrationVelocity;
        rb.maxAngularVelocity = MaxAngularVelocity;
        rb.maxLinearVelocity = MaxLinearVelocity;
        rb.interpolation = RigidbodyInterpolation.None;
        rb.collisionDetectionMode = CollisionDetectionMode.Discrete;
        w.bodies.Add(rb);
        w.kinds.Add(kind);
        return rb;
    }

    /// Jolt's PyramidScene with `height` layers (scenes.rs pyramid): a 100 x 2 x 100 floor, 2 m
    /// boxes, each layer 0.5 m above the last and offset half a box on odd layers, never sleeping.
    static void Pyramid(World w, int height)
    {
        var mat = Material(PyramidFriction, 0f);
        Static(w, "floor", new Vector3(0f, -1f, 0f), go =>
        {
            var c = go.AddComponent<BoxCollider>();
            c.size = new Vector3(100f, 2f, 100f);
            c.sharedMaterial = mat;
        });
        const float size = 2f, separation = 0.5f, half = 0.5f * size;
        int h = height;
        for (int i = 0; i < h; i++)
            for (int j = i / 2; j < h - (i + 1) / 2; j++)
                for (int k = i / 2; k < h - (i + 1) / 2; k++)
                {
                    float shift = (i & 1) == 1 ? half : 0f;
                    var p = new Vector3(-h + size * j + shift, 1f + (size + separation) * i,
                        -h + size * k + shift);
                    var rb = Dynamic(w, "box", p, size * size * size, go =>
                    {
                        var c = go.AddComponent<BoxCollider>();
                        c.size = new Vector3(size, size, size);
                        c.sharedMaterial = mat;
                    });
                    rb.sleepThreshold = 0f; // never sleeps, like can_sleep(false)
                }
    }

    /// Jolt's ConvexVsMeshScene (scenes.rs convex_vs_mesh): a 100 x 100-cell sine terrain of 20,000
    /// triangles and 21 x 4 x 21 bodies (box, sphere, capsule, square-pyramid hull) dropped on it;
    /// friction 0.5 and restitution 0.6 everywhere, sleeping allowed.
    static void ConvexVsMesh(World w, float sleepEnergy)
    {
        const int n = 100;
        const float cell = 3f, maxHeight = 5f, center = n * cell / 2f;
        var mat = Material(0.5f, 0.6f);
        var vertices = new Vector3[(n + 1) * (n + 1)];
        for (int x = 0; x <= n; x++)
            for (int z = 0; z <= n; z++)
            {
                // f32 arithmetic as scenes.rs does it; its libm sinf/cosf and System.Math's
                // double results rounded to float may differ in the last bit (a vertex moves by
                // at most about 5e-7 m).
                float height = (float)Math.Sin(x * 50.0f / n) * (float)Math.Cos(z * 50.0f / n);
                vertices[z * (n + 1) + x] = new Vector3(cell * x, maxHeight * height, cell * z);
            }
        var indices = new int[n * n * 6];
        int t = 0;
        for (int x = 0; x < n; x++)
            for (int z = 0; z < n; z++)
            {
                int start = (n + 1) * z + x;
                indices[t++] = start; indices[t++] = start + n + 1; indices[t++] = start + 1;
                indices[t++] = start + 1; indices[t++] = start + n + 1; indices[t++] = start + n + 2;
            }
        var terrain = new Mesh { name = "terrain" };
        terrain.vertices = vertices;
        terrain.triangles = indices;
        Static(w, "terrain", new Vector3(-center, maxHeight, -center), go =>
        {
            var c = go.AddComponent<MeshCollider>();
            c.sharedMesh = terrain;
            c.sharedMaterial = mat;
        });

        // The square-pyramid hull: apex (0, 1, 0) over a square with corners on the axes at 1 m.
        var hull = new Mesh { name = "hull" };
        hull.vertices = new[]
        {
            new Vector3(0f, 1f, 0f), new Vector3(1f, 0f, 0f), new Vector3(-1f, 0f, 0f),
            new Vector3(0f, 0f, 1f), new Vector3(0f, 0f, -1f),
        };
        hull.triangles = new[] { 0, 1, 4, 0, 4, 2, 0, 2, 3, 0, 3, 1, 1, 3, 2, 1, 2, 4 };
        // Volumes for the masses (scenes.rs lets Rapier compute them from density).
        float boxVolume = 1f * 1.5f * 2f;
        float sphereVolume = 4f / 3f * Mathf.PI * 0.125f;
        float capsuleVolume = Mathf.PI * 0.25f * 1.5f + sphereVolume;
        float hullVolume = 2f / 3f; // base area 2 (diagonals 2), height 1
        for (int x = -10; x <= 10; x++)
            for (int y = 0; y < 4; y++)
                for (int z = -10; z <= 10; z++)
                {
                    var p = new Vector3(7.5f * x, 15f + 2f * y, 7.5f * z);
                    Rigidbody rb;
                    switch (y)
                    {
                        case 0:
                            rb = Dynamic(w, "box", p, boxVolume, go =>
                            {
                                var c = go.AddComponent<BoxCollider>();
                                c.size = new Vector3(1f, 1.5f, 2f);
                                c.sharedMaterial = mat;
                            });
                            break;
                        case 1:
                            rb = Dynamic(w, "sphere", p, sphereVolume, go =>
                            {
                                var c = go.AddComponent<SphereCollider>();
                                c.radius = 0.5f;
                                c.sharedMaterial = mat;
                            });
                            break;
                        case 2:
                            rb = Dynamic(w, "capsule", p, capsuleVolume, go =>
                            {
                                var c = go.AddComponent<CapsuleCollider>();
                                c.radius = 0.5f;
                                c.height = 2.5f; // Rapier's capsule_y(0.75, 0.5): segment 1.5 + 2 radii
                                c.direction = 1;
                                c.sharedMaterial = mat;
                            });
                            break;
                        default:
                            rb = Dynamic(w, "hull", p, hullVolume, go =>
                            {
                                var c = go.AddComponent<MeshCollider>();
                                c.sharedMesh = hull;
                                c.convex = true;
                                c.sharedMaterial = mat;
                            });
                            break;
                    }
                    rb.sleepThreshold = sleepEnergy;
                }
    }

    /// scenes.rs ray_grid: 100 x 100 rays 40 m above the terrain, 100 m long, tilted by a pattern.
    static List<(Vector3 origin, Vector3 dir, float length)> RayGrid()
    {
        var rays = new List<(Vector3, Vector3, float)>(10000);
        for (int i = 0; i < 100; i++)
            for (int j = 0; j < 100; j++)
            {
                var origin = new Vector3(-148.5f + 3f * i, 40f, -148.5f + 3f * j);
                var d = new Vector3(0.1f * (i % 7 - 3), -1f, 0.1f * (j % 5 - 2)) * 100f;
                float length = d.magnitude;
                rays.Add((origin, d / length, length));
            }
        return rays;
    }

    struct Summary
    {
        public double mean, p50, p95, max, total;
    }

    // The Rapier bench's summary (main.rs summarize): nearest-rank percentiles.
    static Summary Summarize(List<double> ms)
    {
        if (ms.Count == 0)
            return new Summary();
        var s = new List<double>(ms);
        s.Sort();
        double total = 0;
        foreach (var v in ms)
            total += v;
        double Pct(double p) => s[Math.Min((int)(p * (s.Count - 1) + 0.5), s.Count - 1)];
        return new Summary { mean = total / ms.Count, p50 = Pct(0.5), p95 = Pct(0.95), max = s[s.Count - 1], total = total };
    }

    // The machine's and this process's CPU times (Windows, in 100 ns units), sampled before and
    // after the steps: the whole machine's busy time less this process's is what other processes
    // used. Windows charges these times by scheduler tick (about 15.6 ms), coarse for windows of
    // under a second.
    [DllImport("kernel32.dll")]
    static extern bool GetSystemTimes(out long idle, out long kernel, out long user);
    [DllImport("kernel32.dll")]
    static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll")]
    static extern bool GetProcessTimes(IntPtr process, out long creation, out long exit,
                                       out long kernel, out long user);

    struct CpuSample
    {
        public long idle, total, own;
    }

    static CpuSample? SampleCpu()
    {
        try
        {
            if (!GetSystemTimes(out long idle, out long kernel, out long user))
                return null;
            if (!GetProcessTimes(GetCurrentProcess(), out _, out _, out long pk, out long pu))
                return null;
            // The machine's kernel time includes its idle time.
            return new CpuSample { idle = idle, total = kernel + user, own = pk + pu };
        }
        catch (Exception) // not Windows
        {
            return null;
        }
    }

    /// (whole-machine CPU %, other processes' CPU %) between two samples, or nulls.
    static (string busy, string other) CpuShare(CpuSample? a, CpuSample? b)
    {
        if (a == null || b == null || b.Value.total <= a.Value.total)
            return ("null", "null");
        double total = b.Value.total - a.Value.total, idle = b.Value.idle - a.Value.idle;
        double own = b.Value.own - a.Value.own;
        return (F(100.0 * (total - idle) / total, 1), F(100.0 * (total - idle - own) / total, 1));
    }

    static ulong Fnv(ulong h, float x)
    {
        uint bits = BitConverter.ToUInt32(BitConverter.GetBytes(x), 0);
        for (int b = 0; b < 4; b++)
        {
            h ^= (bits >> (8 * b)) & 0xff;
            h *= 0x00000100000001b3UL;
        }
        return h;
    }

    static string F(double v, int digits) => v.ToString("F" + digits, CultureInfo.InvariantCulture);

    static string V(Vector3 v) => "[" + F(v.x, 4) + "," + F(v.y, 4) + "," + F(v.z, 4) + "]";

    static string Mat(Collider c)
    {
        var m = c.sharedMaterial;
        return m == null ? "null" :
            "{\"friction\":[" + F(m.staticFriction, 3) + "," + F(m.dynamicFriction, 3) +
            "],\"bounciness\":" + F(m.bounciness, 3) + ",\"friction_combine\":\"" + m.frictionCombine +
            "\",\"bounce_combine\":\"" + m.bounceCombine + "\"}";
    }

    /// The world-space bounds' size of the first body of each kind, read before the first step
    /// (no body is rotated yet, so it is the collider's size), for -probe.
    static Dictionary<string, Vector3> InitialSizes(World w)
    {
        var sizes = new Dictionary<string, Vector3>();
        for (int i = 0; i < w.bodies.Count; i++)
            if (!sizes.ContainsKey(w.kinds[i]))
                sizes[w.kinds[i]] = w.bodies[i].GetComponent<Collider>().bounds.size;
        return sizes;
    }

    /// -probe: the settings the run had and, for the first body of each kind, what Unity made of
    /// it (mass, principal inertia, centre of mass, size before the first step, limits, material),
    /// with every kind's count, active bodies and mean height at the end. Everything but the size
    /// is read after the run; reading changes nothing.
    static void Probe(StringBuilder sb, World w, Dictionary<string, Vector3> initialSizes)
    {
        var info = Physics.GetCurrentIntegrationInfo();
        sb.Append(",\"probe\":{\"backend\":\"").Append(info.name)
          .Append("\",\"gravity\":").Append(V(Physics.gravity))
          .Append(",\"bounce_threshold\":").Append(F(Physics.bounceThreshold, 4))
          .Append(",\"default_sleep_threshold\":").Append(F(Physics.sleepThreshold, 5))
          .Append(",\"default_contact_offset\":").Append(F(Physics.defaultContactOffset, 4))
          .Append(",\"default_max_depenetration\":").Append(F(Physics.defaultMaxDepenetrationVelocity, 3))
          .Append(",\"default_max_angular_speed\":").Append(F(Physics.defaultMaxAngularSpeed, 3))
          .Append(",\"improved_patch_friction\":").Append(Physics.improvedPatchFriction ? "true" : "false")
          .Append(",\"queries_hit_backfaces\":").Append(Physics.queriesHitBackfaces ? "true" : "false")
          .Append(",\"simulation_mode\":\"").Append(Physics.simulationMode).Append('"')
          .Append(",\"statics\":[");
        for (int i = 0; i < w.statics.Count; i++)
        {
            var c = w.statics[i];
            sb.Append(i > 0 ? "," : "").Append("{\"name\":\"").Append(c.gameObject.name)
              .Append("\",\"type\":\"").Append(c.GetType().Name)
              .Append("\",\"contact_offset\":").Append(F(c.contactOffset, 4))
              .Append(",\"material\":").Append(Mat(c)).Append('}');
        }
        sb.Append("],\"kinds\":{");
        var seen = new List<string>();
        for (int i = 0; i < w.bodies.Count; i++)
        {
            string kind = w.kinds[i];
            if (seen.Contains(kind))
                continue;
            seen.Add(kind);
            int count = 0, active = 0;
            double sumY = 0;
            for (int j = 0; j < w.bodies.Count; j++)
                if (w.kinds[j] == kind)
                {
                    count++;
                    sumY += w.bodies[j].position.y;
                    if (!w.bodies[j].IsSleeping())
                        active++;
                }
            var rb = w.bodies[i];
            var c = rb.GetComponent<Collider>();
            sb.Append(seen.Count > 1 ? "," : "").Append('"').Append(kind).Append("\":{")
              .Append("\"count\":").Append(count)
              .Append(",\"active_at_end\":").Append(active)
              .Append(",\"mean_y\":").Append(F(sumY / count, 3))
              .Append(",\"collider\":\"").Append(c.GetType().Name)
              .Append("\",\"mass\":").Append(F(rb.mass, 3))
              .Append(",\"inertia\":").Append(V(rb.inertiaTensor))
              .Append(",\"inertia_rotation\":[").Append(F(rb.inertiaTensorRotation.x, 4)).Append(',')
              .Append(F(rb.inertiaTensorRotation.y, 4)).Append(',').Append(F(rb.inertiaTensorRotation.z, 4))
              .Append(',').Append(F(rb.inertiaTensorRotation.w, 4)).Append(']')
              .Append(",\"center_of_mass\":").Append(V(rb.centerOfMass))
              .Append(",\"initial_size\":").Append(V(initialSizes[kind]))
              .Append(",\"linear_damping\":").Append(F(rb.linearDamping, 4))
              .Append(",\"angular_damping\":").Append(F(rb.angularDamping, 4))
              .Append(",\"max_depenetration\":").Append(F(rb.maxDepenetrationVelocity, 3))
              .Append(",\"max_angular_velocity\":").Append(F(rb.maxAngularVelocity, 3))
              .Append(",\"max_linear_velocity\":").Append(F(rb.maxLinearVelocity, 3))
              .Append(",\"sleep_threshold\":").Append(F(rb.sleepThreshold, 5))
              .Append(",\"solver_iterations\":").Append(rb.solverIterations)
              .Append(",\"solver_velocity_iterations\":").Append(rb.solverVelocityIterations)
              .Append(",\"collision_detection\":\"").Append(rb.collisionDetectionMode)
              .Append("\",\"interpolation\":\"").Append(rb.interpolation)
              .Append("\",\"contact_offset\":").Append(F(c.contactOffset, 4))
              .Append(",\"material\":").Append(Mat(c)).Append('}');
        }
        sb.Append("}}");
    }

    /// Builds the scene, steps it, appends the JSON line to o.output and returns it.
    public static string Run(Options o)
    {
        Physics.simulationMode = SimulationMode.Script;
        Physics.gravity = new Vector3(0f, -9.81f, 0f);
        if (o.positionIterations > 0)
            Physics.defaultSolverIterations = o.positionIterations;
        if (o.velocityIterations > 0)
            Physics.defaultSolverVelocityIterations = o.velocityIterations;
        int workers = Math.Max(0, Math.Min(o.threads - 1, JobsUtility.JobWorkerMaximumCount));
        JobsUtility.JobWorkerCount = workers;

        var w = new World();
        var setup = Stopwatch.StartNew();
        List<(Vector3 origin, Vector3 dir, float length)> rays = null;
        switch (o.scene)
        {
            case "Pyramid": Pyramid(w, 15); break;
            case "Pyramid30": Pyramid(w, 30); break;
            case "ConvexVsMesh": ConvexVsMesh(w, o.sleepEnergy); break;
            case "Raycast": ConvexVsMesh(w, o.sleepEnergy); rays = RayGrid(); break;
            default: throw new ArgumentException("unknown scene " + o.scene);
        }
        Physics.SyncTransforms();
        double setupMs = setup.Elapsed.TotalMilliseconds;
        var initialSizes = o.probe ? InitialSizes(w) : null;

        var stepMs = new List<double>(o.steps);
        var rayMs = new List<double>();
        long hits = 0;
        double fractionSum = 0;
        var timer = new Stopwatch();
        var cpuBefore = SampleCpu();
        for (int i = 0; i < o.steps; i++)
        {
            timer.Restart();
            Physics.Simulate(Dt);
            stepMs.Add(timer.Elapsed.TotalMilliseconds);
            if (rays != null)
            {
                timer.Restart();
                foreach (var r in rays)
                {
                    if (Physics.Raycast(r.origin, r.dir, out RaycastHit hit, r.length, Physics.DefaultRaycastLayers, QueryTriggerInteraction.Ignore))
                    {
                        hits++;
                        fractionSum += hit.distance / r.length;
                    }
                }
                rayMs.Add(timer.Elapsed.TotalMilliseconds);
            }
        }
        var cpu = CpuShare(cpuBefore, SampleCpu());

        var s = Summarize(stepMs);
        var warm = Summarize(stepMs.GetRange(Math.Min(1, stepMs.Count), Math.Max(0, stepMs.Count - 1)));
        float topY = float.MinValue;
        double sumY = 0;
        int active = 0;
        ulong hash = 0xcbf29ce484222325UL;
        foreach (var rb in w.bodies)
        {
            var p = rb.position;
            var q = rb.rotation;
            topY = Mathf.Max(topY, p.y);
            sumY += p.y;
            if (!rb.IsSleeping())
                active++;
            foreach (var v in new[] { p.x, p.y, p.z, q.x, q.y, q.z, q.w })
                hash = Fnv(hash, v);
        }
        double meanY = sumY / Math.Max(1, w.bodies.Count);
        int bodies = w.bodies.Count + w.statics.Count;

        var sb = new StringBuilder();
        sb.Append("{\"engine\":\"physx\",\"version\":\"Unity ").Append(Application.unityVersion)
          .Append("\",\"deterministic\":false,\"config\":\"unity ").Append(o.mode).Append(' ').Append(o.solver)
          .Append("\",\"solver\":\"").Append(o.solver)
          .Append("\",\"scripting_backend\":\"").Append(ScriptingBackend)
          .Append("\",\"development\":").Append(Debug.isDebugBuild ? "true" : "false")
          .Append(",\"scene\":\"").Append(o.scene)
          .Append("\",\"threads\":").Append(o.threads)
          .Append(",\"job_workers\":").Append(JobsUtility.JobWorkerCount)
          .Append(",\"job_worker_max\":").Append(JobsUtility.JobWorkerMaximumCount)
          .Append(",\"steps\":").Append(o.steps)
          .Append(",\"solver_iterations\":").Append(Physics.defaultSolverIterations)
          .Append(",\"velocity_iterations\":").Append(Physics.defaultSolverVelocityIterations)
          .Append(",\"bodies\":").Append(bodies)
          .Append(",\"colliders\":").Append(bodies)
          .Append(",\"active_at_end\":").Append(active)
          .Append(",\"setup_ms\":").Append(F(setupMs, 1))
          .Append(",\"mean_ms\":").Append(F(s.mean, 4))
          .Append(",\"p50_ms\":").Append(F(s.p50, 4))
          .Append(",\"p95_ms\":").Append(F(s.p95, 4))
          .Append(",\"max_ms\":").Append(F(s.max, 4))
          .Append(",\"total_ms\":").Append(F(s.total, 2))
          .Append(",\"first_step_ms\":").Append(F(stepMs.Count > 0 ? stepMs[0] : 0, 4))
          .Append(",\"mean_ms_after_first\":").Append(F(warm.mean, 4))
          .Append(",\"top_y\":").Append(F(topY, 3))
          .Append(",\"mean_y\":").Append(F(meanY, 3))
          .Append(",\"sleep_threshold\":").Append(F(rays != null || o.scene == "ConvexVsMesh" ? o.sleepEnergy : 0f, 5))
          .Append(",\"steps_cpu_busy_pct\":").Append(cpu.busy)
          .Append(",\"steps_cpu_other_pct\":").Append(cpu.other);
        if (rays != null)
        {
            var r = Summarize(rayMs);
            sb.Append(",\"rays_per_step\":").Append(rays.Count)
              .Append(",\"ray_mean_ms\":").Append(F(r.mean, 4))
              .Append(",\"ray_p95_ms\":").Append(F(r.p95, 4))
              .Append(",\"ray_total_ms\":").Append(F(r.total, 2))
              .Append(",\"ray_hits\":").Append(hits)
              .Append(",\"ray_fraction_sum\":").Append(F(fractionSum, 3));
        }
        if (o.probe)
            Probe(sb, w, initialSizes);
        sb.Append(",\"hash\":\"0x").Append(hash.ToString("x16")).Append("\"}");
        string line = sb.ToString();
        if (!string.IsNullOrEmpty(o.output))
            File.AppendAllText(o.output, line + "\n");
        Debug.Log("physxbench " + line);
        return line;
    }
}

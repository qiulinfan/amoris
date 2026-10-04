// The Jolt side of aipocket2's physics benchmark (docs/bench/physics.md).
//
// Runs Jolt's own PerformanceTest scenes (Pyramid, ConvexVsMesh, Ragdoll; their headers are
// included unchanged from JoltPhysics/PerformanceTest) plus two scenes the Rapier side also builds:
// Pyramid30 (the same pyramid, 30 layers, 9,455 boxes) and Raycast (ConvexVsMesh plus 10,000 ray
// casts after every step). Timing is per step, like PerformanceTest's -f option; the summary is one
// JSON line. --export writes the scene's bodies, shapes and constraints after setup, as text, so the
// Rapier side can rebuild the Ragdoll scene from what Jolt actually created.
//
//   jolt_bench --scene <Pyramid|Pyramid30|ConvexVsMesh|Ragdoll|Raycast> [--threads N] [--steps N]
//              [--rays] [--no-sleep] [--vel N] [--pos N] [--csv per_step.csv] [--export scene.txt]
//              [--fork-at N]
//
// --rays casts the Raycast scene's 10,000 rays after every step of any scene; --vel and --pos set
// Jolt's velocity and position iteration counts. --fork-at N is the snapshot-and-fork check
// aipocket2's persistence needs: after N steps the whole simulation state is saved
// (PhysicsSystem::SaveState, all of it: bodies, contacts with their warm-start impulses,
// constraints), the scene is built again in a fresh PhysicsSystem, the state restored into it, and
// both systems step to the end; the fork's end hash is reported beside the original's.

#include <Jolt/Jolt.h>
#include <Jolt/ConfigurationString.h>
#include <Jolt/RegisterTypes.h>
#include <Jolt/Core/Factory.h>
#include <Jolt/Core/TempAllocator.h>
#include <Jolt/Core/JobSystemThreadPool.h>
#include <Jolt/Core/JobSystemSingleThreaded.h>
#include <Jolt/Physics/PhysicsSettings.h>
#include <Jolt/Physics/PhysicsSystem.h>
#include <Jolt/Physics/StateRecorderImpl.h>
#include <Jolt/Physics/Body/BodyCreationSettings.h>
#include <Jolt/Physics/Collision/RayCast.h>
#include <Jolt/Physics/Collision/CastResult.h>
#include <Jolt/Physics/Collision/GroupFilterTable.h>
#include <Jolt/Physics/Collision/Shape/BoxShape.h>
#include <Jolt/Physics/Collision/Shape/SphereShape.h>
#include <Jolt/Physics/Collision/Shape/CapsuleShape.h>
#include <Jolt/Physics/Collision/Shape/TaperedCapsuleShape.h>
#include <Jolt/Physics/Collision/Shape/CompoundShape.h>
#include <Jolt/Physics/Collision/Shape/RotatedTranslatedShape.h>
#include <Jolt/Physics/Constraints/SwingTwistConstraint.h>

JPH_SUPPRESS_WARNINGS_STD_BEGIN
#include <algorithm>
#include <chrono>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <iostream>
#include <memory>
#include <string>
#include <thread>
#include <vector>
#include <time.h>
JPH_SUPPRESS_WARNINGS_STD_END

using namespace JPH;
using namespace JPH::literals;
using namespace std;

JPH_SUPPRESS_WARNINGS

#include "Layers.h"
#include "PerformanceTestScene.h"
#include "PyramidScene.h"
#include "ConvexVsMeshScene.h"
#include "RagdollScene.h"

static constexpr float cDeltaTime = 1.0f / 60.0f;

// Jolt's PyramidScene with the height as a parameter; 30 layers make 9,455 boxes.
class PyramidNScene : public PerformanceTestScene
{
public:
	explicit PyramidNScene(int inHeight) : mHeight(inHeight) { mName = "Pyramid" + to_string(inHeight); }

	virtual const char *GetName() const override { return mName.c_str(); }
	virtual size_t GetTempAllocatorSizeMB() const override { return 256; }
	virtual uint GetMaxBodies() const override { return 65536; }
	virtual uint GetMaxBodyPairs() const override { return 262144; }
	virtual uint GetMaxContactConstraints() const override { return 131072; }

	virtual void StartTest(PhysicsSystem &inPhysicsSystem, EMotionQuality inMotionQuality) override
	{
		BodyInterface &bi = inPhysicsSystem.GetBodyInterface();
		bi.CreateAndAddBody(BodyCreationSettings(new BoxShape(Vec3(50.0f, 1.0f, 50.0f), 0.0f), RVec3(Vec3(0.0f, -1.0f, 0.0f)), Quat::sIdentity(), EMotionType::Static, Layers::NON_MOVING), EActivation::DontActivate);

		const float cBoxSize = 2.0f;
		const float cBoxSeparation = 0.5f;
		const float cHalfBoxSize = 0.5f * cBoxSize;
		const int h = mHeight;
		RefConst<Shape> box_shape = new BoxShape(Vec3::sReplicate(cHalfBoxSize), 0.0f);
		for (int i = 0; i < h; ++i)
			for (int j = i / 2; j < h - (i + 1) / 2; ++j)
				for (int k = i / 2; k < h - (i + 1) / 2; ++k)
				{
					RVec3 position(Real(-h + cBoxSize * j + (i & 1? cHalfBoxSize : 0.0f)), Real(1.0f + (cBoxSize + cBoxSeparation) * i), Real(-h + cBoxSize * k + (i & 1? cHalfBoxSize : 0.0f)));
					BodyCreationSettings settings(box_shape, position, Quat::sIdentity(), EMotionType::Dynamic, Layers::MOVING);
					settings.mMotionQuality = inMotionQuality;
					settings.mAllowSleeping = false;
					bi.CreateAndAddBody(settings, EActivation::Activate);
				}
	}

private:
	int mHeight;
	string mName;
};

// The 10,000 rays of the Raycast scene: a 100 x 100 grid 40 m above ConvexVsMesh's terrain, each ray
// 100 m long and tilted by a small pattern so that rays are not all parallel. The Rapier side casts
// the same rays (bench/physics/rapier/src/scenes.rs, ray_grid).
static void MakeRays(Array<RRayCast> &outRays)
{
	outRays.clear();
	for (int i = 0; i < 100; ++i)
		for (int j = 0; j < 100; ++j)
		{
			Vec3 origin(-148.5f + 3.0f * i, 40.0f, -148.5f + 3.0f * j);
			Vec3 dir(0.1f * float(i % 7 - 3), -1.0f, 0.1f * float(j % 5 - 2));
			outRays.push_back(RRayCast { RVec3(origin), 100.0f * dir });
		}
}

// CPU time of the calling thread. With one thread the whole step runs on it, so this is the step's
// cost without the time the scheduler gave other processes (the benchmark machine is shared).
static double ThreadCpuMs()
{
#ifdef CLOCK_THREAD_CPUTIME_ID
	timespec ts;
	clock_gettime(CLOCK_THREAD_CPUTIME_ID, &ts);
	return double(ts.tv_sec) * 1.0e3 + double(ts.tv_nsec) * 1.0e-6;
#else
	return 0.0; // wasi-libc has no thread CPU clock
#endif
}

struct Stats
{
	double mean = 0, p50 = 0, p95 = 0, max = 0, total = 0;
};

static Stats Summarize(vector<double> inMs)
{
	Stats s;
	if (inMs.empty())
		return s;
	for (double v : inMs)
		s.total += v;
	s.mean = s.total / double(inMs.size());
	sort(inMs.begin(), inMs.end());
	auto pct = [&](double p) { size_t idx = size_t(p * double(inMs.size() - 1) + 0.5); return inMs[min(idx, inMs.size() - 1)]; };
	s.p50 = pct(0.5);
	s.p95 = pct(0.95);
	s.max = inMs.back();
	return s;
}

// --- Export -----------------------------------------------------------------------------------------

static void WriteVec(ostream &o, Vec3Arg v) { o << ' ' << v.GetX() << ' ' << v.GetY() << ' ' << v.GetZ(); }
static void WriteQuat(ostream &o, QuatArg q) { o << ' ' << q.GetX() << ' ' << q.GetY() << ' ' << q.GetZ() << ' ' << q.GetW(); }

// A leaf shape in its body's centre-of-mass frame.
static void ExportLeaves(ostream &o, const Shape *inShape, Mat44Arg inComXf, int &ioCount, bool inCountOnly)
{
	switch (inShape->GetSubType())
	{
	case EShapeSubType::StaticCompound:
	case EShapeSubType::MutableCompound:
	{
		const CompoundShape *c = static_cast<const CompoundShape *>(inShape);
		for (const CompoundShape::SubShape &sub : c->GetSubShapes())
			ExportLeaves(o, sub.mShape, inComXf * Mat44::sRotationTranslation(sub.GetRotation(), sub.GetPositionCOM()), ioCount, inCountOnly);
		return;
	}
	case EShapeSubType::RotatedTranslated:
	{
		const RotatedTranslatedShape *r = static_cast<const RotatedTranslatedShape *>(inShape);
		ExportLeaves(o, r->GetInnerShape(), inComXf * Mat44::sRotation(r->GetRotation()), ioCount, inCountOnly);
		return;
	}
	default:
		break;
	}

	++ioCount;
	if (inCountOnly)
		return;
	switch (inShape->GetSubType())
	{
	case EShapeSubType::Box:
	{
		const BoxShape *b = static_cast<const BoxShape *>(inShape);
		o << "box";
		WriteVec(o, b->GetHalfExtent());
		o << ' ' << b->GetConvexRadius();
		WriteVec(o, inComXf.GetTranslation());
		WriteQuat(o, inComXf.GetQuaternion());
		o << '\n';
		break;
	}
	case EShapeSubType::Sphere:
	{
		const SphereShape *s = static_cast<const SphereShape *>(inShape);
		o << "sphere " << s->GetRadius();
		WriteVec(o, inComXf.GetTranslation());
		o << '\n';
		break;
	}
	case EShapeSubType::Capsule:
	{
		const CapsuleShape *c = static_cast<const CapsuleShape *>(inShape);
		float hh = c->GetHalfHeightOfCylinder();
		o << "capsule";
		WriteVec(o, inComXf * Vec3(0, hh, 0));
		WriteVec(o, inComXf * Vec3(0, -hh, 0));
		o << ' ' << c->GetRadius() << ' ' << c->GetRadius() << '\n';
		break;
	}
	case EShapeSubType::TaperedCapsule:
	{
		// Sphere centres in the leaf's centre-of-mass frame (TaperedCapsuleShape.cpp: mTopCenter and
		// mBottomCenter are H +- ... + (rb - rt) / 2 there).
		const TaperedCapsuleShape *t = static_cast<const TaperedCapsuleShape *>(inShape);
		float h = t->GetHalfHeight(), rt = t->GetTopRadius(), rb = t->GetBottomRadius();
		float shift = 0.5f * (rb - rt);
		o << "capsule";
		WriteVec(o, inComXf * Vec3(0, h + shift, 0));
		WriteVec(o, inComXf * Vec3(0, -h + shift, 0));
		o << ' ' << rt << ' ' << rb << '\n';
		break;
	}
	default:
		o << "unsupported " << int(inShape->GetSubType()) << '\n';
		break;
	}
}

static void ExportScene(PhysicsSystem &inSystem, const char *inPath)
{
	ofstream o(inPath);
	o.precision(9);
	const BodyLockInterface &bli = inSystem.GetBodyLockInterfaceNoLock();
	BodyIDVector ids;
	inSystem.GetBodies(ids);
	sort(ids.begin(), ids.end());

	// Static bodies as world-space triangles, per leaf shape (compound and decorated shapes cannot
	// list triangles themselves, so their leaves are collected first). Each leaf is flagged convex or
	// not, so the Rapier side can make convex leaves convex hulls.
	struct LeafCollector : public TransformedShapeCollector
	{
		virtual void AddHit(const TransformedShape &inShape) override { mLeaves.push_back(inShape); }
		vector<TransformedShape> mLeaves;
	};
	AABox everything(Vec3::sReplicate(-1.0e5f), Vec3::sReplicate(1.0e5f));
	vector<Float3> tris;
	vector<pair<int, size_t>> static_counts;
	for (BodyID id : ids)
	{
		BodyLockRead lock(bli, id);
		const Body &b = lock.GetBody();
		if (!b.IsStatic())
			continue;
		LeafCollector leaves;
		b.GetTransformedShape().CollectTransformedShapes(everything, leaves);
		for (const TransformedShape &ts : leaves.mLeaves)
		{
			size_t before = tris.size();
			Shape::GetTrianglesContext ctx;
			ts.GetTrianglesStart(ctx, everything, RVec3::sZero());
			Float3 buf[3 * 256];
			for (;;)
			{
				int n = ts.GetTrianglesNext(ctx, 256, buf);
				if (n == 0)
					break;
				tris.insert(tris.end(), buf, buf + 3 * n);
			}
			bool convex = ts.mShape->GetType() == EShapeType::Convex;
			static_counts.push_back({ convex? 1 : 0, (tris.size() - before) / 3 });
		}
	}

	// The triangles go to <path>.tri as little-endian f32 (x, y, z per vertex, 3 vertices each): the
	// Ragdoll terrain has 2.1 million of them.
	string tri_path = string(inPath) + ".tri";
	ofstream t(tri_path, ios::binary);
	t.write(reinterpret_cast<const char *>(tris.data()), streamsize(tris.size() * sizeof(Float3)));
	o << "statics " << static_counts.size() << ' ' << tris.size() / 3 << '\n';
	for (auto &c : static_counts)
		o << c.first << ' ' << c.second << '\n';

	// Dynamic bodies: pose of the centre of mass, mass properties, material, damping, collision group
	// and leaf shapes in the centre-of-mass frame.
	UnorderedMap<BodyID, int> index;
	vector<BodyID> dyn;
	for (BodyID id : ids)
	{
		BodyLockRead lock(bli, id);
		if (lock.GetBody().IsDynamic())
		{
			index[id] = int(dyn.size());
			dyn.push_back(id);
		}
	}
	o << "bodies " << dyn.size() << '\n';
	const GroupFilterTable *table = nullptr;
	int num_sub_groups = 0;
	for (BodyID id : dyn)
	{
		BodyLockRead lock(bli, id);
		const Body &b = lock.GetBody();
		const MotionProperties *mp = b.GetMotionProperties();
		const CollisionGroup &cg = b.GetCollisionGroup();
		if (cg.GetGroupFilter() != nullptr)
		{
			table = static_cast<const GroupFilterTable *>(cg.GetGroupFilter());
			num_sub_groups = max(num_sub_groups, int(cg.GetSubGroupID()) + 1);
		}
		int nleaves = 0;
		ExportLeaves(o, b.GetShape(), Mat44::sIdentity(), nleaves, true);
		o << "body";
		WriteVec(o, Vec3(b.GetCenterOfMassPosition()));
		WriteQuat(o, b.GetRotation());
		o << ' ' << 1.0f / mp->GetInverseMass();
		WriteVec(o, mp->GetInverseInertiaDiagonal());
		WriteQuat(o, mp->GetInertiaRotation());
		o << ' ' << b.GetFriction() << ' ' << b.GetRestitution() << ' ' << mp->GetLinearDamping() << ' ' << mp->GetAngularDamping()
		  << ' ' << mp->GetMaxAngularVelocity() << ' ' << cg.GetGroupID() << ' ' << cg.GetSubGroupID() << ' ' << (b.GetAllowSleeping()? 1 : 0)
		  << ' ' << nleaves << '\n';
		nleaves = 0;
		ExportLeaves(o, b.GetShape(), Mat44::sIdentity(), nleaves, false);
	}

	// The ragdoll's group filter: which sub-group pairs do not collide.
	if (table != nullptr)
	{
		vector<pair<int, int>> off;
		for (int i = 0; i < num_sub_groups; ++i)
			for (int j = i + 1; j < num_sub_groups; ++j)
				if (!table->IsCollisionEnabled(CollisionGroup::SubGroupID(i), CollisionGroup::SubGroupID(j)))
					off.push_back({ i, j });
		o << "nocollide " << off.size() << '\n';
		for (auto &p : off)
			o << p.first << ' ' << p.second << '\n';
	}
	else
		o << "nocollide 0\n";

	// Swing-twist constraints: frames relative to each body's centre of mass, limits, motors.
	Constraints constraints = inSystem.GetConstraints();
	vector<const SwingTwistConstraint *> st;
	for (const Ref<Constraint> &c : constraints)
		if (c->GetSubType() == EConstraintSubType::SwingTwist)
			st.push_back(static_cast<const SwingTwistConstraint *>(c.GetPtr()));
	o << "swingtwist " << st.size() << '\n';
	for (const SwingTwistConstraint *c : st)
	{
		o << index[c->GetBody1()->GetID()] << ' ' << index[c->GetBody2()->GetID()];
		WriteVec(o, c->GetLocalSpacePosition1());
		WriteQuat(o, c->GetConstraintToBody1());
		WriteVec(o, c->GetLocalSpacePosition2());
		WriteQuat(o, c->GetConstraintToBody2());
		o << ' ' << c->GetNormalHalfConeAngle() << ' ' << c->GetPlaneHalfConeAngle() << ' ' << c->GetTwistMinAngle() << ' ' << c->GetTwistMaxAngle();
		const MotorSettings &sw = c->GetSwingMotorSettings(), &tw = c->GetTwistMotorSettings();
		o << ' ' << int(sw.mSpringSettings.mMode) << ' ' << sw.mSpringSettings.mFrequency << ' ' << sw.mSpringSettings.mDamping << ' ' << sw.mMaxTorqueLimit;
		o << ' ' << int(tw.mSpringSettings.mMode) << ' ' << tw.mSpringSettings.mFrequency << ' ' << tw.mSpringSettings.mDamping << ' ' << tw.mMaxTorqueLimit;
		o << ' ' << int(c->GetSwingMotorState()) << ' ' << int(c->GetTwistMotorState());
		WriteQuat(o, c->GetTargetOrientationCS());
		o << ' ' << c->GetMaxFrictionTorque() << '\n';
	}
	o << "end\n";
}

// --- Main -------------------------------------------------------------------------------------------

static void TraceImpl(const char *inFMT, ...)
{
	va_list list;
	va_start(list, inFMT);
	char buffer[1024];
	vsnprintf(buffer, sizeof(buffer), inFMT, list);
	va_end(list);
	cerr << buffer << endl;
}

int main(int argc, char **argv)
{
	string scene_name = "Pyramid";
	int threads = 1;
	int steps = 500;
	const char *csv = nullptr;
	const char *export_path = nullptr;
	int velocity_steps = -1, position_steps = -1;
	bool rays_flag = false;
	bool no_sleep = false;
	int fork_at = -1;
	for (int i = 1; i < argc; ++i)
	{
		string a = argv[i];
		auto next = [&]() { return i + 1 < argc? argv[++i] : (char *)""; };
		if (a == "--scene") scene_name = next();
		else if (a == "--threads") { string t = next(); threads = t == "max"? int(max(1u, thread::hardware_concurrency())) : atoi(t.c_str()); }
		else if (a == "--steps") steps = atoi(next());
		else if (a == "--csv") csv = next();
		else if (a == "--export") export_path = next();
		else if (a == "--rays") rays_flag = true;
		else if (a == "--no-sleep") no_sleep = true;
		else if (a == "--vel") velocity_steps = atoi(next());
		else if (a == "--pos") position_steps = atoi(next());
		else if (a == "--fork-at") fork_at = atoi(next());
		else { cerr << "unknown argument " << a << endl; return 2; }
	}

	Trace = TraceImpl;
	RegisterDefaultAllocator();
	Factory::sInstance = new Factory();
	RegisterTypes();

	auto make_scene = [](const string &inName) -> PerformanceTestScene * {
		if (inName == "Pyramid") return new PyramidScene;
		if (inName == "Pyramid30") return new PyramidNScene(30);
		if (inName == "ConvexVsMesh" || inName == "Raycast") return new ConvexVsMeshScene;
		if (inName == "Ragdoll") return new RagdollScene(4, 10, 0.6f);
		return nullptr;
	};
	unique_ptr<PerformanceTestScene> scene(make_scene(scene_name));
	if (scene == nullptr) { cerr << "unknown scene " << scene_name << endl; return 2; }
	bool raycast = scene_name == "Raycast";
	const string base_scene_name = scene_name;
	if (!scene->Load(JOLT_ASSETS))
		return 1;

	TempAllocatorImpl temp_allocator(scene->GetTempAllocatorSizeMB() * 1024 * 1024);
	BPLayerInterfaceImpl bp_interface;
	ObjectVsBroadPhaseLayerFilterImpl obj_vs_bp;
	ObjectLayerPairFilterImpl obj_vs_obj;
#ifdef JPH_PLATFORM_WASM
	// The wasm32-wasip1 build (bench/physics/jolt/wasi) has no threads.
	JobSystemSingleThreaded job_system(cMaxPhysicsJobs);
	threads = 1;
#else
	JobSystemThreadPool job_system(cMaxPhysicsJobs, cMaxPhysicsBarriers, threads - 1);
#endif
	// Builds a scene into a PhysicsSystem with this run's options (also for the fork of --fork-at).
	auto prepare = [&](PhysicsSystem &ioSystem, PerformanceTestScene &ioScene) {
		ioSystem.Init(ioScene.GetMaxBodies(), 0, ioScene.GetMaxBodyPairs(), ioScene.GetMaxContactConstraints(), bp_interface, obj_vs_bp, obj_vs_obj);
		ioScene.StartTest(ioSystem, EMotionQuality::Discrete);
		if (no_sleep)
		{
			// PerformanceTest's -no_sleep: every body stays awake, so the work per step does not
			// depend on how soon each engine lets bodies sleep.
			const BodyLockInterface &bli = ioSystem.GetBodyLockInterfaceNoLock();
			BodyIDVector body_ids;
			ioSystem.GetBodies(body_ids);
			for (BodyID id : body_ids)
			{
				BodyLockWrite lock(bli, id);
				if (lock.Succeeded() && !lock.GetBody().IsStatic())
					lock.GetBody().SetAllowSleeping(false);
			}
		}
		ioSystem.OptimizeBroadPhase();
		if (velocity_steps > 0 || position_steps >= 0)
		{
			PhysicsSettings ps = ioSystem.GetPhysicsSettings();
			if (velocity_steps > 0)
				ps.mNumVelocitySteps = uint(velocity_steps);
			if (position_steps >= 0)
				ps.mNumPositionSteps = uint(position_steps);
			ioSystem.SetPhysicsSettings(ps);
		}
	};
	PhysicsSystem physics_system;
	prepare(physics_system, *scene);
	if (no_sleep)
		scene_name += "NoSleep"; // the scene reports as <Scene>NoSleep

	if (export_path != nullptr)
	{
		ExportScene(physics_system, export_path);
		cerr << "exported " << scene_name << " to " << export_path << endl;
		scene->StopTest(physics_system);
		return 0;
	}

	raycast = raycast || rays_flag;
	Array<RRayCast> rays;
	if (raycast)
		MakeRays(rays);
	const NarrowPhaseQuery &npq = physics_system.GetNarrowPhaseQuery();

	// PerformanceTest's hash: positions and rotations of every body, in body id order.
	auto body_hash = [](PhysicsSystem &inSystem) {
		uint64 h = HashBytes(nullptr, 0);
		BodyInterface &bi = inSystem.GetBodyInterfaceNoLock();
		BodyIDVector ids;
		inSystem.GetBodies(ids);
		for (BodyID id : ids)
		{
			RVec3 pos = bi.GetPosition(id);
			h = HashBytes(&pos, 3 * sizeof(Real), h);
			Quat rot = bi.GetRotation(id);
			h = HashBytes(&rot, sizeof(Quat), h);
		}
		return h;
	};

	// --fork-at: the fork's scene, system and the saved state.
	unique_ptr<PerformanceTestScene> fork_scene;
	unique_ptr<PhysicsSystem> fork_system;
	StateRecorderImpl fork_state;
	bool fork_ok = false;
	double save_ms = 0, rebuild_ms = 0, restore_ms = 0;

	vector<double> step_ms, ray_ms, cpu_ms;
	step_ms.reserve(steps);
	uint64 hits = 0;
	double fraction_sum = 0;
	for (int it = 0; it < steps; ++it)
	{
		if (it == fork_at)
		{
			auto f0 = chrono::high_resolution_clock::now();
			physics_system.SaveState(fork_state);
			auto f1 = chrono::high_resolution_clock::now();
			fork_scene.reset(make_scene(base_scene_name));
			fork_scene->Load(JOLT_ASSETS);
			fork_system = make_unique<PhysicsSystem>();
			prepare(*fork_system, *fork_scene);
			auto f2 = chrono::high_resolution_clock::now();
			fork_ok = fork_system->RestoreState(fork_state);
			auto f3 = chrono::high_resolution_clock::now();
			save_ms = chrono::duration<double, milli>(f1 - f0).count();
			rebuild_ms = chrono::duration<double, milli>(f2 - f1).count();
			restore_ms = chrono::duration<double, milli>(f3 - f2).count();
		}
		double c0 = ThreadCpuMs();
		auto t0 = chrono::high_resolution_clock::now();
		scene->UpdateTest(physics_system, temp_allocator, cDeltaTime);
		physics_system.Update(cDeltaTime, 1, &temp_allocator, &job_system);
		auto t1 = chrono::high_resolution_clock::now();
		cpu_ms.push_back(ThreadCpuMs() - c0);
		step_ms.push_back(chrono::duration<double, milli>(t1 - t0).count());
		if (fork_system != nullptr)
			fork_system->Update(cDeltaTime, 1, &temp_allocator, &job_system); // not timed

		if (raycast)
		{
			auto r0 = chrono::high_resolution_clock::now();
			for (const RRayCast &ray : rays)
			{
				RayCastResult hit;
				if (npq.CastRay(ray, hit))
				{
					++hits;
					fraction_sum += hit.mFraction;
				}
			}
			auto r1 = chrono::high_resolution_clock::now();
			ray_ms.push_back(chrono::duration<double, milli>(r1 - r0).count());
		}
	}

	uint64 hash = body_hash(physics_system);
	BodyInterface &bi = physics_system.GetBodyInterfaceNoLock();
	BodyIDVector body_ids;
	physics_system.GetBodies(body_ids);
	uint num_active = physics_system.GetNumActiveBodies(EBodyType::RigidBody);
	float top_y = -FLT_MAX;
	double sum_y = 0;
	int n_dyn = 0;
	for (BodyID id : body_ids)
		if (bi.GetMotionType(id) == EMotionType::Dynamic)
		{
			float y = float(bi.GetCenterOfMassPosition(id).GetY());
			top_y = max(top_y, y);
			sum_y += y;
			++n_dyn;
		}

	if (csv != nullptr)
	{
		ofstream f(csv);
		f << "step,step_ms" << (raycast? ",ray_ms" : "") << '\n';
		for (size_t i = 0; i < step_ms.size(); ++i)
		{
			f << i << ',' << step_ms[i];
			if (raycast)
				f << ',' << ray_ms[i];
			f << '\n';
		}
	}

	Stats s = Summarize(step_ms);
	// The first step builds contact caches and islands from nothing; report it apart.
	Stats warm = Summarize(vector<double>(step_ms.begin() + min<size_t>(1, step_ms.size()), step_ms.end()));
	string config = GetConfigurationString();
	while (!config.empty() && config.back() == ' ')
		config.pop_back();
	printf("{\"engine\":\"jolt\",\"version\":\"%d.%d.%d\",\"deterministic\":%s,\"config\":\"%s\",\"scene\":\"%s\",\"threads\":%d,\"steps\":%d,"
		   "\"bodies\":%u,\"active_at_end\":%u,\"mean_ms\":%.4f,\"p50_ms\":%.4f,\"p95_ms\":%.4f,\"max_ms\":%.4f,\"total_ms\":%.2f,\"first_step_ms\":%.4f,\"mean_ms_after_first\":%.4f,\"top_y\":%.3f,\"mean_y\":%.3f,\"velocity_steps\":%u,\"position_steps\":%u",
		   JPH_VERSION_MAJOR, JPH_VERSION_MINOR, JPH_VERSION_PATCH,
#ifdef JPH_CROSS_PLATFORM_DETERMINISTIC
		   "true",
#else
		   "false",
#endif
		   config.c_str(), scene_name.c_str(), threads, steps, physics_system.GetNumBodies(), num_active,
		   s.mean, s.p50, s.p95, s.max, s.total, step_ms.empty()? 0.0 : step_ms[0], warm.mean, top_y, sum_y / max(1, n_dyn),
		   physics_system.GetPhysicsSettings().mNumVelocitySteps, physics_system.GetPhysicsSettings().mNumPositionSteps);
#ifdef CLOCK_THREAD_CPUTIME_ID
	if (threads == 1)
#else
	if (false)
#endif
	{
		Stats c = Summarize(cpu_ms);
		printf(",\"cpu_mean_ms\":%.4f,\"cpu_p50_ms\":%.4f", c.mean, c.p50);
	}
	if (raycast)
	{
		Stats r = Summarize(ray_ms);
		printf(",\"rays_per_step\":%d,\"ray_mean_ms\":%.4f,\"ray_p95_ms\":%.4f,\"ray_total_ms\":%.2f,\"ray_hits\":%llu,\"ray_fraction_sum\":%.3f",
			   int(rays.size()), r.mean, r.p95, r.total, (unsigned long long)hits, fraction_sum);
	}
	if (fork_system != nullptr)
	{
		printf(",\"fork_at\":%d,\"fork_restored\":%s,\"state_bytes\":%zu,\"save_ms\":%.3f,\"rebuild_ms\":%.3f,\"restore_ms\":%.3f,\"fork_hash\":\"0x%llx\"",
			   fork_at, fork_ok? "true" : "false", fork_state.GetDataSize(), save_ms, rebuild_ms, restore_ms, (unsigned long long)body_hash(*fork_system));
		fork_scene->StopTest(*fork_system);
	}
	printf(",\"hash\":\"0x%llx\"}\n", (unsigned long long)hash);

	scene->StopTest(physics_system);
	UnregisterTypes();
	delete Factory::sInstance;
	Factory::sInstance = nullptr;
	return 0;
}

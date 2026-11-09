// Jolt snapshot continuation: a throwaway spike of the physics research, never
// product code.
//
// Runs three scenes built from Jolt's own performance tests and samples with
// Jolt Physics v5.6.0 built for cross-platform determinism, saves the whole
// state through the state recorder after a number of steps, and checks that a
// world restored from that state continues exactly like the uninterrupted run:
// the same world restored in place, a new world built from the same scene
// description in the same creation order then restored, at the same thread
// count and at another thread count. The library runs on a job system the
// spike implements over a fixed pool of threads. Every body's position and
// rotation is hashed after every step; the first divergent step against the
// uninterrupted run is the measurement, with the state size per body and the
// times to save and restore.
//
// Prints one JSON document on standard output. See README.md.

#include <Jolt/Jolt.h>

#include <Jolt/ConfigurationString.h>
#include <Jolt/Core/Factory.h>
#include <Jolt/Core/TempAllocator.h>
#include <Jolt/Physics/Body/BodyCreationSettings.h>
#include <Jolt/Physics/Collision/BroadPhase/BroadPhaseLayer.h>
#include <Jolt/Physics/Collision/ObjectLayer.h>
#include <Jolt/Physics/Collision/Shape/BoxShape.h>
#include <Jolt/Physics/Collision/Shape/CapsuleShape.h>
#include <Jolt/Physics/Collision/Shape/ConvexHullShape.h>
#include <Jolt/Physics/Collision/Shape/MeshShape.h>
#include <Jolt/Physics/Collision/Shape/SphereShape.h>
#include <Jolt/Physics/Constraints/PointConstraint.h>
#include <Jolt/Physics/PhysicsSettings.h>
#include <Jolt/Physics/PhysicsSystem.h>
#include <Jolt/Physics/StateRecorderImpl.h>
#include <Jolt/RegisterTypes.h>

#include "engine_job_system.h"

#include <algorithm>
#include <chrono>
#include <cinttypes>
#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

using namespace JPH;
using namespace JPH::literals;

namespace
{

constexpr int SCHEMA_VERSION = 1;
constexpr float TIME_STEP = 1.0f / 60.0f;
constexpr int COLLISION_STEPS = 1;
constexpr int STEPS_BEFORE_SNAPSHOT = 100;
constexpr int STEPS_AFTER_SNAPSHOT = 200;
constexpr int THREAD_COUNTS[] = { 1, 4 };
constexpr uint MAX_BODIES = 10240;
constexpr uint MAX_BODY_PAIRS = 65536;
constexpr uint MAX_CONTACT_CONSTRAINTS = 20480;
constexpr size_t TEMP_ALLOCATOR_BYTES = 32 * 1024 * 1024;

constexpr uint64_t FNV_OFFSET_BASIS = 0xcbf29ce484222325ULL;
constexpr uint64_t FNV_PRIME = 0x00000100000001b3ULL;

using Clock = std::chrono::steady_clock;

double MillisecondsSince( Clock::time_point start )
{
	return std::chrono::duration<double, std::milli>( Clock::now() - start ).count();
}

uint64_t HashBytes( uint64_t hash, const void* data, size_t count )
{
	const uint8_t* bytes = static_cast<const uint8_t*>( data );
	for ( size_t i = 0; i < count; ++i )
	{
		hash = ( hash ^ bytes[i] ) * FNV_PRIME;
	}
	return hash;
}

// --- Layers: the performance tests' two layers plus one for the constraint grid ---

namespace Layers
{
constexpr ObjectLayer NON_MOVING = 0;
constexpr ObjectLayer MOVING = 1;
constexpr ObjectLayer GRID = 2; // the constraint grid's bodies collide with the non-moving layer only
constexpr ObjectLayer NUM_LAYERS = 3;
} // namespace Layers

namespace BroadPhaseLayers
{
constexpr BroadPhaseLayer NON_MOVING( 0 );
constexpr BroadPhaseLayer MOVING( 1 );
constexpr uint NUM_LAYERS( 2 );
} // namespace BroadPhaseLayers

class ObjectLayerPairFilterImpl final : public ObjectLayerPairFilter
{
public:
	bool ShouldCollide( ObjectLayer a, ObjectLayer b ) const override
	{
		switch ( a )
		{
			case Layers::NON_MOVING:
				return b != Layers::NON_MOVING;
			case Layers::MOVING:
				return b != Layers::GRID;
			case Layers::GRID:
				return b == Layers::NON_MOVING;
			default:
				return false;
		}
	}
};

class BroadPhaseLayerInterfaceImpl final : public BroadPhaseLayerInterface
{
public:
	uint GetNumBroadPhaseLayers() const override
	{
		return BroadPhaseLayers::NUM_LAYERS;
	}

	BroadPhaseLayer GetBroadPhaseLayer( ObjectLayer layer ) const override
	{
		return layer == Layers::NON_MOVING ? BroadPhaseLayers::NON_MOVING : BroadPhaseLayers::MOVING;
	}

#if defined( JPH_EXTERNAL_PROFILE ) || defined( JPH_PROFILE_ENABLED )
	const char* GetBroadPhaseLayerName( BroadPhaseLayer layer ) const override
	{
		return (BroadPhaseLayer::Type)layer == (BroadPhaseLayer::Type)BroadPhaseLayers::NON_MOVING ? "NON_MOVING" : "MOVING";
	}
#endif
};

class ObjectVsBroadPhaseLayerFilterImpl final : public ObjectVsBroadPhaseLayerFilter
{
public:
	bool ShouldCollide( ObjectLayer layer, BroadPhaseLayer broadPhaseLayer ) const override
	{
		if ( layer == Layers::NON_MOVING )
		{
			return broadPhaseLayer == BroadPhaseLayers::MOVING;
		}
		return true;
	}
};

BroadPhaseLayerInterfaceImpl broadPhaseLayerInterface;
ObjectVsBroadPhaseLayerFilterImpl objectVsBroadPhaseLayerFilter;
ObjectLayerPairFilterImpl objectLayerPairFilter;

// --- Scenes ------------------------------------------------------------------------

typedef void BuildFcn( PhysicsSystem& system );

struct Scene
{
	const char* name;
	const char* origin;
	BuildFcn* build;
};

// Jolt's PerformanceTest/PyramidScene.h: a pyramid of boxes without convex radius
// on a floor, sleeping disabled so that the island stays awake.
void BuildPyramid( PhysicsSystem& system )
{
	BodyInterface& bodies = system.GetBodyInterface();
	bodies.CreateAndAddBody( BodyCreationSettings( new BoxShape( Vec3( 50.0f, 1.0f, 50.0f ), 0.0f ), RVec3( 0.0f, -1.0f, 0.0f ),
												   Quat::sIdentity(), EMotionType::Static, Layers::NON_MOVING ),
							 EActivation::DontActivate );
	const float boxSize = 2.0f;
	const float boxSeparation = 0.5f;
	const float halfBoxSize = 0.5f * boxSize;
	const int pyramidHeight = 15;
	RefConst<Shape> boxShape = new BoxShape( Vec3::sReplicate( halfBoxSize ), 0.0f );
	for ( int i = 0; i < pyramidHeight; ++i )
	{
		for ( int j = i / 2; j < pyramidHeight - ( i + 1 ) / 2; ++j )
		{
			for ( int k = i / 2; k < pyramidHeight - ( i + 1 ) / 2; ++k )
			{
				RVec3 position( Real( -pyramidHeight + boxSize * j + ( i & 1 ? halfBoxSize : 0.0f ) ),
								Real( 1.0f + ( boxSize + boxSeparation ) * i ),
								Real( -pyramidHeight + boxSize * k + ( i & 1 ? halfBoxSize : 0.0f ) ) );
				BodyCreationSettings settings( boxShape, position, Quat::sIdentity(), EMotionType::Dynamic, Layers::MOVING );
				settings.mAllowSleeping = false;
				bodies.CreateAndAddBody( settings, EActivation::Activate );
			}
		}
	}
}

// Jolt's PerformanceTest/ConvexVsMeshScene.h: boxes, spheres, capsules and convex
// hulls dropped on a sloping mesh terrain, with the scene's reduced solver
// iteration counts and discrete motion quality.
void BuildConvexVsMesh( PhysicsSystem& system )
{
	const int n = 100;
	const float cellSize = 3.0f;
	const float maxHeight = 5.0f;
	float center = n * cellSize / 2;

	VertexList vertices;
	vertices.resize( ( n + 1 ) * ( n + 1 ) );
	for ( int x = 0; x <= n; ++x )
	{
		for ( int z = 0; z <= n; ++z )
		{
			float height = Sin( float( x ) * 50.0f / n ) * Cos( float( z ) * 50.0f / n );
			vertices[z * ( n + 1 ) + x] = Float3( cellSize * x, maxHeight * height, cellSize * z );
		}
	}
	IndexedTriangleList indices;
	indices.resize( n * n * 2 );
	IndexedTriangle* next = indices.data();
	for ( int x = 0; x < n; ++x )
	{
		for ( int z = 0; z < n; ++z )
		{
			int start = ( n + 1 ) * z + x;
			next->mIdx[0] = start;
			next->mIdx[1] = start + n + 1;
			next->mIdx[2] = start + 1;
			next++;
			next->mIdx[0] = start + 1;
			next->mIdx[1] = start + n + 1;
			next->mIdx[2] = start + n + 2;
			next++;
		}
	}
	Ref<MeshShapeSettings> meshSettings = new MeshShapeSettings( vertices, indices );
	meshSettings->mMaxTrianglesPerLeaf = 4;
	BodyCreationSettings mesh;
	mesh.mMotionType = EMotionType::Static;
	mesh.mObjectLayer = Layers::NON_MOVING;
	mesh.mPosition = RVec3( Real( -center ), Real( maxHeight ), Real( -center ) );
	mesh.mFriction = 0.5f;
	mesh.mRestitution = 0.6f;
	mesh.SetShapeSettings( meshSettings );

	Array<Ref<Shape>> shapes = {
		new BoxShape( Vec3( 0.5f, 0.75f, 1.0f ) ),
		new SphereShape( 0.5f ),
		new CapsuleShape( 0.75f, 0.5f ),
		ConvexHullShapeSettings( { Vec3( 0, 1, 0 ), Vec3( 1, 0, 0 ), Vec3( -1, 0, 0 ), Vec3( 0, 0, 1 ), Vec3( 0, 0, -1 ) } )
			.Create()
			.Get(),
	};

	PhysicsSettings settings = system.GetPhysicsSettings();
	settings.mNumVelocitySteps = 4;
	settings.mNumPositionSteps = 1;
	system.SetPhysicsSettings( settings );

	BodyInterface& bodies = system.GetBodyInterface();
	bodies.CreateAndAddBody( mesh, EActivation::DontActivate );
	for ( int x = -10; x <= 10; ++x )
	{
		for ( int y = 0; y < (int)shapes.size(); ++y )
		{
			for ( int z = -10; z <= 10; ++z )
			{
				BodyCreationSettings creation;
				creation.mMotionType = EMotionType::Dynamic;
				creation.mMotionQuality = EMotionQuality::Discrete;
				creation.mObjectLayer = Layers::MOVING;
				creation.mPosition = RVec3( 7.5_r * x, 15.0_r + 2.0_r * y, 7.5_r * z );
				creation.mFriction = 0.5f;
				creation.mRestitution = 0.6f;
				creation.SetShape( shapes[y] );
				bodies.CreateAndAddBody( creation, EActivation::Activate );
			}
		}
	}
}

// A grid of spheres linked by point constraints, the construction of Jolt's
// Samples/Tests/Constraints/PointConstraintTest.cpp (bodies created then added,
// a point constraint between neighbours at their shared point) laid out as the
// joint grid of the Box2D and Rapier scenes: a column of static anchors, every
// other body dynamic and never sleeping, the grid's bodies colliding with the
// non-moving layer only.
void BuildPointConstraintGrid( PhysicsSystem& system )
{
	const int n = 30;
	BodyInterface& bodies = system.GetBodyInterface();
	RefConst<Shape> sphere = new SphereShape( 0.4f );
	std::vector<Body*> grid( (size_t)n * n, nullptr );
	for ( int k = 0; k < n; ++k )
	{
		for ( int i = 0; i < n; ++i )
		{
			bool isStatic = i == 0;
			RVec3 position( Real( k ), Real( -i ), 0.0_r );
			BodyCreationSettings settings( sphere, position, Quat::sIdentity(), isStatic ? EMotionType::Static : EMotionType::Dynamic,
										   isStatic ? Layers::NON_MOVING : Layers::GRID );
			settings.mAllowSleeping = false;
			Body* body = bodies.CreateBody( settings );
			bodies.AddBody( body->GetID(), isStatic ? EActivation::DontActivate : EActivation::Activate );
			size_t index = (size_t)k * n + i;
			grid[index] = body;
			if ( i > 0 )
			{
				PointConstraintSettings joint;
				joint.mPoint1 = joint.mPoint2 = position + Vec3( 0.0f, 0.5f, 0.0f );
				system.AddConstraint( joint.Create( *grid[index - 1], *body ) );
			}
			if ( k > 0 )
			{
				PointConstraintSettings joint;
				joint.mPoint1 = joint.mPoint2 = position + Vec3( -0.5f, 0.0f, 0.0f );
				system.AddConstraint( joint.Create( *grid[index - n], *body ) );
			}
		}
	}
}

const Scene SCENES[] = {
	{ "pyramid", "PerformanceTest/PyramidScene.h", BuildPyramid },
	{ "convex_vs_mesh", "PerformanceTest/ConvexVsMeshScene.h", BuildConvexVsMesh },
	{ "point_constraint_grid", "Samples/Tests/Constraints/PointConstraintTest.cpp, laid out as a grid", BuildPointConstraintGrid },
};

// --- One simulation: a physics system on a job system of a given thread count ----

struct Simulation
{
	TempAllocatorImpl temp;
	EngineJobSystem jobs;
	PhysicsSystem system;
	int threads;
	int updateErrors = 0;

	Simulation( const Scene& scene, int threadCount )
		: temp( TEMP_ALLOCATOR_BYTES ), jobs( cMaxPhysicsJobs, cMaxPhysicsBarriers, threadCount ), threads( threadCount )
	{
		system.Init( MAX_BODIES, 0, MAX_BODY_PAIRS, MAX_CONTACT_CONSTRAINTS, broadPhaseLayerInterface, objectVsBroadPhaseLayerFilter,
					 objectLayerPairFilter );
		system.SetGravity( Vec3( 0.0f, -9.81f, 0.0f ) );
		scene.build( system );
		system.OptimizeBroadPhase();
	}

	// Steps once; returns the step's wall time in milliseconds.
	double Step()
	{
		Clock::time_point start = Clock::now();
		EPhysicsUpdateError error = system.Update( TIME_STEP, COLLISION_STEPS, &temp, &jobs );
		double ms = MillisecondsSince( start );
		if ( error != EPhysicsUpdateError::None )
		{
			updateErrors += 1;
		}
		return ms;
	}

	// Every body's position and rotation, sorted by body identifier (index and
	// sequence number, stable across worlds built in the same creation order),
	// hashed field by field.
	uint64_t Hash( int* hashedBodyCount ) const
	{
		BodyIDVector ids;
		system.GetBodies( ids );
		std::sort( ids.begin(), ids.end(),
				   []( const BodyID& a, const BodyID& b ) { return a.GetIndexAndSequenceNumber() < b.GetIndexAndSequenceNumber(); } );
		const BodyInterface& bodies = system.GetBodyInterfaceNoLock();
		uint64_t hash = FNV_OFFSET_BASIS;
		for ( const BodyID& id : ids )
		{
			RVec3 position;
			Quat rotation;
			bodies.GetPositionAndRotation( id, position, rotation );
			float components[7] = { (float)position.GetX(), (float)position.GetY(), (float)position.GetZ(), rotation.GetX(),
									rotation.GetY(), rotation.GetZ(), rotation.GetW() };
			hash = HashBytes( hash, components, sizeof( components ) );
		}
		*hashedBodyCount = (int)ids.size();
		return hash;
	}

	int BodyCount() const
	{
		return (int)system.GetNumBodies();
	}

	std::string SaveState( double* ms ) const
	{
		Clock::time_point start = Clock::now();
		StateRecorderImpl recorder;
		system.SaveState( recorder, EStateRecorderState::All );
		std::string data = recorder.GetData();
		*ms = MillisecondsSince( start );
		return data;
	}

	bool RestoreState( const std::string& data, double* ms )
	{
		Clock::time_point start = Clock::now();
		StateRecorderImpl recorder;
		recorder.WriteBytes( data.data(), data.size() );
		recorder.Rewind();
		bool restored = system.RestoreState( recorder );
		*ms = MillisecondsSince( start );
		return restored;
	}
};

// Steps `steps` times, hashing after every step; returns the total step time.
double StepAndHash( Simulation& simulation, int steps, std::vector<uint64_t>& hashes, int* hashedBodyCount )
{
	double total = 0.0;
	for ( int step = 0; step < steps; ++step )
	{
		total += simulation.Step();
		hashes.push_back( simulation.Hash( hashedBodyCount ) );
	}
	return total;
}

int FirstDivergentStep( const std::vector<uint64_t>& reference, const std::vector<uint64_t>& run )
{
	size_t steps = std::min( reference.size(), run.size() );
	for ( size_t step = 0; step < steps; ++step )
	{
		if ( reference[step] != run[step] )
		{
			return (int)step;
		}
	}
	return -1;
}

// --- The paths -----------------------------------------------------------------------

struct PathResult
{
	const char* path;
	int threads;
	bool restoreSucceeded;
	bool hashAtRestoreMatches;
	int firstDivergentStep;
	uint64_t finalHash;
	int hashedBodyCount;
	int updateErrors;
	double restoreMs;
};

struct SceneResult
{
	const Scene* scene;
	int threads;
	int stepsBefore;
	int stepsAfter;
	int bodyCount;
	int hashedBodyCount;
	bool runsIdenticalBeforeSnapshot;
	uint64_t hashAtSnapshot;
	uint64_t uninterruptedFinalHash;
	double uninterruptedStepMsTotal;
	int uninterruptedUpdateErrors;
	size_t stateBytes;
	double saveMs;
	bool secondWorldStateIdentical;
	std::vector<PathResult> paths;
};

PathResult ContinueAndCompare( Simulation* simulation, const char* path, int threads, bool restoreSucceeded, double restoreMs,
							   uint64_t hashAtSnapshot, const std::vector<uint64_t>& reference, int stepsAfter )
{
	PathResult result = { path, threads, restoreSucceeded, false, -1, 0, 0, 0, restoreMs };
	if ( restoreSucceeded == false )
	{
		return result;
	}
	int hashed = 0;
	result.hashAtRestoreMatches = simulation->Hash( &hashed ) == hashAtSnapshot;
	int errorsBefore = simulation->updateErrors;
	std::vector<uint64_t> hashes;
	hashes.reserve( (size_t)stepsAfter );
	StepAndHash( *simulation, stepsAfter, hashes, &hashed );
	result.firstDivergentStep = FirstDivergentStep( reference, hashes );
	result.finalHash = hashes.empty() ? 0 : hashes.back();
	result.hashedBodyCount = hashed;
	result.updateErrors = simulation->updateErrors - errorsBefore;
	return result;
}

SceneResult RunScene( const Scene& scene, int threads, bool quick )
{
	SceneResult result = {};
	result.scene = &scene;
	result.threads = threads;
	result.stepsBefore = quick ? STEPS_BEFORE_SNAPSHOT / 10 : STEPS_BEFORE_SNAPSHOT;
	result.stepsAfter = quick ? STEPS_AFTER_SNAPSHOT / 10 : STEPS_AFTER_SNAPSHOT;

	// The uninterrupted run: the reference for every path.
	std::vector<uint64_t> before;
	std::vector<uint64_t> reference;
	int hashed = 0;
	{
		Simulation uninterrupted( scene, threads );
		result.uninterruptedStepMsTotal = StepAndHash( uninterrupted, result.stepsBefore, before, &hashed );
		result.uninterruptedStepMsTotal += StepAndHash( uninterrupted, result.stepsAfter, reference, &hashed );
		result.bodyCount = uninterrupted.BodyCount();
		result.hashedBodyCount = hashed;
		result.uninterruptedFinalHash = reference.back();
		result.uninterruptedUpdateErrors = uninterrupted.updateErrors;
	}

	// The snapshotted run: the same steps up to the save, then the paths.
	std::unique_ptr<Simulation> world( new Simulation( scene, threads ) );
	std::vector<uint64_t> hashes;
	StepAndHash( *world, result.stepsBefore, hashes, &hashed );
	result.runsIdenticalBeforeSnapshot = FirstDivergentStep( before, hashes ) == -1;
	result.hashAtSnapshot = hashes.back();
	std::string state = world->SaveState( &result.saveMs );
	result.stateBytes = state.size();

	// A third world saved after the same steps: the bytes must be the same.
	{
		Simulation other( scene, threads );
		std::vector<uint64_t> otherHashes;
		StepAndHash( other, result.stepsBefore, otherHashes, &hashed );
		double ms = 0.0;
		result.secondWorldStateIdentical = other.SaveState( &ms ) == state;
	}

	// Path 1: the world keeps stepping after the save.
	result.paths.push_back( ContinueAndCompare( world.get(), "continued_after_snapshot", threads, true, 0.0, result.hashAtSnapshot,
												reference, result.stepsAfter ) );

	// Path 2: the same world restored in place.
	double restoreMs = 0.0;
	bool restored = world->RestoreState( state, &restoreMs );
	result.paths.push_back( ContinueAndCompare( world.get(), "restored_in_place", threads, restored, restoreMs, result.hashAtSnapshot,
												reference, result.stepsAfter ) );
	world.reset();

	// Path 3: a new world built from the same scene description in the same
	// creation order (so that the body identifiers match), restored, on a job
	// system of the same thread count.
	{
		Simulation rebuilt( scene, threads );
		restored = rebuilt.RestoreState( state, &restoreMs );
		result.paths.push_back( ContinueAndCompare( &rebuilt, "new_world_same_threads", threads, restored, restoreMs,
													result.hashAtSnapshot, reference, result.stepsAfter ) );
	}

	// Path 4: the same, on a job system of the other thread count.
	{
		int otherThreads = threads == 1 ? 4 : 1;
		Simulation rebuilt( scene, otherThreads );
		restored = rebuilt.RestoreState( state, &restoreMs );
		result.paths.push_back( ContinueAndCompare( &rebuilt, "new_world_other_threads", otherThreads, restored, restoreMs,
													result.hashAtSnapshot, reference, result.stepsAfter ) );
	}
	return result;
}

// --- Platform facts ----------------------------------------------------------------

const char* OperatingSystem()
{
#if defined( _WIN32 )
	return "windows";
#elif defined( __APPLE__ )
	return "macos";
#elif defined( __linux__ )
	return "linux";
#else
	return "unknown";
#endif
}

const char* Architecture()
{
#if defined( __x86_64__ ) || defined( _M_X64 )
	return "x86_64";
#elif defined( __aarch64__ ) || defined( _M_ARM64 )
	return "aarch64";
#else
	return "unknown";
#endif
}

void PrintCompiler()
{
#if defined( __clang__ )
	std::printf( "\"clang %s\"", __clang_version__ );
#elif defined( _MSC_FULL_VER )
	std::printf( "\"msvc %d\"", _MSC_FULL_VER );
#elif defined( __GNUC__ )
	std::printf( "\"gcc %s\"", __VERSION__ );
#else
	std::printf( "\"unknown\"" );
#endif
}

void PrintJsonString( const char* text )
{
	std::putchar( '"' );
	for ( const char* c = text; *c != 0; ++c )
	{
		if ( *c == '"' || *c == '\\' )
		{
			std::putchar( '\\' );
		}
		else if ( *c == '\n' )
		{
			std::putchar( '\\' );
			std::putchar( 'n' );
			continue;
		}
		std::putchar( *c );
	}
	std::putchar( '"' );
}

void TraceImpl( const char* format, ... )
{
	va_list list;
	va_start( list, format );
	char buffer[1024];
	std::vsnprintf( buffer, sizeof( buffer ), format, list );
	va_end( list );
	std::fprintf( stderr, "jolt: %s\n", buffer );
}

#ifdef JPH_ENABLE_ASSERTS
bool AssertFailedImpl( const char* expression, const char* message, const char* file, uint line )
{
	std::fprintf( stderr, "jolt assert: %s:%u: (%s) %s\n", file, line, expression, message != nullptr ? message : "" );
	return true;
}
#endif

// --- Self-check: the driver's own logic, not the hypothesis -----------------------

int SelfCheck()
{
	if ( HashBytes( FNV_OFFSET_BASIS, "", 0 ) != 0xcbf29ce484222325ULL || HashBytes( FNV_OFFSET_BASIS, "a", 1 ) != 0xaf63dc4c8601ec8cULL )
	{
		std::fprintf( stderr, "self-check: FNV-1a vectors failed\n" );
		return 1;
	}
	if ( engineJobSystemSelfCheck() != 0 )
	{
		std::fprintf( stderr, "self-check: the job system failed its own checks\n" );
		return 1;
	}
	// Two identical runs hash identically at every step and hash every body; a
	// state restored in place and into a world rebuilt in the same order gives
	// back the hash of the save instant.
	for ( const Scene& scene : SCENES )
	{
		Simulation a( scene, 1 );
		Simulation b( scene, 4 );
		std::vector<uint64_t> hashesA;
		std::vector<uint64_t> hashesB;
		int hashed = 0;
		StepAndHash( a, 10, hashesA, &hashed );
		StepAndHash( b, 10, hashesB, &hashed );
		if ( hashed == 0 || hashed != a.BodyCount() )
		{
			std::fprintf( stderr, "self-check: %s hashed %d bodies of %d\n", scene.name, hashed, a.BodyCount() );
			return 1;
		}
		if ( FirstDivergentStep( hashesA, hashesB ) != -1 )
		{
			std::fprintf( stderr, "self-check: %s differs between one and four threads at step %d (the library's determinism, reported, not a driver fault)\n",
						  scene.name, FirstDivergentStep( hashesA, hashesB ) );
		}
		double ms = 0.0;
		std::string state = a.SaveState( &ms );
		uint64_t atSave = hashesA.back();
		StepAndHash( a, 3, hashesA, &hashed );
		if ( a.RestoreState( state, &ms ) == false || a.Hash( &hashed ) != atSave )
		{
			std::fprintf( stderr, "self-check: %s: the in-place restore did not give back the save instant\n", scene.name );
			return 1;
		}
		Simulation rebuilt( scene, 1 );
		if ( rebuilt.RestoreState( state, &ms ) == false || rebuilt.Hash( &hashed ) != atSave )
		{
			std::fprintf( stderr, "self-check: %s: the rebuilt world did not give back the save instant\n", scene.name );
			return 1;
		}
	}
	std::fprintf( stderr, "self-check passed\n" );
	return 0;
}

} // namespace

// --- Main -----------------------------------------------------------------------------

int main( int argc, char** argv )
{
	const char* cell = "unspecified";
	bool quick = false;
	bool selfCheck = false;
	for ( int i = 1; i < argc; ++i )
	{
		if ( std::strcmp( argv[i], "--quick" ) == 0 )
		{
			quick = true;
		}
		else if ( std::strcmp( argv[i], "--self-check" ) == 0 )
		{
			selfCheck = true;
		}
		else
		{
			cell = argv[i];
		}
	}

	RegisterDefaultAllocator();
	Trace = TraceImpl;
	JPH_IF_ENABLE_ASSERTS( AssertFailed = AssertFailedImpl; )
	Factory::sInstance = new Factory();
	RegisterTypes();

	int exitCode = 0;
	if ( selfCheck )
	{
		exitCode = SelfCheck();
	}
	else
	{
		std::vector<SceneResult> results;
		for ( const Scene& scene : SCENES )
		{
			for ( int threads : THREAD_COUNTS )
			{
				std::fprintf( stderr, "%s at %d threads ...", scene.name, threads );
				results.push_back( RunScene( scene, threads, quick ) );
				const SceneResult& result = results.back();
				std::fprintf( stderr, " state %zu bytes", result.stateBytes );
				for ( const PathResult& path : result.paths )
				{
					std::fprintf( stderr, ", %s diverges at %d", path.path, path.firstDivergentStep );
				}
				std::fprintf( stderr, "\n" );
			}
		}

		std::printf( "{\"schema_version\":%d,\"spike\":\"jolt-snapshot-continuation\",\"cell\":", SCHEMA_VERSION );
		PrintJsonString( cell );
		std::printf( ",\"quick\":%s,\"jolt\":{\"version\":\"5.6.0\",\"git_tag\":\"v5.6.0\",\"license\":\"MIT\","
					 "\"cross_platform_deterministic\":%s,\"double_precision\":%s,\"configuration\":",
					 quick ? "true" : "false",
#ifdef JPH_CROSS_PLATFORM_DETERMINISTIC
					 "true",
#else
					 "false",
#endif
#ifdef JPH_DOUBLE_PRECISION
					 "true"
#else
					 "false"
#endif
		);
		PrintJsonString( GetConfigurationString() );
		std::printf( "},\"job_system\":{\"kind\":\"the spike's fixed pool over Jolt's barrier base, the waiting thread runs the barrier's jobs\","
					 "\"max_jobs\":%d,\"max_barriers\":%d},",
					 cMaxPhysicsJobs, cMaxPhysicsBarriers );
		std::printf( "\"settings\":{\"time_step\":%.9g,\"collision_steps\":%d,\"steps_before_snapshot\":%d,\"steps_after_snapshot\":%d,"
					 "\"thread_counts\":[1,4],\"saved_state\":\"all (global, bodies, contacts, constraints)\"},",
					 (double)TIME_STEP, COLLISION_STEPS, STEPS_BEFORE_SNAPSHOT, STEPS_AFTER_SNAPSHOT );
		std::printf( "\"platform\":{\"os\":\"%s\",\"arch\":\"%s\",\"hardware_threads\":%u,\"compiler\":", OperatingSystem(),
					 Architecture(), std::thread::hardware_concurrency() );
		PrintCompiler();
		std::printf( "},\"runs\":[" );
		for ( size_t i = 0; i < results.size(); ++i )
		{
			const SceneResult& r = results[i];
			std::printf( "%s{\"scene\":\"%s\",\"origin\":", i == 0 ? "" : ",", r.scene->name );
			PrintJsonString( r.scene->origin );
			std::printf( ",\"threads\":%d,\"steps_before_snapshot\":%d,\"steps_after_snapshot\":%d,\"body_count\":%d,"
						 "\"hashed_body_count\":%d,\"runs_identical_before_snapshot\":%s,\"hash_at_snapshot\":\"0x%016" PRIx64 "\","
						 "\"uninterrupted_final_hash\":\"0x%016" PRIx64 "\",\"uninterrupted_step_ms_total\":%.3f,"
						 "\"uninterrupted_update_errors\":%d,\"state_bytes\":%zu,\"bytes_per_body\":%.1f,\"save_ms\":%.3f,"
						 "\"second_world_state_identical\":%s,\"paths\":[",
						 r.threads, r.stepsBefore, r.stepsAfter, r.bodyCount, r.hashedBodyCount,
						 r.runsIdenticalBeforeSnapshot ? "true" : "false", r.hashAtSnapshot, r.uninterruptedFinalHash,
						 r.uninterruptedStepMsTotal, r.uninterruptedUpdateErrors, r.stateBytes,
						 r.bodyCount == 0 ? 0.0 : (double)r.stateBytes / (double)r.bodyCount, r.saveMs,
						 r.secondWorldStateIdentical ? "true" : "false" );
			for ( size_t p = 0; p < r.paths.size(); ++p )
			{
				const PathResult& path = r.paths[p];
				std::printf( "%s{\"path\":\"%s\",\"threads\":%d,\"restore_succeeded\":%s,\"hash_at_restore_matches\":%s,"
							 "\"first_divergent_step\":%d,\"final_hash\":\"0x%016" PRIx64 "\",\"hashed_body_count\":%d,"
							 "\"update_errors\":%d,\"restore_ms\":%.3f}",
							 p == 0 ? "" : ",", path.path, path.threads, path.restoreSucceeded ? "true" : "false",
							 path.hashAtRestoreMatches ? "true" : "false", path.firstDivergentStep, path.finalHash,
							 path.hashedBodyCount, path.updateErrors, path.restoreMs );
			}
			std::printf( "]}" );
		}
		std::printf( "]}\n" );
	}

	UnregisterTypes();
	delete Factory::sInstance;
	Factory::sInstance = nullptr;
	return exitCode;
}

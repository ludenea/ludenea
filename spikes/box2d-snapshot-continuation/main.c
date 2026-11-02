// Box2D snapshot continuation: a throwaway spike of the physics research, never
// product code.
//
// Runs the author's benchmark scenes with Box2D v3.2.0, takes a snapshot of the
// world after a number of steps, and checks that the world restored from that
// snapshot continues exactly like the uninterrupted run: restored in place,
// rebuilt as a new world with the same worker count, and rebuilt as a new world
// with another worker count. Every body transform is hashed after every step;
// the first divergent step against the uninterrupted run is the measurement.
// The size of the snapshot and the time to take and restore it are recorded.
//
// Prints one JSON document on standard output. See README.md.

#include "benchmarks.h"

#include "box2d/base.h"
#include "box2d/box2d.h"
#include "box2d/id.h"
#include "box2d/math_functions.h"
#include "box2d/types.h"

#include <inttypes.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define ARRAY_COUNT( a ) ( (int)( sizeof( a ) / sizeof( ( a )[0] ) ) )

static const int SCHEMA_VERSION = 1;
static const float TIME_STEP = 1.0f / 60.0f;
static const int SUBSTEP_COUNT = 4;
static const int STEPS_BEFORE_SNAPSHOT = 100;
static const int STEPS_AFTER_SNAPSHOT = 200;
static const int WORKER_COUNTS[] = { 1, 4 };

static const uint64_t FNV_OFFSET_BASIS = 0xcbf29ce484222325ULL;
static const uint64_t FNV_PRIME = 0x00000100000001b3ULL;

typedef b2Capacity CapacityFcn( void );
typedef void CreateFcn( b2WorldId worldId );

// Scenes from Box2D's shared/benchmarks.c that need no per-step driver, so that
// a world rebuilt from a snapshot can be stepped without the scene's own state.
typedef struct Scene
{
	const char* name;
	CapacityFcn* capacity;
	CreateFcn* create;
} Scene;

static const Scene SCENES[] = {
	{ "joint_grid", GetJointGridCapacity, CreateJointGrid },
	{ "large_pyramid", GetLargePyramidCapacity, CreateLargePyramid },
	{ "many_pyramids", GetManyPyramidsCapacity, CreateManyPyramids },
	{ "smash", GetSmashCapacity, CreateSmash },
	{ "tumbler", GetTumblerCapacity, CreateTumbler },
};

// --- State hash: every body transform, in creation order --------------------

static uint64_t HashBytes( uint64_t hash, const void* data, size_t count )
{
	const uint8_t* bytes = data;
	for ( size_t i = 0; i < count; ++i )
	{
		hash = ( hash ^ bytes[i] ) * FNV_PRIME;
	}
	return hash;
}

typedef struct BodyList
{
	uint64_t* ids;
	int count;
	int capacity;
} BodyList;

static bool CollectBody( b2ShapeId shapeId, void* context )
{
	BodyList* list = context;
	if ( list->count == list->capacity )
	{
		list->capacity = list->capacity == 0 ? 1024 : 2 * list->capacity;
		list->ids = realloc( list->ids, (size_t)list->capacity * sizeof( uint64_t ) );
		if ( list->ids == NULL )
		{
			fprintf( stderr, "out of memory\n" );
			exit( 1 );
		}
	}
	list->ids[list->count] = b2StoreBodyId( b2Shape_GetBody( shapeId ) );
	list->count += 1;
	return true;
}

static int CompareIds( const void* a, const void* b )
{
	uint64_t x = *(const uint64_t*)a;
	uint64_t y = *(const uint64_t*)b;
	return x < y ? -1 : ( x > y ? 1 : 0 );
}

// Enumerates every shape through a broadphase query covering the whole scene,
// reduces the shapes to their bodies, sorts the bodies by their stored id (the
// creation slot comes first in it, and a snapshot preserves slots and
// generations, so two worlds compare body by body), and hashes position and
// rotation field by field.
static uint64_t HashBodyTransforms( b2WorldId worldId, BodyList* list, int* hashedBodyCount )
{
	list->count = 0;
	b2QueryFilter filter = b2DefaultQueryFilter();
	filter.categoryBits = UINT64_MAX;
	filter.maskBits = UINT64_MAX;
	b2AABB everything = { { -100000.0f, -100000.0f }, { 100000.0f, 100000.0f } };
	b2World_OverlapAABB( worldId, b2Pos_zero, everything, filter, CollectBody, list );

	qsort( list->ids, (size_t)list->count, sizeof( uint64_t ), CompareIds );

	uint64_t hash = FNV_OFFSET_BASIS;
	int bodyCount = 0;
	for ( int i = 0; i < list->count; ++i )
	{
		if ( i > 0 && list->ids[i] == list->ids[i - 1] )
		{
			continue;
		}
		b2WorldTransform transform = b2Body_GetTransform( b2LoadBodyId( list->ids[i] ) );
		hash = HashBytes( hash, &transform.p.x, sizeof( transform.p.x ) );
		hash = HashBytes( hash, &transform.p.y, sizeof( transform.p.y ) );
		hash = HashBytes( hash, &transform.q.c, sizeof( transform.q.c ) );
		hash = HashBytes( hash, &transform.q.s, sizeof( transform.q.s ) );
		bodyCount += 1;
	}
	*hashedBodyCount = bodyCount;
	return hash;
}

// --- Worlds and steps --------------------------------------------------------

static b2WorldId CreateSceneWorld( const Scene* scene, int workerCount )
{
	b2WorldDef def = b2DefaultWorldDef();
	def.workerCount = workerCount;
	def.capacity = scene->capacity();
	b2WorldId worldId = b2CreateWorld( &def );
	scene->create( worldId );
	return worldId;
}

// Steps the world `steps` times and hashes after every step into `hashes`.
static void StepAndHash( b2WorldId worldId, int steps, uint64_t* hashes, BodyList* list, int* hashedBodyCount )
{
	for ( int step = 0; step < steps; ++step )
	{
		b2World_Step( worldId, TIME_STEP, SUBSTEP_COUNT );
		hashes[step] = HashBodyTransforms( worldId, list, hashedBodyCount );
	}
}

static int FirstDivergentStep( const uint64_t* reference, const uint64_t* run, int steps )
{
	for ( int step = 0; step < steps; ++step )
	{
		if ( reference[step] != run[step] )
		{
			return step;
		}
	}
	return -1;
}

typedef struct Snapshot
{
	uint8_t* image;
	int size;
	float takeMs;
} Snapshot;

static Snapshot TakeSnapshot( b2WorldId worldId )
{
	Snapshot snapshot = { 0 };
	uint64_t ticks = b2GetTicks();
	snapshot.size = b2World_GetSnapshot( worldId, NULL, 0 );
	if ( snapshot.size <= 0 )
	{
		fprintf( stderr, "snapshot size query failed\n" );
		exit( 1 );
	}
	snapshot.image = malloc( (size_t)snapshot.size );
	if ( snapshot.image == NULL )
	{
		fprintf( stderr, "out of memory\n" );
		exit( 1 );
	}
	int written = b2World_GetSnapshot( worldId, snapshot.image, snapshot.size );
	snapshot.takeMs = b2GetMilliseconds( ticks );
	if ( written != snapshot.size )
	{
		fprintf( stderr, "snapshot wrote %d bytes of %d\n", written, snapshot.size );
		exit( 1 );
	}
	return snapshot;
}

// --- One continuation path -----------------------------------------------------

typedef struct PathResult
{
	const char* path;
	int workers;			// the worker count of the world that continues
	bool restoreSucceeded;	// the restore call returned true, or the new world is valid
	bool hashAtRestoreMatches; // the hash right after the restore equals the hash at the snapshot
	int firstDivergentStep;	// in steps after the snapshot; -1 when identical throughout
	uint64_t finalHash;
	int hashedBodyCount;
	float restoreMs;
} PathResult;

typedef struct SceneResult
{
	const Scene* scene;
	int workers;
	int bodyCount;
	int hashedBodyCount;
	uint64_t hashAtSnapshot;
	uint64_t uninterruptedFinalHash;
	int snapshotBytes;
	float snapshotMs;
	PathResult paths[4];
	int pathCount;
} SceneResult;

static void ContinueAndCompare( b2WorldId worldId, const char* path, int workers, bool restoreSucceeded, float restoreMs,
								const SceneResult* scene, const uint64_t* reference, uint64_t* hashes, BodyList* list,
								PathResult* result )
{
	memset( result, 0, sizeof( *result ) );
	result->path = path;
	result->workers = workers;
	result->restoreSucceeded = restoreSucceeded;
	result->restoreMs = restoreMs;
	result->firstDivergentStep = -1;
	if ( restoreSucceeded == false )
	{
		return;
	}
	int hashedBodyCount = 0;
	result->hashAtRestoreMatches = HashBodyTransforms( worldId, list, &hashedBodyCount ) == scene->hashAtSnapshot;
	StepAndHash( worldId, STEPS_AFTER_SNAPSHOT, hashes, list, &hashedBodyCount );
	result->firstDivergentStep = FirstDivergentStep( reference, hashes, STEPS_AFTER_SNAPSHOT );
	result->finalHash = hashes[STEPS_AFTER_SNAPSHOT - 1];
	result->hashedBodyCount = hashedBodyCount;
}

static void RunScene( const Scene* scene, int workers, SceneResult* result, BodyList* list, uint64_t* before,
					  uint64_t* reference, uint64_t* hashes )
{
	memset( result, 0, sizeof( *result ) );
	result->scene = scene;
	result->workers = workers;

	// The uninterrupted run: the reference for every path.
	b2WorldId uninterrupted = CreateSceneWorld( scene, workers );
	int hashedBodyCount = 0;
	StepAndHash( uninterrupted, STEPS_BEFORE_SNAPSHOT, before, list, &hashedBodyCount );
	StepAndHash( uninterrupted, STEPS_AFTER_SNAPSHOT, reference, list, &hashedBodyCount );
	result->bodyCount = b2World_GetCounters( uninterrupted ).bodyCount;
	result->hashedBodyCount = hashedBodyCount;
	result->uninterruptedFinalHash = reference[STEPS_AFTER_SNAPSHOT - 1];
	b2DestroyWorld( uninterrupted );

	// The snapshotted run: same steps up to the snapshot, then the paths.
	b2WorldId worldId = CreateSceneWorld( scene, workers );
	StepAndHash( worldId, STEPS_BEFORE_SNAPSHOT, hashes, list, &hashedBodyCount );
	if ( FirstDivergentStep( before, hashes, STEPS_BEFORE_SNAPSHOT ) != -1 )
	{
		fprintf( stderr, "%s at %d workers: two identical runs diverged before the snapshot\n", scene->name, workers );
		exit( 1 );
	}
	result->hashAtSnapshot = hashes[STEPS_BEFORE_SNAPSHOT - 1];
	Snapshot snapshot = TakeSnapshot( worldId );
	result->snapshotBytes = snapshot.size;
	result->snapshotMs = snapshot.takeMs;

	// Path 1: the world keeps stepping after the snapshot was taken (taking a
	// snapshot must not perturb the world).
	ContinueAndCompare( worldId, "continued_after_snapshot", workers, true, 0.0f, result, reference, hashes, list,
						&result->paths[result->pathCount++] );

	// Path 2: the same world restored in place from the snapshot, then stepped.
	uint64_t ticks = b2GetTicks();
	bool restored = b2World_RestoreFromSnapshot( worldId, snapshot.image, snapshot.size );
	float restoreMs = b2GetMilliseconds( ticks );
	ContinueAndCompare( worldId, "restored_in_place", workers, restored, restoreMs, result, reference, hashes, list,
						&result->paths[result->pathCount++] );
	b2DestroyWorld( worldId );

	// Path 3: a new world built from the snapshot with the same worker count.
	ticks = b2GetTicks();
	b2WorldId rebuilt = b2CreateWorldFromSnapshot( snapshot.image, snapshot.size, workers );
	restoreMs = b2GetMilliseconds( ticks );
	ContinueAndCompare( rebuilt, "new_world_same_workers", workers, b2World_IsValid( rebuilt ), restoreMs, result,
						reference, hashes, list, &result->paths[result->pathCount++] );
	if ( b2World_IsValid( rebuilt ) )
	{
		b2DestroyWorld( rebuilt );
	}

	// Path 4: a new world built from the snapshot with the other worker count.
	int otherWorkers = workers == 1 ? 4 : 1;
	ticks = b2GetTicks();
	b2WorldId other = b2CreateWorldFromSnapshot( snapshot.image, snapshot.size, otherWorkers );
	restoreMs = b2GetMilliseconds( ticks );
	ContinueAndCompare( other, "new_world_other_workers", otherWorkers, b2World_IsValid( other ), restoreMs, result,
						reference, hashes, list, &result->paths[result->pathCount++] );
	if ( b2World_IsValid( other ) )
	{
		b2DestroyWorld( other );
	}

	free( snapshot.image );
}

// --- Platform facts ----------------------------------------------------------

static const char* OperatingSystem( void )
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

static const char* Architecture( void )
{
#if defined( __x86_64__ ) || defined( _M_X64 )
	return "x86_64";
#elif defined( __aarch64__ ) || defined( _M_ARM64 )
	return "aarch64";
#else
	return "unknown";
#endif
}

static void PrintCompiler( void )
{
#if defined( __clang__ )
	printf( "\"clang %s\"", __clang_version__ );
#elif defined( _MSC_FULL_VER )
	printf( "\"msvc %d\"", _MSC_FULL_VER );
#elif defined( __GNUC__ )
	printf( "\"gcc %s\"", __VERSION__ );
#else
	printf( "\"unknown\"" );
#endif
}

static void PrintJsonString( const char* text )
{
	putchar( '"' );
	for ( const char* c = text; *c != 0; ++c )
	{
		if ( *c == '"' || *c == '\\' )
		{
			putchar( '\\' );
		}
		putchar( *c );
	}
	putchar( '"' );
}

// --- Self-check: the driver's own logic, not the hypothesis -----------------

static int SelfCheck( void )
{
	if ( HashBytes( FNV_OFFSET_BASIS, "", 0 ) != 0xcbf29ce484222325ULL ||
		 HashBytes( FNV_OFFSET_BASIS, "a", 1 ) != 0xaf63dc4c8601ec8cULL )
	{
		fprintf( stderr, "self-check: FNV-1a vectors failed\n" );
		return 1;
	}

	// Two identical serial runs hash identically, and a snapshot restored in place
	// at once gives back the hash of the snapshot instant.
	BodyList list = { 0 };
	uint64_t a[20];
	uint64_t b[20];
	int hashed = 0;
	b2WorldId worldA = CreateSceneWorld( &SCENES[1], 1 );
	b2WorldId worldB = CreateSceneWorld( &SCENES[1], 1 );
	StepAndHash( worldA, 20, a, &list, &hashed );
	StepAndHash( worldB, 20, b, &list, &hashed );
	if ( hashed == 0 || FirstDivergentStep( a, b, 20 ) != -1 )
	{
		fprintf( stderr, "self-check: two serial runs differ or no body was hashed\n" );
		return 1;
	}
	Snapshot snapshot = TakeSnapshot( worldA );
	StepAndHash( worldA, 5, b, &list, &hashed );
	if ( b2World_RestoreFromSnapshot( worldA, snapshot.image, snapshot.size ) == false ||
		 HashBodyTransforms( worldA, &list, &hashed ) != a[19] )
	{
		fprintf( stderr, "self-check: the in-place restore did not give back the snapshot instant\n" );
		return 1;
	}
	b2WorldId rebuilt = b2CreateWorldFromSnapshot( snapshot.image, snapshot.size, 1 );
	if ( b2World_IsValid( rebuilt ) == false || HashBodyTransforms( rebuilt, &list, &hashed ) != a[19] )
	{
		fprintf( stderr, "self-check: the world rebuilt from the snapshot differs from the snapshot instant\n" );
		return 1;
	}
	b2DestroyWorld( rebuilt );
	b2DestroyWorld( worldA );
	b2DestroyWorld( worldB );
	free( snapshot.image );
	free( list.ids );
	fprintf( stderr, "self-check passed\n" );
	return 0;
}

// --- Main ------------------------------------------------------------------

int main( int argc, char** argv )
{
	const char* cell = "unspecified";
	bool selfCheck = false;
	for ( int i = 1; i < argc; ++i )
	{
		if ( strcmp( argv[i], "--self-check" ) == 0 )
		{
			selfCheck = true;
		}
		else
		{
			cell = argv[i];
		}
	}
	if ( selfCheck )
	{
		return SelfCheck();
	}

	int sceneCount = ARRAY_COUNT( SCENES );
	int workerCountCount = ARRAY_COUNT( WORKER_COUNTS );
	int runCount = sceneCount * workerCountCount;
	SceneResult* results = calloc( (size_t)runCount, sizeof( SceneResult ) );
	uint64_t* before = calloc( (size_t)STEPS_BEFORE_SNAPSHOT, sizeof( uint64_t ) );
	uint64_t* reference = calloc( (size_t)STEPS_AFTER_SNAPSHOT, sizeof( uint64_t ) );
	uint64_t* hashes = calloc( (size_t)STEPS_AFTER_SNAPSHOT, sizeof( uint64_t ) );
	BodyList list = { 0 };
	if ( results == NULL || before == NULL || reference == NULL || hashes == NULL )
	{
		fprintf( stderr, "out of memory\n" );
		return 1;
	}

	int runIndex = 0;
	for ( int s = 0; s < sceneCount; ++s )
	{
		for ( int w = 0; w < workerCountCount; ++w )
		{
			SceneResult* result = &results[runIndex++];
			fprintf( stderr, "%s at %d workers ...", SCENES[s].name, WORKER_COUNTS[w] );
			RunScene( &SCENES[s], WORKER_COUNTS[w], result, &list, before, reference, hashes );
			fprintf( stderr, " snapshot %d bytes", result->snapshotBytes );
			for ( int p = 0; p < result->pathCount; ++p )
			{
				fprintf( stderr, ", %s diverges at %d", result->paths[p].path, result->paths[p].firstDivergentStep );
			}
			fprintf( stderr, "\n" );
		}
	}

	b2Version version = b2GetVersion();
	printf( "{\"schema_version\":%d,\"spike\":\"box2d-snapshot-continuation\",\"cell\":", SCHEMA_VERSION );
	PrintJsonString( cell );
	printf( ",\"box2d\":{\"version\":\"%d.%d.%d\",\"git_tag\":\"v3.2.0\",\"license\":\"MIT\",\"avx2_available\":%s,"
			"\"max_workers\":%d},",
			version.major, version.minor, version.revision, b2IsAVX2Available() ? "true" : "false", B2_MAX_WORKERS );
	printf( "\"settings\":{\"time_step\":%.9g,\"substeps\":%d,\"steps_before_snapshot\":%d,\"steps_after_snapshot\":%d},",
			(double)TIME_STEP, SUBSTEP_COUNT, STEPS_BEFORE_SNAPSHOT, STEPS_AFTER_SNAPSHOT );
	printf( "\"platform\":{\"os\":\"%s\",\"arch\":\"%s\",\"compiler\":", OperatingSystem(), Architecture() );
	PrintCompiler();
	printf( "},\"runs\":[" );
	for ( int i = 0; i < runCount; ++i )
	{
		const SceneResult* r = &results[i];
		printf( "%s{\"scene\":\"%s\",\"workers\":%d,\"body_count\":%d,\"hashed_body_count\":%d,"
				"\"hash_at_snapshot\":\"0x%016" PRIx64 "\",\"uninterrupted_final_hash\":\"0x%016" PRIx64 "\","
				"\"snapshot_bytes\":%d,\"snapshot_ms\":%.3f,\"paths\":[",
				i == 0 ? "" : ",", r->scene->name, r->workers, r->bodyCount, r->hashedBodyCount, r->hashAtSnapshot,
				r->uninterruptedFinalHash, r->snapshotBytes, (double)r->snapshotMs );
		for ( int p = 0; p < r->pathCount; ++p )
		{
			const PathResult* path = &r->paths[p];
			printf( "%s{\"path\":\"%s\",\"workers\":%d,\"restore_succeeded\":%s,\"hash_at_restore_matches\":%s,"
					"\"first_divergent_step\":%d,\"final_hash\":\"0x%016" PRIx64 "\",\"hashed_body_count\":%d,"
					"\"restore_ms\":%.3f}",
					p == 0 ? "" : ",", path->path, path->workers, path->restoreSucceeded ? "true" : "false",
					path->hashAtRestoreMatches ? "true" : "false", path->firstDivergentStep, path->finalHash,
					path->hashedBodyCount, (double)path->restoreMs );
		}
		printf( "]}" );
	}
	printf( "]}\n" );

	free( results );
	free( before );
	free( reference );
	free( hashes );
	free( list.ids );
	return 0;
}

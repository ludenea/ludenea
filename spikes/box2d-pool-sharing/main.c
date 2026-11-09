// Box2D pool sharing: a throwaway spike of the physics research, never product code.
//
// Drives Box2D v3.2.0 through its task interface from a scheduler shaped like the
// engine's frame pool (a fixed team claiming the library's tasks under a central
// counter, a bounded spin then a park, a finish that helps the library's pending
// tasks only), against the library's native scheduler at the same worker count,
// on the author's benchmark scenes, and hashes every body transform after every
// step. Identical hashes say the adapter changes no result; the step times say
// what sharing the engine's workers with the library costs. A dedicated-subset
// variant gives the library half the team while the other half runs a synthetic
// parallel pass, so that a proposal can choose per machine class.
//
// Prints one JSON document on standard output. See README.md.

#include "benchmarks.h"
#include "engine_pool.h"
#include "utils.h"

#include "box2d/base.h"
#include "box2d/box2d.h"
#include "box2d/constants.h"
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
static const int MAX_STEPS = 1000;
static const int SPIN_BOUND_MICROSECONDS = 50;
static const int BASE_WORKER_COUNTS[] = { 1, 2, 4, 8 };
#define MAX_WORKER_COUNTS 8
// Every configuration runs this many times and the fastest run is kept, since
// a single multithreaded run's time varies by tens of percent on a desktop
// machine; the hashes of every round must agree.
#define ROUNDS 3

static const uint64_t FNV_OFFSET_BASIS = 0xcbf29ce484222325ULL;
static const uint64_t FNV_PRIME = 0x00000100000001b3ULL;

typedef b2Capacity CapacityFcn( void );
typedef void CreateFcn( b2WorldId worldId );
typedef float StepFcn( b2WorldId worldId, int stepCount );

// The scenes come from Box2D's shared/benchmarks.c, with the step counts of
// Box2D's benchmark/main.c, and the final hash the worker sweep spike measured
// for each of them on the five cells (identical across schedulers, worker
// counts and SIMD widths), so that this spike compares with that measurement.
typedef struct Scene
{
	const char* name;
	CapacityFcn* capacity;
	CreateFcn* create;
	StepFcn* step;
	int steps;
	uint64_t workerSweepFinalHash;
} Scene;

static const Scene SCENES[] = {
	{ "joint_grid", GetJointGridCapacity, CreateJointGrid, NULL, 500, 0x9c1e03dad6409a09ULL },
	{ "large_pyramid", GetLargePyramidCapacity, CreateLargePyramid, NULL, 500, 0xc3af4b9fd4b96e77ULL },
	{ "many_pyramids", GetManyPyramidsCapacity, CreateManyPyramids, NULL, 200, 0xb1c91fd6de0df6d9ULL },
	{ "rain", GetRainCapacity, CreateRain, StepRain, 1000, 0x07e3c76d31f4bc47ULL },
	{ "smash", GetSmashCapacity, CreateSmash, NULL, 300, 0x752022fc74c9c592ULL },
	{ "spinner", GetSpinnerCapacity, CreateSpinner, StepSpinner, 500, 0xc43a20d0bfd4107dULL },
	{ "tumbler", GetTumblerCapacity, CreateTumbler, NULL, 750, 0x67b938db701c26ceULL },
};

typedef enum SchedulerKind
{
	SCHEDULER_NATIVE,		 // Box2D's own task system (no callbacks); serial at one worker
	SCHEDULER_ENGINE_WHOLE,	 // the engine-shaped pool, the whole team claiming the library's tasks
	SCHEDULER_ENGINE_SUBSET, // the engine-shaped pool, half the team for the library, the other half on a synthetic pass
} SchedulerKind;

static const char* SchedulerName( SchedulerKind kind )
{
	switch ( kind )
	{
		case SCHEDULER_NATIVE:
			return "native";
		case SCHEDULER_ENGINE_WHOLE:
			return "engine_whole_team";
		default:
			return "engine_dedicated_subset";
	}
}

// --- Worlds -------------------------------------------------------------------

static int LibraryWorkers( SchedulerKind kind, int team )
{
	return kind == SCHEDULER_ENGINE_SUBSET ? team / 2 : team;
}

static b2WorldId CreateWorld( SchedulerKind kind, int team, EnginePool** pool, const Scene* scene )
{
	b2WorldDef def = b2DefaultWorldDef();
	*pool = NULL;
	if ( kind == SCHEDULER_NATIVE )
	{
		def.workerCount = team;
	}
	else
	{
		int libraryWorkers = LibraryWorkers( kind, team );
		*pool = enginePoolCreate( team, libraryWorkers, SPIN_BOUND_MICROSECONDS );
		// Box2D reads the worker count once; it is the number of workers that may
		// run its tasks, the stepping thread included, as the pool counts them.
		def.workerCount = libraryWorkers;
		def.enqueueTask = enginePoolEnqueue;
		def.finishTask = enginePoolFinish;
		def.userTaskContext = *pool;
	}
	if ( scene != NULL && scene->capacity != NULL )
	{
		def.capacity = scene->capacity();
	}
	return b2CreateWorld( &def );
}

static void DestroyWorld( b2WorldId worldId, EnginePool* pool )
{
	b2DestroyWorld( worldId );
	enginePoolDestroy( pool );
}

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

// The worker sweep's hash: every shape through a broadphase query covering the
// scene, reduced to bodies, sorted by stored id, position and rotation hashed
// field by field. Same construction, so that the hashes compare.
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

// --- One run: a scene, a scheduler, a team -------------------------------------

typedef struct RunResult
{
	const Scene* scene;
	SchedulerKind scheduler;
	int team;
	int libraryWorkers;
	int steps;
	int bodyCount;
	int hashedBodyCount;
	int taskCount;
	EnginePoolCounters pool;
	uint64_t finalHash;
	uint64_t chainHash;
	float stepMsTotal;
	float stepMsMedian;
	float stepMsMax;
	int firstDivergentStep;
	bool matchesWorkerSweep;		  // meaningful at the full step count only
	float costOverNative;			  // this run's total over the native run's total at the same team; 0 for native
	float costOverNativeSameLibrary; // over the native run's total at this run's library worker count; 0 for native
	int rounds;						  // rounds run; the fields above describe the fastest
	float roundsMsTotal[ROUNDS];
	bool roundsIdentical; // every round gave the same final hash
	uint64_t stepHashes[1000];
} RunResult;

static int CompareFloats( const void* a, const void* b )
{
	float x = *(const float*)a;
	float y = *(const float*)b;
	return x < y ? -1 : ( x > y ? 1 : 0 );
}

static void RunScene( const Scene* scene, SchedulerKind kind, int team, int steps, RunResult* result, BodyList* list,
					  float* stepTimes )
{
	EnginePool* pool = NULL;
	b2WorldId worldId = CreateWorld( kind, team, &pool, scene );
	scene->create( worldId );

	memset( result, 0, sizeof( *result ) );
	result->scene = scene;
	result->scheduler = kind;
	result->team = team;
	result->libraryWorkers = LibraryWorkers( kind, team );
	result->steps = steps;
	result->firstDivergentStep = -1;

	uint64_t chain = FNV_OFFSET_BASIS;
	float total = 0.0f;
	float max = 0.0f;
	int hashedBodyCount = 0;
	for ( int step = 0; step < steps; ++step )
	{
		if ( scene->step != NULL )
		{
			scene->step( worldId, step );
		}
		if ( pool != NULL )
		{
			enginePoolBeginStep( pool );
		}
		uint64_t ticks = b2GetTicks();
		b2World_Step( worldId, TIME_STEP, SUBSTEP_COUNT );
		float ms = b2GetMilliseconds( ticks );
		if ( pool != NULL )
		{
			enginePoolEndStep( pool );
		}
		stepTimes[step] = ms;
		total += ms;
		max = ms > max ? ms : max;

		uint64_t hash = HashBodyTransforms( worldId, list, &hashedBodyCount );
		result->stepHashes[step] = hash;
		chain = HashBytes( chain, &hash, sizeof( hash ) );
	}

	b2Counters counters = b2World_GetCounters( worldId );
	result->bodyCount = counters.bodyCount;
	result->hashedBodyCount = hashedBodyCount;
	result->taskCount = counters.taskCount;
	if ( pool != NULL )
	{
		result->pool = enginePoolCounters( pool );
	}
	result->finalHash = result->stepHashes[steps - 1];
	result->chainHash = chain;
	result->stepMsTotal = total;
	result->stepMsMax = max;
	qsort( stepTimes, (size_t)steps, sizeof( float ), CompareFloats );
	result->stepMsMedian = stepTimes[steps / 2];
	result->matchesWorkerSweep = steps == scene->steps && result->finalHash == scene->workerSweepFinalHash;

	DestroyWorld( worldId, pool );
}

static int FirstDivergentStep( const RunResult* reference, const RunResult* run )
{
	int steps = reference->steps < run->steps ? reference->steps : run->steps;
	for ( int step = 0; step < steps; ++step )
	{
		if ( reference->stepHashes[step] != run->stepHashes[step] )
		{
			return step;
		}
	}
	return -1;
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
	if ( enginePoolSelfCheck() != 0 )
	{
		fprintf( stderr, "self-check: the engine pool failed its own checks\n" );
		return 1;
	}

	// Two identical serial runs hash identically at every step, and the
	// engine-shaped pool at four workers (whole team and dedicated subset) runs
	// every task the library enqueues and reaches the same hashes.
	BodyList list = { 0 };
	float stepTimes[1000];
	RunResult* a = malloc( sizeof( RunResult ) );
	RunResult* b = malloc( sizeof( RunResult ) );
	if ( a == NULL || b == NULL )
	{
		fprintf( stderr, "out of memory\n" );
		return 1;
	}
	RunScene( &SCENES[1], SCHEDULER_NATIVE, 1, 20, a, &list, stepTimes );
	RunScene( &SCENES[1], SCHEDULER_NATIVE, 1, 20, b, &list, stepTimes );
	if ( a->hashedBodyCount != a->bodyCount || a->hashedBodyCount == 0 || FirstDivergentStep( a, b ) != -1 )
	{
		fprintf( stderr, "self-check: two serial runs differ, or bodies were missed (%d of %d hashed)\n", a->hashedBodyCount,
				 a->bodyCount );
		return 1;
	}
	RunScene( &SCENES[1], SCHEDULER_ENGINE_WHOLE, 4, 20, b, &list, stepTimes );
	if ( b->pool.enqueued == 0 || b->pool.ranByStepThread + b->pool.ranByWorkers != b->pool.enqueued ||
		 b->hashedBodyCount != a->hashedBodyCount || FirstDivergentStep( a, b ) != -1 )
	{
		fprintf( stderr, "self-check: the whole-team pool enqueued nothing, lost tasks or diverged (%" PRIu64 " enqueued, %" PRIu64
						 " run)\n",
				 b->pool.enqueued, b->pool.ranByStepThread + b->pool.ranByWorkers );
		return 1;
	}
	RunScene( &SCENES[1], SCHEDULER_ENGINE_SUBSET, 4, 20, b, &list, stepTimes );
	if ( b->pool.enqueued == 0 || b->pool.syntheticChunks == 0 || b->hashedBodyCount != a->hashedBodyCount ||
		 FirstDivergentStep( a, b ) != -1 )
	{
		fprintf( stderr, "self-check: the dedicated-subset pool enqueued nothing, ran no synthetic chunk or diverged\n" );
		return 1;
	}
	RunScene( &SCENES[1], SCHEDULER_ENGINE_WHOLE, 1, 20, b, &list, stepTimes );
	if ( b->pool.enqueued == 0 || b->pool.ranByStepThread != b->pool.enqueued || FirstDivergentStep( a, b ) != -1 )
	{
		fprintf( stderr, "self-check: the one-worker pool did not run the tasks inline or diverged\n" );
		return 1;
	}

	free( a );
	free( b );
	free( list.ids );
	fprintf( stderr, "self-check passed\n" );
	return 0;
}

// --- Main ------------------------------------------------------------------

int main( int argc, char** argv )
{
	const char* cell = "unspecified";
	bool quick = false;
	bool selfCheck = false;
	for ( int i = 1; i < argc; ++i )
	{
		if ( strcmp( argv[i], "--quick" ) == 0 )
		{
			quick = true;
		}
		else if ( strcmp( argv[i], "--self-check" ) == 0 )
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

	// The base counts, plus 16 and the machine's own logical processor count on
	// a machine with more than 8 (the founder's machine has 16 cores and 32
	// logical processors), capped by what Box2D accepts.
	int workerCounts[MAX_WORKER_COUNTS];
	int workerCountCount = 0;
	for ( int i = 0; i < ARRAY_COUNT( BASE_WORKER_COUNTS ); ++i )
	{
		workerCounts[workerCountCount++] = BASE_WORKER_COUNTS[i];
	}
	int cores = GetNumberOfCores();
	if ( cores > B2_MAX_WORKERS )
	{
		cores = B2_MAX_WORKERS;
	}
	if ( cores >= 16 )
	{
		workerCounts[workerCountCount++] = 16;
	}
	if ( cores > workerCounts[workerCountCount - 1] )
	{
		workerCounts[workerCountCount++] = cores;
	}

	int sceneCount = ARRAY_COUNT( SCENES );
	// Per scene and team: native, engine whole team, engine dedicated subset (teams of two or more).
	int runsPerScene = 0;
	for ( int c = 0; c < workerCountCount; ++c )
	{
		runsPerScene += workerCounts[c] >= 2 ? 3 : 2;
	}
	int runCount = sceneCount * runsPerScene;

	RunResult* results = calloc( (size_t)runCount, sizeof( RunResult ) );
	RunResult* candidate = calloc( 1, sizeof( RunResult ) );
	float* stepTimes = calloc( (size_t)MAX_STEPS, sizeof( float ) );
	BodyList list = { 0 };
	if ( results == NULL || candidate == NULL || stepTimes == NULL )
	{
		fprintf( stderr, "out of memory\n" );
		return 1;
	}
	int rounds = quick ? 1 : ROUNDS;

	int runIndex = 0;
	for ( int s = 0; s < sceneCount; ++s )
	{
		const Scene* scene = &SCENES[s];
		int steps = quick ? ( scene->steps / 10 < 20 ? 20 : scene->steps / 10 ) : scene->steps;
		RunResult* reference = NULL;
		const RunResult* nativeByTeam[MAX_WORKER_COUNTS] = { 0 };
		for ( int c = 0; c < workerCountCount; ++c )
		{
			int team = workerCounts[c];
			for ( int k = 0; k < 3; ++k )
			{
				SchedulerKind kind = (SchedulerKind)k;
				if ( kind == SCHEDULER_ENGINE_SUBSET && team < 2 )
				{
					continue;
				}
				RunResult* result = &results[runIndex];
				runIndex += 1;
				fprintf( stderr, "%s %s team %d ...", scene->name, SchedulerName( kind ), team );
				float roundsMsTotal[ROUNDS] = { 0 };
				bool roundsIdentical = true;
				for ( int round = 0; round < rounds; ++round )
				{
					RunResult* target = round == 0 ? result : candidate;
					RunScene( scene, kind, team, steps, target, &list, stepTimes );
					roundsMsTotal[round] = target->stepMsTotal;
					fprintf( stderr, " %.1f ms", target->stepMsTotal );
					if ( round > 0 )
					{
						roundsIdentical = roundsIdentical && candidate->finalHash == result->finalHash;
						if ( candidate->stepMsTotal < result->stepMsTotal )
						{
							memcpy( result, candidate, sizeof( RunResult ) );
						}
					}
				}
				result->rounds = rounds;
				memcpy( result->roundsMsTotal, roundsMsTotal, sizeof( roundsMsTotal ) );
				result->roundsIdentical = roundsIdentical;
				fprintf( stderr, ", hash %016" PRIx64 "%s\n", result->finalHash, roundsIdentical ? "" : " (rounds differ)" );
				if ( reference == NULL )
				{
					reference = result; // the native scheduler at one worker
				}
				else
				{
					result->firstDivergentStep = FirstDivergentStep( reference, result );
				}
				if ( kind == SCHEDULER_NATIVE )
				{
					nativeByTeam[c] = result;
					continue;
				}
				if ( nativeByTeam[c]->stepMsTotal > 0.0f )
				{
					result->costOverNative = result->stepMsTotal / nativeByTeam[c]->stepMsTotal;
				}
				// The native run at this run's library worker count, when one was
				// measured (the subset's half team is itself a base count).
				for ( int n = 0; n <= c; ++n )
				{
					if ( nativeByTeam[n] != NULL && nativeByTeam[n]->team == result->libraryWorkers &&
						 nativeByTeam[n]->stepMsTotal > 0.0f )
					{
						result->costOverNativeSameLibrary = result->stepMsTotal / nativeByTeam[n]->stepMsTotal;
					}
				}
			}
		}
	}

	b2Version version = b2GetVersion();
	printf( "{\"schema_version\":%d,\"spike\":\"box2d-pool-sharing\",\"cell\":", SCHEMA_VERSION );
	PrintJsonString( cell );
	printf( ",\"quick\":%s,", quick ? "true" : "false" );
	printf( "\"box2d\":{\"version\":\"%d.%d.%d\",\"git_tag\":\"v3.2.0\",\"license\":\"MIT\",\"avx2_available\":%s,"
			"\"detected_simd_width\":%d,\"max_workers\":%d,\"max_tasks_per_step\":%d},",
			version.major, version.minor, version.revision, b2IsAVX2Available() ? "true" : "false",
			b2IsAVX2Available() ? 8 : 4, B2_MAX_WORKERS, B2_MAX_TASKS );
	printf( "\"engine_pool\":{\"claiming\":\"central counter over a ring of published tasks\",\"spin_bound_us\":%d,"
			"\"waiting\":\"spin the bound then park on a condition variable\",\"finish\":\"helps the library's pending tasks only\","
			"\"one_worker\":\"the task runs inline in the enqueue\",\"dedicated_subset\":\"the library gets half the team, the other half runs a synthetic chunked pass during the step\"},",
			SPIN_BOUND_MICROSECONDS );
	printf( "\"settings\":{\"time_step\":%.9g,\"substeps\":%d,\"worker_counts\":[", (double)TIME_STEP, SUBSTEP_COUNT );
	for ( int c = 0; c < workerCountCount; ++c )
	{
		printf( "%s%d", c == 0 ? "" : ",", workerCounts[c] );
	}
	printf( "],\"rounds\":%d,\"kept_round\":\"the fastest\",\"reference_run\":\"native scheduler, 1 worker, same scene\","
			"\"cost_threshold_dedicated_cores\":1.10},",
			rounds );
	printf( "\"platform\":{\"os\":\"%s\",\"arch\":\"%s\",\"cores\":%d,\"compiler\":", OperatingSystem(), Architecture(),
			GetNumberOfCores() );
	PrintCompiler();
	printf( "},\"runs\":[" );
	for ( int i = 0; i < runCount; ++i )
	{
		const RunResult* r = &results[i];
		printf( "%s{\"scene\":\"%s\",\"scheduler\":\"%s\",\"team_workers\":%d,\"library_workers\":%d,\"steps\":%d,"
				"\"body_count\":%d,\"hashed_body_count\":%d,\"tasks_per_step\":%d,\"enqueued_tasks\":%" PRIu64
				",\"tasks_run_by_step_thread\":%" PRIu64 ",\"tasks_run_by_workers\":%" PRIu64 ",\"worker_parks\":%" PRIu64
				",\"finish_parks\":%" PRIu64 ",\"synthetic_chunks_per_step\":%.1f,\"synthetic_passes\":%" PRIu64
				",\"final_hash\":\"0x%016" PRIx64 "\",\"chain_hash\":\"0x%016" PRIx64 "\",\"matches_worker_sweep\":%s,"
				"\"step_ms_total\":%.3f,\"step_ms_median\":%.4f,\"step_ms_max\":%.3f,\"first_divergent_step\":%d,"
				"\"cost_over_native\":%.4f,\"cost_over_native_same_library_workers\":%.4f,\"rounds\":%d,"
				"\"rounds_identical\":%s,\"rounds_ms_total\":[",
				i == 0 ? "" : ",", r->scene->name, SchedulerName( r->scheduler ), r->team, r->libraryWorkers, r->steps,
				r->bodyCount, r->hashedBodyCount, r->taskCount, r->pool.enqueued, r->pool.ranByStepThread,
				r->pool.ranByWorkers, r->pool.workerParks, r->pool.finishParks, r->pool.syntheticChunksPerStep,
				r->pool.syntheticPasses, r->finalHash, r->chainHash, r->matchesWorkerSweep ? "true" : "false",
				(double)r->stepMsTotal, (double)r->stepMsMedian, (double)r->stepMsMax, r->firstDivergentStep,
				(double)r->costOverNative, (double)r->costOverNativeSameLibrary, r->rounds,
				r->roundsIdentical ? "true" : "false" );
		for ( int round = 0; round < r->rounds; ++round )
		{
			printf( "%s%.3f", round == 0 ? "" : ",", (double)r->roundsMsTotal[round] );
		}
		printf( "]}" );
	}
	printf( "]}\n" );

	free( results );
	free( candidate );
	free( stepTimes );
	free( list.ids );
	return 0;
}

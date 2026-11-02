// Box2D worker sweep: a throwaway spike of the physics research, never product code.
//
// Drives Box2D v3.2.0 through its task interface from an external scheduler
// (enkiTS) and through its native task system, on the author's benchmark scenes,
// at 1, 2, 4 and 8 workers, and hashes every body transform after every step.
// Identical hashes across worker counts and schedulers measure determinism
// under multithreading; identical hashes across the cells of the research CI
// measure cross-platform determinism. Step times are recorded as well, so that
// scaling compares with the numbers published in Box2D's benchmark directory.
//
// Prints one JSON document on standard output. See README.md.

#include "benchmarks.h"
#include "determinism.h"
#include "utils.h"

#include "box2d/base.h"
#include "box2d/box2d.h"
#include "box2d/constants.h"
#include "box2d/id.h"
#include "box2d/math_functions.h"
#include "box2d/types.h"

#include "TaskScheduler_c.h"

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
static const int HINGE_STEP_LIMIT = 1000;
static const int WORKER_COUNTS[] = { 1, 2, 4, 8 };

static const uint64_t FNV_OFFSET_BASIS = 0xcbf29ce484222325ULL;
static const uint64_t FNV_PRIME = 0x00000100000001b3ULL;

// The slot ring of the external scheduler. Box2D never enqueues more than
// B2_MAX_TASKS tasks per step and finishes every task before the step returns;
// the ring is twice that so that a slot is never reused while in flight.
#define TASK_SLOT_COUNT ( 2 * B2_MAX_TASKS )

typedef b2Capacity CapacityFcn( void );
typedef void CreateFcn( b2WorldId worldId );
typedef float StepFcn( b2WorldId worldId, int stepCount );

// The scenes come from Box2D's shared/benchmarks.c, with the step counts of
// Box2D's benchmark/main.c so that step times compare with the published numbers.
typedef struct Scene
{
	const char* name;
	CapacityFcn* capacity;
	CreateFcn* create;
	StepFcn* step;
	int steps;
} Scene;

static const Scene SCENES[] = {
	{ "joint_grid", GetJointGridCapacity, CreateJointGrid, NULL, 500 },
	{ "large_pyramid", GetLargePyramidCapacity, CreateLargePyramid, NULL, 500 },
	{ "many_pyramids", GetManyPyramidsCapacity, CreateManyPyramids, NULL, 200 },
	{ "rain", GetRainCapacity, CreateRain, StepRain, 1000 },
	{ "smash", GetSmashCapacity, CreateSmash, NULL, 300 },
	{ "spinner", GetSpinnerCapacity, CreateSpinner, StepSpinner, 500 },
	{ "tumbler", GetTumblerCapacity, CreateTumbler, NULL, 750 },
};

typedef enum SchedulerKind
{
	SCHEDULER_EXTERNAL, // enkiTS through b2WorldDef task callbacks
	SCHEDULER_NATIVE,	// Box2D's own task system (no callbacks); serial at one worker
} SchedulerKind;

static const char* SchedulerName( SchedulerKind kind )
{
	return kind == SCHEDULER_EXTERNAL ? "external" : "native";
}

// --- External scheduler: enkiTS behind the Box2D task callbacks -------------

typedef struct TaskSlot
{
	enkiTaskSet* set;
	b2TaskCallbackFcn* task;
	void* taskContext;
} TaskSlot;

typedef struct ExternalScheduler
{
	enkiTaskScheduler* scheduler;
	TaskSlot slots[TASK_SLOT_COUNT];
	int nextSlot;
	int enqueueCount;
} ExternalScheduler;

// Box2D v3.2 asks the task system to run the callback exactly once on a worker
// thread; Box2D distributes the work itself. A task set of size one does that.
static void RunBox2DTask( uint32_t start, uint32_t end, uint32_t threadIndex, void* args )
{
	(void)start;
	(void)end;
	(void)threadIndex;
	TaskSlot* slot = args;
	slot->task( slot->taskContext );
}

static void* EnqueueTask( b2TaskCallbackFcn* task, void* taskContext, void* userContext )
{
	ExternalScheduler* external = userContext;
	TaskSlot* slot = &external->slots[external->nextSlot];
	external->nextSlot = ( external->nextSlot + 1 ) % TASK_SLOT_COUNT;
	external->enqueueCount += 1;
	// A slot is only reused long after Box2D finished it; this wait is a safety net.
	if ( enkiIsTaskSetComplete( external->scheduler, slot->set ) == 0 )
	{
		enkiWaitForTaskSet( external->scheduler, slot->set );
	}
	slot->task = task;
	slot->taskContext = taskContext;
	enkiAddTaskSetArgs( external->scheduler, slot->set, slot, 1 );
	return slot;
}

static void FinishTask( void* userTask, void* userContext )
{
	ExternalScheduler* external = userContext;
	TaskSlot* slot = userTask;
	enkiWaitForTaskSet( external->scheduler, slot->set );
}

// enkiTS counts the calling thread as thread 0, as Box2D counts the thread that
// calls b2World_Step as worker 0, so the total thread count is the worker count.
static void InitExternalScheduler( ExternalScheduler* external, int workerCount )
{
	memset( external, 0, sizeof( *external ) );
	external->scheduler = enkiNewTaskScheduler();
	enkiInitTaskSchedulerNumThreads( external->scheduler, (uint32_t)workerCount );
	for ( int i = 0; i < TASK_SLOT_COUNT; ++i )
	{
		external->slots[i].set = enkiCreateTaskSet( external->scheduler, RunBox2DTask );
	}
}

static void DestroyExternalScheduler( ExternalScheduler* external )
{
	enkiWaitforAllAndShutdown( external->scheduler );
	for ( int i = 0; i < TASK_SLOT_COUNT; ++i )
	{
		enkiDeleteTaskSet( external->scheduler, external->slots[i].set );
	}
	enkiDeleteTaskScheduler( external->scheduler );
	external->scheduler = NULL;
}

static b2WorldId CreateWorld( SchedulerKind kind, int workerCount, ExternalScheduler* external, const Scene* scene,
							  int simdWidth )
{
	b2WorldDef def = b2DefaultWorldDef();
	def.workerCount = workerCount;
	if ( kind == SCHEDULER_EXTERNAL )
	{
		InitExternalScheduler( external, workerCount );
		def.enqueueTask = EnqueueTask;
		def.finishTask = FinishTask;
		def.userTaskContext = external;
	}
	if ( scene != NULL && scene->capacity != NULL )
	{
		def.capacity = scene->capacity();
	}
	b2WorldId worldId = b2CreateWorld( &def );
	// Width 4 forces the SSE2 path where AVX2 would be dispatched; elsewhere the
	// detected width is the only one and the flag has no effect.
	b2World_EnableSSE2Fallback( worldId, simdWidth == 4 );
	return worldId;
}

static void DestroyWorld( b2WorldId worldId, SchedulerKind kind, ExternalScheduler* external )
{
	b2DestroyWorld( worldId );
	if ( kind == SCHEDULER_EXTERNAL )
	{
		DestroyExternalScheduler( external );
	}
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

// Enumerates every shape through a broadphase query covering the whole scene,
// reduces the shapes to their bodies, sorts the bodies by their stored id (the
// creation slot comes first in it, so the order does not depend on the query's
// traversal), and hashes position and rotation field by field. Bodies without a
// shape are not reached; the body count of the world is reported next to the
// number of bodies hashed so that the gap is visible.
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

// --- One run: a scene, a scheduler, a worker count ---------------------------

typedef struct RunResult
{
	const Scene* scene;
	int simdWidth;
	SchedulerKind scheduler;
	int workers;
	int steps;
	int bodyCount;
	int hashedBodyCount;
	int taskCount;
	int enqueueCount;
	uint64_t finalHash;
	uint64_t chainHash;
	float stepMsTotal;
	float stepMsMedian;
	float stepMsMax;
	int firstDivergentStep;
	uint64_t stepHashes[1000];
} RunResult;

static int CompareFloats( const void* a, const void* b )
{
	float x = *(const float*)a;
	float y = *(const float*)b;
	return x < y ? -1 : ( x > y ? 1 : 0 );
}

static void RunScene( const Scene* scene, int simdWidth, SchedulerKind kind, int workers, int steps, RunResult* result,
					  BodyList* list, float* stepTimes )
{
	ExternalScheduler external;
	b2WorldId worldId = CreateWorld( kind, workers, &external, scene, simdWidth );
	scene->create( worldId );

	memset( result, 0, sizeof( *result ) );
	result->scene = scene;
	result->simdWidth = simdWidth;
	result->scheduler = kind;
	result->workers = workers;
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
		uint64_t ticks = b2GetTicks();
		b2World_Step( worldId, TIME_STEP, SUBSTEP_COUNT );
		float ms = b2GetMilliseconds( ticks );
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
	result->enqueueCount = kind == SCHEDULER_EXTERNAL ? external.enqueueCount : 0;
	result->finalHash = result->stepHashes[steps - 1];
	result->chainHash = chain;
	result->stepMsTotal = total;
	result->stepMsMax = max;
	qsort( stepTimes, (size_t)steps, sizeof( float ), CompareFloats );
	result->stepMsMedian = stepTimes[steps / 2];

	DestroyWorld( worldId, kind, &external );
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

// --- Falling hinges: the author's own determinism scenario -------------------

typedef struct HingeResult
{
	int simdWidth;
	SchedulerKind scheduler;
	int workers;
	uint32_t hash;
	int sleepStep;
	bool completed;
	bool matchesUpstream;
} HingeResult;

static void RunFallingHinges( int simdWidth, SchedulerKind kind, int workers, HingeResult* result )
{
	ExternalScheduler external;
	b2WorldId worldId = CreateWorld( kind, workers, &external, NULL, simdWidth );
	FallingHingeData data = CreateFallingHinges( worldId );

	bool done = false;
	for ( int step = 0; step < HINGE_STEP_LIMIT && done == false; ++step )
	{
		b2World_Step( worldId, TIME_STEP, SUBSTEP_COUNT );
		done = UpdateFallingHinges( worldId, &data );
	}

	result->simdWidth = simdWidth;
	result->scheduler = kind;
	result->workers = workers;
	result->hash = data.hash;
	result->sleepStep = data.sleepStep;
	result->completed = done;
	result->matchesUpstream = done && data.hash == EXPECTED_HASH && data.sleepStep == EXPECTED_SLEEP_STEP;

	DestroyFallingHinges( &data );
	DestroyWorld( worldId, kind, &external );
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
	// FNV-1a 64 reference vectors.
	if ( HashBytes( FNV_OFFSET_BASIS, "", 0 ) != 0xcbf29ce484222325ULL || HashBytes( FNV_OFFSET_BASIS, "a", 1 ) != 0xaf63dc4c8601ec8cULL )
	{
		fprintf( stderr, "self-check: FNV-1a vectors failed\n" );
		return 1;
	}

	// Two identical serial runs of a small scene hash identically at every step:
	// the driver carries no hidden state between runs, and the hash does not depend
	// on the traversal order of the broadphase query.
	BodyList list = { 0 };
	float stepTimes[1000];
	RunResult* a = malloc( sizeof( RunResult ) );
	RunResult* b = malloc( sizeof( RunResult ) );
	RunScene( &SCENES[1], 0, SCHEDULER_NATIVE, 1, 20, a, &list, stepTimes );
	RunScene( &SCENES[1], 0, SCHEDULER_NATIVE, 1, 20, b, &list, stepTimes );
	if ( a->hashedBodyCount != a->bodyCount || a->hashedBodyCount == 0 || FirstDivergentStep( a, b ) != -1 )
	{
		fprintf( stderr, "self-check: two serial runs differ, or bodies were missed (%d of %d hashed)\n", a->hashedBodyCount,
				 a->bodyCount );
		return 1;
	}

	// The external scheduler runs every task Box2D enqueues and the step completes.
	RunScene( &SCENES[1], 0, SCHEDULER_EXTERNAL, 2, 20, b, &list, stepTimes );
	if ( b->enqueueCount == 0 || b->hashedBodyCount != a->hashedBodyCount )
	{
		fprintf( stderr, "self-check: the external scheduler enqueued nothing or lost bodies\n" );
		return 1;
	}

	// The falling hinges scenario completes within the step limit.
	HingeResult hinge;
	RunFallingHinges( 0, SCHEDULER_NATIVE, 1, &hinge );
	if ( hinge.completed == false )
	{
		fprintf( stderr, "self-check: the falling hinges did not sleep within %d steps\n", HINGE_STEP_LIMIT );
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

	int detectedWidth = b2IsAVX2Available() ? 8 : 4;
	// On a machine with AVX2 both paths are measured: the dispatched one and the SSE2 fallback.
	int simdWidths[2] = { detectedWidth, 4 };
	int simdWidthCount = detectedWidth == 8 ? 2 : 1;
	int workerCountCount = ARRAY_COUNT( WORKER_COUNTS );
	int sceneCount = ARRAY_COUNT( SCENES );
	int runsPerScene = simdWidthCount * 2 * workerCountCount;
	int runCount = sceneCount * runsPerScene;

	RunResult* results = calloc( (size_t)runCount, sizeof( RunResult ) );
	HingeResult* hinges = calloc( (size_t)( simdWidthCount * 2 * workerCountCount ), sizeof( HingeResult ) );
	float* stepTimes = calloc( (size_t)MAX_STEPS, sizeof( float ) );
	BodyList list = { 0 };
	if ( results == NULL || hinges == NULL || stepTimes == NULL )
	{
		fprintf( stderr, "out of memory\n" );
		return 1;
	}

	int runIndex = 0;
	for ( int s = 0; s < sceneCount; ++s )
	{
		const Scene* scene = &SCENES[s];
		int steps = quick ? ( scene->steps / 10 < 20 ? 20 : scene->steps / 10 ) : scene->steps;
		for ( int w = 0; w < simdWidthCount; ++w )
		{
			RunResult* reference = NULL;
			for ( int k = 0; k < 2; ++k )
			{
				SchedulerKind kind = k == 0 ? SCHEDULER_EXTERNAL : SCHEDULER_NATIVE;
				for ( int c = 0; c < workerCountCount; ++c )
				{
					RunResult* result = &results[runIndex];
					runIndex += 1;
					fprintf( stderr, "%s width %d %s workers %d ...", scene->name, simdWidths[w], SchedulerName( kind ),
							 WORKER_COUNTS[c] );
					RunScene( scene, simdWidths[w], kind, WORKER_COUNTS[c], steps, result, &list, stepTimes );
					fprintf( stderr, " %.1f ms, hash %016" PRIx64 "\n", result->stepMsTotal, result->finalHash );
					if ( reference == NULL )
					{
						reference = result; // the external scheduler at one worker
					}
					else
					{
						result->firstDivergentStep = FirstDivergentStep( reference, result );
					}
				}
			}
		}
	}

	int hingeIndex = 0;
	for ( int w = 0; w < simdWidthCount; ++w )
	{
		for ( int k = 0; k < 2; ++k )
		{
			for ( int c = 0; c < workerCountCount; ++c )
			{
				RunFallingHinges( simdWidths[w], k == 0 ? SCHEDULER_EXTERNAL : SCHEDULER_NATIVE, WORKER_COUNTS[c],
								  &hinges[hingeIndex] );
				hingeIndex += 1;
			}
		}
	}

	b2Version version = b2GetVersion();
	printf( "{\"schema_version\":%d,", SCHEMA_VERSION );
	printf( "\"spike\":\"box2d-worker-sweep\",\"cell\":" );
	PrintJsonString( cell );
	printf( ",\"quick\":%s,", quick ? "true" : "false" );
	printf( "\"box2d\":{\"version\":\"%d.%d.%d\",\"git_tag\":\"v3.2.0\",\"license\":\"MIT\",\"double_precision\":%s,"
			"\"avx2_available\":%s,\"detected_simd_width\":%d,\"max_workers\":%d,\"max_tasks_per_step\":%d},",
			version.major, version.minor, version.revision,
#if defined( BOX2D_DOUBLE_PRECISION )
			"true",
#else
			"false",
#endif
			b2IsAVX2Available() ? "true" : "false", detectedWidth, B2_MAX_WORKERS, B2_MAX_TASKS );
	printf( "\"external_scheduler\":{\"name\":\"enkiTS\",\"git_tag\":\"v1.12\",\"license\":\"zlib\"},"
			"\"settings\":{\"time_step\":%.9g,\"substeps\":%d,\"worker_counts\":[",
			(double)TIME_STEP, SUBSTEP_COUNT );
	for ( int c = 0; c < workerCountCount; ++c )
	{
		printf( "%s%d", c == 0 ? "" : ",", WORKER_COUNTS[c] );
	}
	printf( "],\"reference_run\":\"external scheduler, 1 worker, same scene and SIMD width\"},\"platform\":{\"os\":\"%s\",\"arch\":\"%s\","
			"\"cores\":%d,\"compiler\":",
			OperatingSystem(), Architecture(), GetNumberOfCores() );
	PrintCompiler();
	printf( "},\"runs\":[" );
	for ( int i = 0; i < runCount; ++i )
	{
		const RunResult* r = &results[i];
		printf( "%s{\"scene\":\"%s\",\"simd_width\":%d,\"scheduler\":\"%s\",\"workers\":%d,\"steps\":%d,\"body_count\":%d,"
				"\"hashed_body_count\":%d,\"tasks_per_step\":%d,\"enqueued_tasks\":%d,\"final_hash\":\"0x%016" PRIx64 "\","
				"\"chain_hash\":\"0x%016" PRIx64 "\",\"step_ms_total\":%.3f,\"step_ms_median\":%.4f,\"step_ms_max\":%.3f,"
				"\"first_divergent_step\":%d}",
				i == 0 ? "" : ",", r->scene->name, r->simdWidth, SchedulerName( r->scheduler ), r->workers, r->steps,
				r->bodyCount, r->hashedBodyCount, r->taskCount, r->enqueueCount, r->finalHash, r->chainHash,
				(double)r->stepMsTotal, (double)r->stepMsMedian, (double)r->stepMsMax, r->firstDivergentStep );
	}
	printf( "],\"falling_hinges\":[" );
	for ( int i = 0; i < hingeIndex; ++i )
	{
		const HingeResult* h = &hinges[i];
		printf( "%s{\"simd_width\":%d,\"scheduler\":\"%s\",\"workers\":%d,\"hash\":\"0x%08" PRIx32 "\",\"sleep_step\":%d,"
				"\"completed\":%s,\"upstream_expected_hash\":\"0x%08" PRIx32 "\",\"upstream_expected_sleep_step\":%d,"
				"\"matches_upstream_expectation\":%s}",
				i == 0 ? "" : ",", h->simdWidth, SchedulerName( h->scheduler ), h->workers, h->hash, h->sleepStep,
				h->completed ? "true" : "false", (uint32_t)EXPECTED_HASH, EXPECTED_SLEEP_STEP,
				h->matchesUpstream ? "true" : "false" );
	}
	printf( "]}\n" );

	free( results );
	free( hinges );
	free( stepTimes );
	free( list.ids );
	return 0;
}

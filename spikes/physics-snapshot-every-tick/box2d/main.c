// Physics snapshot at every tick, the Box2D half: a throwaway spike of the
// networking research, never product code.
//
// Takes three parts after a tick of a Box2D v3.2.0 world (the library's image,
// the tables that map an entity to its library handle, the canonical bytes of
// the components), restores them in place for a correction of several ticks,
// and measures what that costs beside the fixed step: a take, a whole
// correction inside one frame at several cadences and depths, the memory of
// the ring, the image after a churn of bodies and of a sleeping world, the
// continuation after a restore at every frame, and a correction by
// write-back that needs no image.
//
// Prints one JSON document on standard output. See ../README.md.

#include "scenes.h"

#include <math.h>

typedef struct Config
{
	bool quick;
	int ladder[5];
	int ladderCount;
	int claimBodies;
	int warmupTicks;
	int forwardTicks;
	int corrections;
	int settleAfter;
	int continuationFrames;
	int churnTicks;
	int asleepTicksAtMost;
	int asleepTakes;
	int writeBackWarmup;
	int writeBackLaterTicks;
} Config;

static Config FullConfig( void )
{
	Config config = { false, { 100, 300, 1000, 3000, 10000 }, 5, 1000, 60, 600, 1000, 100, 1000, 10000, 3000, 200, 120, 600 };
	return config;
}

static Config QuickConfig( void )
{
	Config config = { true, { 100, 300, 0, 0, 0 }, 2, 300, 30, 60, 60, 30, 40, 300, 900, 20, 60, 60 };
	return config;
}

// --- Samples -------------------------------------------------------------------

typedef struct Samples
{
	double* values;
	int count;
	int capacity;
} Samples;

static void Push( Samples* samples, double value )
{
	if ( samples->count == samples->capacity )
	{
		samples->capacity = samples->capacity == 0 ? 1024 : 2 * samples->capacity;
		samples->values = Reallocate( samples->values, (size_t)samples->capacity * sizeof( double ) );
	}
	samples->values[samples->count++] = value;
}

static int CompareDoubles( const void* a, const void* b )
{
	double x = *(const double*)a;
	double y = *(const double*)b;
	return x < y ? -1 : ( x > y ? 1 : 0 );
}

typedef struct Summary
{
	double median;
	double p99;
	double max;
} Summary;

// The median, the 99th percentile (the value that at most one sample in a
// hundred exceeds) and the largest of the samples, which are then released.
static Summary Summarize( Samples* samples )
{
	Summary summary = { 0.0, 0.0, 0.0 };
	if ( samples->count > 0 )
	{
		qsort( samples->values, (size_t)samples->count, sizeof( double ), CompareDoubles );
		int n = samples->count;
		int rank = ( 99 * n + 99 ) / 100;
		summary.median = samples->values[n / 2];
		summary.p99 = samples->values[rank - 1];
		summary.max = samples->values[n - 1];
	}
	free( samples->values );
	memset( samples, 0, sizeof( *samples ) );
	return summary;
}

static void Tick( Sim* sim, int tick, bool mispredicted )
{
	SimApplyInputs( sim, tick, mispredicted );
	SimStep( sim );
	SimSync( sim );
}

// --- A take after every tick -----------------------------------------------------

typedef struct ForwardResult
{
	SceneKind scene;
	int bodies;
	int workers;
	int ticks;
	Summary step, sync, image, tables, canonical, take, tickAndTake;
	Summary imageBytes, takeBytes;
	int tablesBytes;
	int canonicalBytes;
	int ringSlots;
	int64_t ringBytes;
	uint64_t finalHash;
} ForwardResult;

static ForwardResult RunForward( const Config* config, SceneKind scene, int bodies, int workers )
{
	ForwardResult result;
	memset( &result, 0, sizeof( result ) );
	result.scene = scene;
	result.bodies = bodies;
	result.workers = workers;
	result.ticks = config->forwardTicks;
	result.ringSlots = 10;

	Sim sim;
	BuildScene( &sim, scene, bodies, workers, false, true );
	Ring ring;
	RingInit( &ring, result.ringSlots, 1 );
	for ( int tick = 1; tick <= config->warmupTicks; ++tick )
	{
		Tick( &sim, tick, false );
		// The last ticks of the warm-up fill the ring, so that no timed take
		// pays for the first use of a slot's memory.
		if ( tick > config->warmupTicks - result.ringSlots )
		{
			SimTake( &sim, RingSlot( &ring, tick ), tick );
		}
	}
	Samples step = { 0 }, sync = { 0 }, image = { 0 }, tables = { 0 }, canonical = { 0 }, take = { 0 }, both = { 0 };
	Samples imageBytes = { 0 }, takeBytes = { 0 };
	for ( int i = 1; i <= config->forwardTicks; ++i )
	{
		int tick = config->warmupTicks + i;
		SimApplyInputs( &sim, tick, false );
		uint64_t ticks = b2GetTicks();
		SimStep( &sim );
		double stepMs = ElapsedMs( &ticks );
		SimSync( &sim );
		double syncMs = ElapsedMs( &ticks );
		Take* slot = RingSlot( &ring, tick );
		TakeTimes times = SimTake( &sim, slot, tick );
		double takeMs = times.imageMs + times.tablesMs + times.canonicalMs;
		Push( &step, stepMs );
		Push( &sync, syncMs );
		Push( &image, times.imageMs );
		Push( &tables, times.tablesMs );
		Push( &canonical, times.canonicalMs );
		Push( &take, takeMs );
		Push( &both, stepMs + syncMs + takeMs );
		Push( &imageBytes, (double)slot->image.size );
		Push( &takeBytes, (double)TakeBytes( slot ) );
		result.tablesBytes = slot->tables.size;
		result.canonicalBytes = slot->canonical.size;
	}
	result.step = Summarize( &step );
	result.sync = Summarize( &sync );
	result.image = Summarize( &image );
	result.tables = Summarize( &tables );
	result.canonical = Summarize( &canonical );
	result.take = Summarize( &take );
	result.tickAndTake = Summarize( &both );
	result.imageBytes = Summarize( &imageBytes );
	result.takeBytes = Summarize( &takeBytes );
	result.ringBytes = RingBytes( &ring );
	result.finalHash = SimHash( &sim );
	RingFree( &ring );
	SimDestroy( &sim );
	return result;
}

// --- A correction inside one frame ------------------------------------------------

typedef struct CorrectionForm
{
	const char* name;
	int cadence;		 // a take of the three parts every so many ticks
	int depth;			 // the ticks a correction goes back
	bool confirmedAlone; // the ring holds the confirmed tick and no other
	bool checkContinuation;
} CorrectionForm;

static const CorrectionForm CORRECTION_FORMS[] = {
	{ "every_tick", 1, 2, false, false },
	{ "every_tick", 1, 8, false, true },
	{ "every_tick", 1, 24, false, false },
	{ "every_second_tick", 2, 8, false, false },
	{ "every_fourth_tick", 4, 8, false, false },
	{ "confirmed_tick_alone", 1, 8, true, false },
};

#define CORRECTION_FORM_COUNT ( (int)( sizeof( CORRECTION_FORMS ) / sizeof( CORRECTION_FORMS[0] ) ) )

typedef struct CorrectionResult
{
	SceneKind scene;
	int bodies;
	int workers;
	const CorrectionForm* form;
	int correctionsRun;
	bool settledBeforeTheEnd;
	int exceeding;
	int ticksAgainAtMost;
	// The first tick applied after the restore is among the ticks applied
	// again and is timed apart as well.
	Summary frame, restore, tickAgain, firstTickAgain;
	int ringSlots;
	int64_t ringBytes;
	uint64_t finalHash;
	int continuation; // -1 not checked, 0 the final hash differs from the uninterrupted world's, 1 equal
} CorrectionResult;

static CorrectionResult RunCorrections( const Config* config, SceneKind scene, int bodies, int workers,
										const CorrectionForm* form )
{
	CorrectionResult result;
	memset( &result, 0, sizeof( result ) );
	result.scene = scene;
	result.bodies = bodies;
	result.workers = workers;
	result.form = form;
	result.continuation = -1;

	int cadence = form->cadence;
	int depth = form->depth;
	// Every tick a frame restores from or writes is held at once: the ticks
	// from the one a correction starts from to the frame's new tick.
	result.ringSlots = form->confirmedAlone ? 1 : ( depth + cadence ) / cadence + 1;

	Sim sim;
	BuildScene( &sim, scene, bodies, workers, false, true );
	Ring ring;
	RingInit( &ring, result.ringSlots, cadence );
	int now = config->warmupTicks;
	for ( int tick = 1; tick <= now; ++tick )
	{
		Tick( &sim, tick, false );
		bool taken = form->confirmedAlone ? tick == now - depth : tick % cadence == 0;
		if ( taken )
		{
			SimTake( &sim, RingSlot( &ring, tick ), tick );
		}
	}

	Samples frame = { 0 }, restore = { 0 }, again = { 0 }, first = { 0 };
	for ( int n = 0; n < config->corrections; ++n )
	{
		uint64_t frameTicks = b2GetTicks();
		uint64_t ticks = frameTicks;
		int start = form->confirmedAlone ? now - depth : ( ( now - depth ) / cadence ) * cadence;
		SimRestore( &sim, RingHeld( &ring, start ) );
		Push( &restore, ElapsedMs( &ticks ) );
		if ( now - start > result.ticksAgainAtMost )
		{
			result.ticksAgainAtMost = now - start;
		}
		for ( int tick = start + 1; tick <= now + 1; ++tick )
		{
			Tick( &sim, tick, false );
			bool taken = form->confirmedAlone ? tick == start + 1 : tick % cadence == 0;
			if ( taken )
			{
				SimTake( &sim, RingSlot( &ring, tick ), tick );
			}
			if ( tick <= now )
			{
				double tickMs = ElapsedMs( &ticks );
				Push( &again, tickMs );
				if ( tick == start + 1 )
				{
					Push( &first, tickMs );
				}
			}
		}
		now += 1;
		double frameMs = (double)b2GetMilliseconds( frameTicks );
		Push( &frame, frameMs );
		result.correctionsRun += 1;
		if ( frameMs > FIXED_STEP_MS )
		{
			result.exceeding += 1;
		}
		// Eleven frames over the fixed step among a thousand put the 99th
		// percentile over it whatever the others measure: the verdict is
		// settled and the run stops.
		if ( result.correctionsRun >= config->settleAfter && result.exceeding >= 11 &&
			 result.correctionsRun < config->corrections )
		{
			result.settledBeforeTheEnd = true;
			break;
		}
	}
	result.frame = Summarize( &frame );
	result.restore = Summarize( &restore );
	result.tickAgain = Summarize( &again );
	result.firstTickAgain = Summarize( &first );
	result.ringBytes = RingBytes( &ring );
	result.finalHash = SimHash( &sim );
	RingFree( &ring );
	SimDestroy( &sim );

	if ( form->checkContinuation )
	{
		Sim uninterrupted;
		BuildScene( &uninterrupted, scene, bodies, workers, false, true );
		for ( int tick = 1; tick <= now; ++tick )
		{
			Tick( &uninterrupted, tick, false );
		}
		result.continuation = SimHash( &uninterrupted ) == result.finalHash ? 1 : 0;
		SimDestroy( &uninterrupted );
	}
	return result;
}

// --- The continuation after a restore at every frame ---------------------------------

typedef struct ContinuationResult
{
	const char* scene;
	int bodies;
	int workers;
	int frames;
	int sameInputsAgain;
	int amended;
	int mispredictedTicks;
	int mispredictedTicksThatDiffered;
	int ticksCompared;
	int firstDifferingTick; // -1 when every confirmed tick hashes as the uninterrupted world's
	uint64_t finalHash;
	uint64_t uninterruptedFinalHash;
	int imageBytes;
	double wallSeconds;
} ContinuationResult;

static ContinuationResult RunContinuation( const Config* config, const LibraryScene* scene, int workers )
{
	ContinuationResult result;
	memset( &result, 0, sizeof( result ) );
	result.scene = scene->name;
	result.workers = workers;
	result.frames = config->continuationFrames;
	result.firstDifferingTick = -1;
	uint64_t wall = b2GetTicks();

	const int warmup = 8;
	int lastTick = warmup + config->continuationFrames;
	uint64_t* reference = Allocate( (size_t)( lastTick + 1 ) * sizeof( uint64_t ) );

	// The world never restored, which receives the amended stream from the start.
	Sim uninterrupted;
	BuildLibraryScene( &uninterrupted, scene, workers );
	result.bodies = uninterrupted.count;
	for ( int tick = 1; tick <= lastTick; ++tick )
	{
		Tick( &uninterrupted, tick, false );
		reference[tick] = SimHash( &uninterrupted );
	}
	result.uninterruptedFinalHash = reference[lastTick];
	SimDestroy( &uninterrupted );

	Sim sim;
	BuildLibraryScene( &sim, scene, workers );
	Ring ring;
	RingInit( &ring, 10, 1 );
	Random random = { SCENE_SEED ^ 0x636f6e74696e7565ULL };
	int now = 0;
	bool lastMispredicted = false;
#define CONFIRM( tick )                                                                                               \
	do                                                                                                               \
	{                                                                                                                \
		result.ticksCompared += 1;                                                                                   \
		if ( SimHash( &sim ) != reference[tick] && result.firstDifferingTick == -1 )                                 \
		{                                                                                                            \
			result.firstDifferingTick = tick;                                                                        \
		}                                                                                                            \
	}                                                                                                                \
	while ( 0 )
	for ( int tick = 1; tick <= warmup; ++tick )
	{
		Tick( &sim, tick, false );
		SimTake( &sim, RingSlot( &ring, tick ), tick );
		CONFIRM( tick );
		now = tick;
	}
	for ( int frame = 0; frame <= config->continuationFrames; ++frame )
	{
		// The last pass is a correction of one tick with no new tick after it,
		// so that the last tick is confirmed too.
		bool closing = frame == config->continuationFrames;
		if ( closing && lastMispredicted == false )
		{
			break;
		}
		int depth = closing ? 1 : 1 + (int)( RandomNext( &random ) % 8 );
		SimRestore( &sim, RingHeld( &ring, now - depth ) );
		for ( int tick = now - depth + 1; tick <= now; ++tick )
		{
			Tick( &sim, tick, false );
			SimTake( &sim, RingSlot( &ring, tick ), tick );
			CONFIRM( tick );
		}
		if ( closing )
		{
			break;
		}
		if ( lastMispredicted )
		{
			result.amended += 1;
		}
		else
		{
			result.sameInputsAgain += 1;
		}
		bool mispredicted = ( RandomNext( &random ) & 1 ) != 0;
		now += 1;
		Tick( &sim, now, mispredicted );
		Take* slot = RingSlot( &ring, now );
		SimTake( &sim, slot, now );
		result.imageBytes = slot->image.size;
		if ( mispredicted )
		{
			result.mispredictedTicks += 1;
			if ( SimHash( &sim ) != reference[now] )
			{
				result.mispredictedTicksThatDiffered += 1;
			}
		}
		else
		{
			CONFIRM( now );
		}
		lastMispredicted = mispredicted;
	}
#undef CONFIRM
	result.finalHash = SimHash( &sim );
	RingFree( &ring );
	SimDestroy( &sim );
	free( reference );
	result.wallSeconds = (double)b2GetMilliseconds( wall ) / 1000.0;
	return result;
}

// --- The image after a churn of bodies ---------------------------------------------

typedef struct ChurnResult
{
	int bodies;
	int ticks;
	int changesPerTick;
	int churnedImageBytes;
	int freshImageBytes;
	int churnedTakeBytes;
	int freshTakeBytes;
	bool sameComponents;
} ChurnResult;

static ChurnResult RunChurn( const Config* config, int workers )
{
	ChurnResult result;
	memset( &result, 0, sizeof( result ) );
	result.bodies = config->claimBodies;
	result.ticks = config->churnTicks;
	result.changesPerTick = config->claimBodies / 100;
	int side = CeilSqrt( config->claimBodies );

	Sim churned;
	BuildScene( &churned, SCENE_MOTION, config->claimBodies, workers, false, true );
	Random random = { SCENE_SEED ^ 0x636875726e303031ULL };
	for ( int tick = 1; tick <= config->churnTicks; ++tick )
	{
		ChurnTick( &churned, &random, side );
		SimApplyInputs( &churned, tick, false );
		SimStep( &churned );
	}
	SimSync( &churned );
	Take churnedTake;
	memset( &churnedTake, 0, sizeof( churnedTake ) );
	SimTake( &churned, &churnedTake, config->churnTicks );

	// The fresh world: created from the components of the churned world's last
	// tick, so that it holds the same bodies with no history.
	Sim fresh;
	SimInit( &fresh, ( b2Vec2 ){ 0.0f, 0.0f }, workers, false, INPUT_SEED, SceneImpulseSpeed( SCENE_MOTION ) );
	BuildMotionWalls( &fresh, side );
	for ( int i = 0; i < churned.count; ++i )
	{
		SimSpawn( &fresh, &churned.entities[i].definition, &churned.components[i] );
	}
	SimRefreshPushable( &fresh );
	SimSync( &fresh );
	Take freshTake;
	memset( &freshTake, 0, sizeof( freshTake ) );
	SimTake( &fresh, &freshTake, config->churnTicks );

	result.churnedImageBytes = churnedTake.image.size;
	result.freshImageBytes = freshTake.image.size;
	result.churnedTakeBytes = TakeBytes( &churnedTake );
	result.freshTakeBytes = TakeBytes( &freshTake );
	result.sameComponents = SimHash( &churned ) == SimHash( &fresh );
	TakeFree( &churnedTake );
	TakeFree( &freshTake );
	SimDestroy( &churned );
	SimDestroy( &fresh );
	return result;
}

// --- A sleeping world --------------------------------------------------------------

typedef struct AsleepResult
{
	int bodies;
	int ticksUntilAsleep;
	int asleepBodies;
	int awakeBodiesAfterWaking;
	Summary asleepTake, awakeTake;
	int asleepImageBytes;
	int awakeImageBytes;
	int asleepTakeBytes;
	int awakeTakeBytes;
} AsleepResult;

static Summary TakeRepeatedly( Sim* sim, Take* take, int takes )
{
	Samples samples = { 0 };
	for ( int i = 0; i < takes; ++i )
	{
		TakeTimes times = SimTake( sim, take, 0 );
		Push( &samples, times.imageMs + times.tablesMs + times.canonicalMs );
	}
	return Summarize( &samples );
}

static AsleepResult RunAsleep( const Config* config, int workers )
{
	AsleepResult result;
	memset( &result, 0, sizeof( result ) );
	result.bodies = config->claimBodies;

	Sim sim;
	BuildScene( &sim, SCENE_PILE, config->claimBodies, workers, true, false );
	int tick = 0;
	while ( tick < config->asleepTicksAtMost && ( tick < 10 || b2World_GetAwakeBodyCount( sim.world ) > 0 ) )
	{
		tick += 1;
		SimStep( &sim );
	}
	result.ticksUntilAsleep = tick;
	SimSync( &sim );
	for ( int i = 0; i < sim.count; ++i )
	{
		result.asleepBodies += sim.components[i].awake == 0 ? 1 : 0;
	}
	Take take;
	memset( &take, 0, sizeof( take ) );
	TakeRepeatedly( &sim, &take, 5 );
	result.asleepTake = TakeRepeatedly( &sim, &take, config->asleepTakes );
	result.asleepImageBytes = take.image.size;
	result.asleepTakeBytes = TakeBytes( &take );

	// The same world awake: every body is woken, and one step gives it back
	// the contact constraints an awake world carries.
	for ( int i = 0; i < sim.count; ++i )
	{
		b2Body_SetAwake( sim.entities[i].body, true );
	}
	SimStep( &sim );
	SimSync( &sim );
	for ( int i = 0; i < sim.count; ++i )
	{
		result.awakeBodiesAfterWaking += sim.components[i].awake != 0 ? 1 : 0;
	}
	TakeRepeatedly( &sim, &take, 5 );
	result.awakeTake = TakeRepeatedly( &sim, &take, config->asleepTakes );
	result.awakeImageBytes = take.image.size;
	result.awakeTakeBytes = TakeBytes( &take );
	TakeFree( &take );
	SimDestroy( &sim );
	return result;
}

// --- The correction by write-back ---------------------------------------------------

typedef struct WriteBackResult
{
	SceneKind scene;
	int bodies;
	int workers;
	bool identicalBeforeTheCorrection;
	int firstDifferingTickOfTheEight; // 1 to 8, or -1 when the eight ticks hash as the uninterrupted world's
	double largestErrorOverBodySize;
	double largestErrorDistance;
	int firstTickTheHashesPart; // counted from the correction, or -1
	int tickTheyMeetAgain;		// counted from the correction, or -1
	bool equalAtTheEnd;
	double highestUninterrupted;
	double highestCorrected;
} WriteBackResult;

static double Highest( const Sim* sim )
{
	double highest = -1.0e30;
	for ( int i = 0; i < sim->count; ++i )
	{
		if ( (double)sim->components[i].py > highest )
		{
			highest = (double)sim->components[i].py;
		}
	}
	return highest;
}

static WriteBackResult RunWriteBack( const Config* config, SceneKind scene, int workers )
{
	WriteBackResult result;
	memset( &result, 0, sizeof( result ) );
	result.scene = scene;
	result.bodies = config->claimBodies;
	result.workers = workers;
	result.firstDifferingTickOfTheEight = -1;
	result.firstTickTheHashesPart = -1;
	result.tickTheyMeetAgain = -1;

	const int depth = 8;
	int base = config->writeBackWarmup;
	int corrected = base + depth;
	int last = corrected + config->writeBackLaterTicks;
	uint64_t* reference = Allocate( (size_t)( last + 1 ) * sizeof( uint64_t ) );

	Sim uninterrupted;
	BuildScene( &uninterrupted, scene, config->claimBodies, workers, false, true );
	float* places = Allocate( (size_t)uninterrupted.count * 2 * sizeof( float ) );
	for ( int tick = 1; tick <= last; ++tick )
	{
		Tick( &uninterrupted, tick, false );
		reference[tick] = SimHash( &uninterrupted );
		if ( tick == corrected )
		{
			for ( int i = 0; i < uninterrupted.count; ++i )
			{
				places[2 * i] = uninterrupted.components[i].px;
				places[2 * i + 1] = uninterrupted.components[i].py;
			}
		}
	}
	result.highestUninterrupted = Highest( &uninterrupted );
	SimDestroy( &uninterrupted );

	Sim sim;
	BuildScene( &sim, scene, config->claimBodies, workers, false, true );
	Take take;
	memset( &take, 0, sizeof( take ) );
	result.identicalBeforeTheCorrection = true;
	for ( int tick = 1; tick <= corrected; ++tick )
	{
		Tick( &sim, tick, false );
		if ( SimHash( &sim ) != reference[tick] )
		{
			result.identicalBeforeTheCorrection = false;
		}
		if ( tick == base )
		{
			SimTake( &sim, &take, tick );
		}
	}

	// The correction: the components of eight ticks ago are restored and
	// written into the bodies of the world as it stands now, then the eight
	// ticks are applied again with the same inputs.
	SimWriteBack( &sim, &take );
	bool parted = false;
	for ( int tick = base + 1; tick <= last; ++tick )
	{
		Tick( &sim, tick, false );
		bool equal = SimHash( &sim ) == reference[tick];
		int sinceCorrection = tick - base;
		if ( equal == false && parted == false )
		{
			parted = true;
			result.firstTickTheHashesPart = sinceCorrection;
			if ( sinceCorrection <= depth )
			{
				result.firstDifferingTickOfTheEight = sinceCorrection;
			}
		}
		if ( equal && parted && result.tickTheyMeetAgain == -1 )
		{
			result.tickTheyMeetAgain = sinceCorrection;
		}
		if ( tick == corrected )
		{
			for ( int i = 0; i < sim.count; ++i )
			{
				double dx = (double)sim.components[i].px - (double)places[2 * i];
				double dy = (double)sim.components[i].py - (double)places[2 * i + 1];
				double distance = sqrt( dx * dx + dy * dy );
				double relative = distance / (double)SizeOf( &sim.entities[i].definition );
				if ( relative > result.largestErrorOverBodySize )
				{
					result.largestErrorOverBodySize = relative;
					result.largestErrorDistance = distance;
				}
			}
		}
		if ( tick == last )
		{
			result.equalAtTheEnd = equal;
		}
	}
	result.highestCorrected = Highest( &sim );
	TakeFree( &take );
	SimDestroy( &sim );
	free( places );
	free( reference );
	return result;
}

// --- The report --------------------------------------------------------------------

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

static void PrintSummary( const char* name, Summary summary )
{
	printf( "\"%s_median\":%.4f,\"%s_p99\":%.4f,\"%s_max\":%.4f,", name, summary.median, name, summary.p99, name,
			summary.max );
}

static void PrintForward( const ForwardResult* r, bool first )
{
	printf( "%s{\"scene\":\"%s\",\"bodies\":%d,\"workers\":%d,\"ticks\":%d,", first ? "" : ",", SCENE_NAMES[r->scene],
			r->bodies, r->workers, r->ticks );
	PrintSummary( "step_ms", r->step );
	PrintSummary( "read_back_ms", r->sync );
	PrintSummary( "take_image_ms", r->image );
	PrintSummary( "take_tables_ms", r->tables );
	PrintSummary( "take_canonical_ms", r->canonical );
	PrintSummary( "take_ms", r->take );
	PrintSummary( "tick_and_take_ms", r->tickAndTake );
	double oneTake = r->takeBytes.median;
	printf( "\"image_bytes\":%.0f,\"image_bytes_largest\":%.0f,\"tables_bytes\":%d,\"canonical_bytes\":%d,"
			"\"take_bytes\":%.0f,\"image_bytes_per_body\":%.1f,\"tables_bytes_per_body\":%.1f,"
			"\"canonical_bytes_per_body\":%.1f,\"ring_slots\":%d,\"ring_bytes\":%" PRId64
			",\"ring_over_ten_takes\":%.4f,\"final_hash\":\"0x%016" PRIx64 "\"}",
			r->imageBytes.median, r->imageBytes.max, r->tablesBytes, r->canonicalBytes, oneTake,
			r->imageBytes.median / r->bodies, (double)r->tablesBytes / r->bodies, (double)r->canonicalBytes / r->bodies,
			r->ringSlots, r->ringBytes, oneTake > 0.0 ? (double)r->ringBytes / ( 10.0 * oneTake ) : 0.0, r->finalHash );
}

static void PrintCorrection( const CorrectionResult* r, bool first )
{
	printf( "%s{\"scene\":\"%s\",\"bodies\":%d,\"workers\":%d,\"takes\":\"%s\",\"cadence\":%d,\"depth\":%d,"
			"\"ticks_applied_again_at_most\":%d,\"corrections\":%d,\"settled_before_the_end\":%s,"
			"\"frames_over_the_fixed_step\":%d,",
			first ? "" : ",", SCENE_NAMES[r->scene], r->bodies, r->workers, r->form->name, r->form->cadence,
			r->form->depth, r->ticksAgainAtMost, r->correctionsRun, r->settledBeforeTheEnd ? "true" : "false",
			r->exceeding );
	PrintSummary( "frame_ms", r->frame );
	PrintSummary( "restore_ms", r->restore );
	PrintSummary( "tick_again_ms", r->tickAgain );
	PrintSummary( "first_tick_again_ms", r->firstTickAgain );
	printf( "\"fits_one_fixed_step\":%s,\"ring_slots\":%d,\"ring_bytes\":%" PRId64 ",\"final_hash\":\"0x%016" PRIx64
			"\",\"final_hash_equals_the_uninterrupted_world\":%s}",
			r->frame.p99 <= FIXED_STEP_MS ? "true" : "false", r->ringSlots, r->ringBytes, r->finalHash,
			r->continuation < 0 ? "null" : ( r->continuation == 1 ? "true" : "false" ) );
}

static void PrintContinuation( const ContinuationResult* r, bool first )
{
	printf( "%s{\"scene\":\"%s\",\"bodies\":%d,\"workers\":%d,\"frames\":%d,\"corrections_with_the_same_inputs\":%d,"
			"\"corrections_with_an_amended_input\":%d,\"mispredicted_ticks\":%d,"
			"\"mispredicted_ticks_that_differed\":%d,\"ticks_compared\":%d,\"first_differing_tick\":%d,"
			"\"final_hash\":\"0x%016" PRIx64 "\",\"uninterrupted_final_hash\":\"0x%016" PRIx64
			"\",\"image_bytes\":%d,\"wall_seconds\":%.1f}",
			first ? "" : ",", r->scene, r->bodies, r->workers, r->frames, r->sameInputsAgain, r->amended,
			r->mispredictedTicks, r->mispredictedTicksThatDiffered, r->ticksCompared, r->firstDifferingTick,
			r->finalHash, r->uninterruptedFinalHash, r->imageBytes, r->wallSeconds );
}

static void PrintWriteBack( const WriteBackResult* r, bool first )
{
	printf( "%s{\"scene\":\"%s\",\"bodies\":%d,\"workers\":%d,\"identical_before_the_correction\":%s,"
			"\"first_differing_tick_of_the_eight\":%d,\"largest_error_over_body_size\":%.6g,"
			"\"largest_error_distance\":%.6g,\"first_tick_the_hashes_part\":%d,\"tick_they_meet_again\":%d,"
			"\"equal_at_the_end\":%s,\"highest_body_uninterrupted\":%.4f,\"highest_body_corrected\":%.4f}",
			first ? "" : ",", SCENE_NAMES[r->scene], r->bodies, r->workers,
			r->identicalBeforeTheCorrection ? "true" : "false", r->firstDifferingTickOfTheEight,
			r->largestErrorOverBodySize, r->largestErrorDistance, r->firstTickTheHashesPart, r->tickTheyMeetAgain,
			r->equalAtTheEnd ? "true" : "false", r->highestUninterrupted, r->highestCorrected );
}

static bool Wanted( const char* only, const char* section )
{
	return only == NULL || strcmp( only, section ) == 0;
}

// --- Self-check: the driver's own logic, not the claims ---------------------------

static int SelfCheck( void )
{
	if ( HashBytes( FNV_OFFSET_BASIS, "", 0 ) != 0xcbf29ce484222325ULL ||
		 HashBytes( FNV_OFFSET_BASIS, "a", 1 ) != 0xaf63dc4c8601ec8cULL )
	{
		fprintf( stderr, "self-check: FNV-1a vectors failed\n" );
		return 1;
	}
	for ( int scene = 0; scene < SCENE_KIND_COUNT; ++scene )
	{
		Sim a, b;
		BuildScene( &a, (SceneKind)scene, 120, 1, false, true );
		BuildScene( &b, (SceneKind)scene, 120, 1, false, true );
		if ( a.count != 120 )
		{
			fprintf( stderr, "self-check: %s holds %d bodies, not 120\n", SCENE_NAMES[scene], a.count );
			return 1;
		}
		Take take;
		memset( &take, 0, sizeof( take ) );
		uint64_t atTake = 0;
		for ( int tick = 1; tick <= 30; ++tick )
		{
			Tick( &a, tick, false );
			Tick( &b, tick, false );
			if ( SimHash( &a ) != SimHash( &b ) )
			{
				fprintf( stderr, "self-check: two runs of %s differ at tick %d\n", SCENE_NAMES[scene], tick );
				return 1;
			}
			if ( tick == 20 )
			{
				SimTake( &a, &take, tick );
				atTake = SimHash( &a );
			}
		}
		uint64_t atEnd = SimHash( &a );
		SimRestore( &a, &take );
		if ( SimHash( &a ) != atTake )
		{
			fprintf( stderr, "self-check: the restored components of %s are not the taken ones\n", SCENE_NAMES[scene] );
			return 1;
		}
		for ( int tick = 21; tick <= 30; ++tick )
		{
			Tick( &a, tick, false );
		}
		if ( SimHash( &a ) != atEnd )
		{
			fprintf( stderr, "self-check: %s restored and stepped again differs\n", SCENE_NAMES[scene] );
			return 1;
		}
		// A mispredicted input must change what follows.
		SimRestore( &a, &take );
		Tick( &a, 21, true );
		SimRestore( &b, &take );
		Tick( &b, 21, false );
		if ( SimHash( &a ) == SimHash( &b ) )
		{
			fprintf( stderr, "self-check: a mispredicted input changed nothing in %s\n", SCENE_NAMES[scene] );
			return 1;
		}
		TakeFree( &take );
		SimDestroy( &a );
		SimDestroy( &b );
	}
	fprintf( stderr, "self-check passed\n" );
	return 0;
}

int main( int argc, char** argv )
{
	const char* cell = "unspecified";
	const char* only = NULL;
	bool quick = false;
	for ( int i = 1; i < argc; ++i )
	{
		if ( strcmp( argv[i], "--self-check" ) == 0 )
		{
			return SelfCheck();
		}
		else if ( strcmp( argv[i], "--quick" ) == 0 )
		{
			quick = true;
		}
		else if ( strcmp( argv[i], "--only" ) == 0 && i + 1 < argc )
		{
			only = argv[++i];
		}
		else
		{
			cell = argv[i];
		}
	}
	Config config = quick ? QuickConfig() : FullConfig();
	const int workerCounts[2] = { 1, 4 };

	b2Version version = b2GetVersion();
	printf( "{\"library\":\"box2d\",\"dimension\":\"2d\",\"cell\":\"%s\",\"version\":\"%d.%d.%d\",\"git_tag\":\"v3.2.0\","
			"\"license\":\"MIT\",\"quick\":%s,\"architecture\":\"%s\",\"compiler\":",
			cell, version.major, version.minor, version.revision, quick ? "true" : "false", Architecture() );
	PrintCompiler();
	printf( ",\"avx2_available\":%s,\"settings\":{\"time_step\":%.9g,\"substeps\":%d,\"fixed_step_ms\":%.4f,"
			"\"impulses_per_tick\":%d,\"warm_up_ticks\":%d,\"take_ticks\":%d,\"corrections\":%d,"
			"\"corrections_before_a_settled_verdict_stops_a_run\":%d,\"continuation_frames\":%d,\"churn_ticks\":%d,"
			"\"bodies_of_the_claims\":%d}",
			b2IsAVX2Available() ? "true" : "false", (double)TIME_STEP, SUBSTEP_COUNT, FIXED_STEP_MS, IMPULSES_PER_TICK,
			config.warmupTicks, config.forwardTicks, config.corrections, config.settleAfter, config.continuationFrames,
			config.churnTicks, config.claimBodies );

	printf( ",\"takes_after_every_tick\":[" );
	if ( Wanted( only, "takes" ) )
	{
		bool first = true;
		for ( int scene = 0; scene < SCENE_KIND_COUNT; ++scene )
		{
			for ( int s = 0; s < config.ladderCount; ++s )
			{
				for ( int w = 0; w < 2; ++w )
				{
					ForwardResult r = RunForward( &config, (SceneKind)scene, config.ladder[s], workerCounts[w] );
					fprintf( stderr, "takes %s %d bodies %d workers: step %.3f ms, take %.3f ms (p99 %.3f), %.0f bytes\n",
							 SCENE_NAMES[scene], r.bodies, r.workers, r.step.median, r.take.median, r.take.p99,
							 r.takeBytes.median );
					PrintForward( &r, first );
					first = false;
				}
			}
		}
	}
	printf( "],\"corrections\":[" );
	if ( Wanted( only, "corrections" ) )
	{
		bool first = true;
		for ( int scene = 0; scene < SCENE_KIND_COUNT; ++scene )
		{
			for ( int s = 0; s < config.ladderCount; ++s )
			{
				for ( int f = 0; f < CORRECTION_FORM_COUNT; ++f )
				{
					CorrectionResult r =
						RunCorrections( &config, (SceneKind)scene, config.ladder[s], 4, &CORRECTION_FORMS[f] );
					fprintf( stderr,
							 "corrections %s %d bodies, takes %s, depth %d: frame %.3f ms (p99 %.3f) over %d%s\n",
							 SCENE_NAMES[scene], r.bodies, r.form->name, r.form->depth, r.frame.median, r.frame.p99,
							 r.correctionsRun, r.settledBeforeTheEnd ? ", settled" : "" );
					PrintCorrection( &r, first );
					first = false;
				}
			}
		}
	}
	printf( "],\"continuation\":[" );
	if ( Wanted( only, "continuation" ) )
	{
		bool first = true;
		for ( int scene = 0; scene < LIBRARY_SCENE_COUNT; ++scene )
		{
			for ( int w = 0; w < 2; ++w )
			{
				ContinuationResult r = RunContinuation( &config, &LIBRARY_SCENES[scene], workerCounts[w] );
				fprintf( stderr, "continuation %s %d bodies %d workers: first differing tick %d, %.1f s\n", r.scene,
						 r.bodies, r.workers, r.firstDifferingTick, r.wallSeconds );
				PrintContinuation( &r, first );
				first = false;
			}
		}
	}
	printf( "],\"churn\":" );
	if ( Wanted( only, "churn" ) )
	{
		ChurnResult r = RunChurn( &config, 4 );
		fprintf( stderr, "churn: image %d bytes after the churn, %d bytes fresh\n", r.churnedImageBytes,
				 r.freshImageBytes );
		printf( "{\"scene\":\"bodies_in_motion\",\"bodies\":%d,\"ticks\":%d,\"destroyed_and_created_per_tick\":%d,"
				"\"churned_image_bytes\":%d,\"fresh_image_bytes\":%d,\"churned_over_fresh\":%.4f,"
				"\"churned_take_bytes\":%d,\"fresh_take_bytes\":%d,\"the_fresh_world_holds_the_same_components\":%s}",
				r.bodies, r.ticks, r.changesPerTick, r.churnedImageBytes, r.freshImageBytes,
				(double)r.churnedImageBytes / (double)r.freshImageBytes, r.churnedTakeBytes, r.freshTakeBytes,
				r.sameComponents ? "true" : "false" );
	}
	else
	{
		printf( "null" );
	}
	printf( ",\"asleep\":" );
	if ( Wanted( only, "asleep" ) )
	{
		AsleepResult r = RunAsleep( &config, 4 );
		fprintf( stderr, "asleep: %d of %d bodies asleep after %d ticks, take %.4f ms asleep and %.4f ms awake\n",
				 r.asleepBodies, r.bodies, r.ticksUntilAsleep, r.asleepTake.median, r.awakeTake.median );
		printf( "{\"scene\":\"pile_at_rest\",\"bodies\":%d,\"ticks_until_asleep\":%d,\"asleep_bodies\":%d,"
				"\"awake_bodies_after_waking\":%d,",
				r.bodies, r.ticksUntilAsleep, r.asleepBodies, r.awakeBodiesAfterWaking );
		PrintSummary( "asleep_take_ms", r.asleepTake );
		PrintSummary( "awake_take_ms", r.awakeTake );
		printf( "\"asleep_image_bytes\":%d,\"awake_image_bytes\":%d,\"asleep_take_bytes\":%d,\"awake_take_bytes\":%d,"
				"\"asleep_over_awake_time\":%.4f,\"asleep_over_awake_bytes\":%.4f}",
				r.asleepImageBytes, r.awakeImageBytes, r.asleepTakeBytes, r.awakeTakeBytes,
				r.awakeTake.median > 0.0 ? r.asleepTake.median / r.awakeTake.median : 0.0,
				(double)r.asleepTakeBytes / (double)r.awakeTakeBytes );
	}
	else
	{
		printf( "null" );
	}
	printf( ",\"write_back\":[" );
	if ( Wanted( only, "write_back" ) )
	{
		bool first = true;
		for ( int scene = 0; scene < SCENE_KIND_COUNT; ++scene )
		{
			for ( int w = 0; w < 2; ++w )
			{
				WriteBackResult r = RunWriteBack( &config, (SceneKind)scene, workerCounts[w] );
				fprintf( stderr, "write-back %s %d workers: hashes part at tick %d, largest error %.3g of a body\n",
						 SCENE_NAMES[scene], r.workers, r.firstTickTheHashesPart, r.largestErrorOverBodySize );
				PrintWriteBack( &r, first );
				first = false;
			}
		}
	}
	printf( "]}\n" );
	return 0;
}

// The engine-shaped pool of the Box2D pool sharing spike: a throwaway spike of
// the physics research, never product code. See engine_pool.h and README.md.
//
// Shape, borrowed from the engine's concurrency model: a fixed team of workers,
// the stepping thread counted as worker 0; the library's tasks published one by
// one into a ring and claimed under one central counter; a worker that finds
// nothing spins for at most the bound, then parks on a condition variable with
// the lost-wake-up check done under the lock; the finish call helps with the
// library's pending tasks only, then waits for its task; the dedicated-subset
// variant gives the library part of the team while the rest runs a synthetic
// parallel pass of the reference workload's kind for the duration of each step.

#include "engine_pool.h"

#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <cstdlib>
#include <mutex>
#include <thread>
#include <vector>

namespace
{

using Clock = std::chrono::steady_clock;

// Twice Box2D's B2_MAX_TASKS (256): the library finishes every task before the
// step returns, so a slot is never reused while its task is in flight.
constexpr uint32_t RING = 512;

enum ItemState : int
{
	ITEM_DONE = 0, // also the initial state of an unused slot
	ITEM_PENDING = 1,
	ITEM_RUNNING = 2,
};

struct Item
{
	EnginePoolTaskFcn* task = nullptr;
	void* context = nullptr;
	std::atomic<int> state{ ITEM_DONE };
};

// The synthetic pass: the reference workload's kind of frame-bound chunked
// float work (a damped spring integrated over rows), 256 rows per chunk, each
// synthetic worker on rows of its own so that the pass shares no memory.
constexpr size_t SYNTHETIC_CHUNK_ROWS = 256;
constexpr size_t SYNTHETIC_CHUNKS_PER_PASS = 64;

struct SyntheticRows
{
	std::vector<float> positions;
	std::vector<float> velocities;
};

} // namespace

struct EnginePool
{
	int teamSize = 1;
	int libraryWorkers = 1;
	std::chrono::microseconds spinBound{ 50 };

	std::vector<std::thread> threads;
	std::atomic<bool> quit{ false };

	// The ring and its three counters: slots reserved by publishers, slots
	// published (visible to claimers), slots claimed.
	Item items[RING];
	alignas( 64 ) std::atomic<uint32_t> reserved{ 0 };
	alignas( 64 ) std::atomic<uint32_t> tail{ 0 };
	alignas( 64 ) std::atomic<uint32_t> head{ 0 };

	// Parking of the library workers.
	std::mutex mutex;
	std::condition_variable wake;
	std::atomic<int> parked{ 0 };

	// Parking of the stepping thread inside finish.
	std::mutex finishMutex;
	std::condition_variable finishWake;
	std::atomic<int> finishParked{ 0 };

	// The synthetic half.
	std::atomic<bool> stepActive{ false };
	std::mutex syntheticMutex;
	std::condition_variable syntheticWake;
	std::atomic<uint32_t> syntheticClaim{ 0 };

	// Counters.
	std::atomic<uint64_t> enqueued{ 0 };
	std::atomic<uint64_t> ranByStepThread{ 0 };
	std::atomic<uint64_t> ranByWorkers{ 0 };
	std::atomic<uint64_t> workerParks{ 0 };
	std::atomic<uint64_t> finishParks{ 0 };
	std::atomic<uint64_t> syntheticChunks{ 0 };
	std::atomic<uint64_t> syntheticPasses{ 0 };
	std::atomic<uint64_t> steps{ 0 };
};

namespace
{

inline void Relax()
{
	std::this_thread::yield();
}

// Claims the next published task under the central counter and runs it.
// Returns false when nothing is pending.
bool ClaimAndRun( EnginePool& pool, bool byStepThread )
{
	uint32_t claim = pool.head.load( std::memory_order_acquire );
	for ( ;; )
	{
		uint32_t published = pool.tail.load( std::memory_order_acquire );
		if ( (int32_t)( published - claim ) <= 0 )
		{
			return false;
		}
		if ( pool.head.compare_exchange_weak( claim, claim + 1, std::memory_order_acq_rel, std::memory_order_acquire ) )
		{
			break;
		}
	}
	Item& item = pool.items[claim % RING];
	item.state.store( ITEM_RUNNING, std::memory_order_relaxed );
	item.task( item.context );
	// Sequentially consistent against the finisher's park: it increments
	// finishParked then reads the state; this stores the state then reads
	// finishParked, so one of the two sees the other.
	item.state.store( ITEM_DONE, std::memory_order_seq_cst );
	if ( pool.finishParked.load( std::memory_order_seq_cst ) > 0 )
	{
		std::lock_guard<std::mutex> guard( pool.finishMutex );
		pool.finishWake.notify_all();
	}
	if ( byStepThread )
	{
		pool.ranByStepThread.fetch_add( 1, std::memory_order_relaxed );
	}
	else
	{
		pool.ranByWorkers.fetch_add( 1, std::memory_order_relaxed );
	}
	return true;
}

void LibraryWorkerMain( EnginePool& pool )
{
	Clock::time_point idleSince = Clock::now();
	for ( ;; )
	{
		if ( pool.quit.load( std::memory_order_acquire ) )
		{
			return;
		}
		if ( ClaimAndRun( pool, false ) )
		{
			idleSince = Clock::now();
			continue;
		}
		if ( Clock::now() - idleSince < pool.spinBound )
		{
			Relax();
			continue;
		}
		std::unique_lock<std::mutex> lock( pool.mutex );
		pool.parked.fetch_add( 1, std::memory_order_seq_cst );
		pool.workerParks.fetch_add( 1, std::memory_order_relaxed );
		pool.wake.wait( lock, [&] {
			return pool.quit.load( std::memory_order_acquire ) ||
				   pool.tail.load( std::memory_order_seq_cst ) != pool.head.load( std::memory_order_seq_cst );
		} );
		pool.parked.fetch_sub( 1, std::memory_order_seq_cst );
		lock.unlock();
		idleSince = Clock::now();
	}
}

void RunSyntheticChunk( SyntheticRows& rows, size_t chunk )
{
	const float dt = 1.0f / 64.0f;
	const float stiffness = 4.0f;
	const float damping = 0.99609375f; // 1 - 2^-8, the reference workload's damping
	size_t begin = chunk * SYNTHETIC_CHUNK_ROWS;
	size_t end = begin + SYNTHETIC_CHUNK_ROWS;
	for ( size_t i = begin; i < end; ++i )
	{
		float p = rows.positions[i];
		float v = rows.velocities[i];
		v = v * damping - stiffness * p * dt;
		p = p + v * dt;
		rows.positions[i] = p;
		rows.velocities[i] = v;
	}
}

void SyntheticWorkerMain( EnginePool& pool, int index )
{
	SyntheticRows rows;
	rows.positions.resize( SYNTHETIC_CHUNK_ROWS * SYNTHETIC_CHUNKS_PER_PASS );
	rows.velocities.resize( SYNTHETIC_CHUNK_ROWS * SYNTHETIC_CHUNKS_PER_PASS );
	for ( size_t i = 0; i < rows.positions.size(); ++i )
	{
		rows.positions[i] = (float)( ( i + (size_t)index ) % 97 ) * 0.01f;
		rows.velocities[i] = 0.0f;
	}
	for ( ;; )
	{
		if ( pool.quit.load( std::memory_order_acquire ) )
		{
			return;
		}
		if ( pool.stepActive.load( std::memory_order_acquire ) == false )
		{
			std::unique_lock<std::mutex> lock( pool.syntheticMutex );
			pool.syntheticWake.wait( lock, [&] {
				return pool.quit.load( std::memory_order_acquire ) || pool.stepActive.load( std::memory_order_acquire );
			} );
			continue;
		}
		// The claim is a central counter like the frame pool's; the rows are the
		// worker's own, so claiming only orders the counting.
		uint32_t claim = pool.syntheticClaim.fetch_add( 1, std::memory_order_relaxed );
		RunSyntheticChunk( rows, claim % SYNTHETIC_CHUNKS_PER_PASS );
		pool.syntheticChunks.fetch_add( 1, std::memory_order_relaxed );
		if ( claim % SYNTHETIC_CHUNKS_PER_PASS == SYNTHETIC_CHUNKS_PER_PASS - 1 )
		{
			pool.syntheticPasses.fetch_add( 1, std::memory_order_relaxed );
		}
	}
}

void RunInline( EnginePool& pool, EnginePoolTaskFcn* task, void* context )
{
	task( context );
	pool.enqueued.fetch_add( 1, std::memory_order_relaxed );
	pool.ranByStepThread.fetch_add( 1, std::memory_order_relaxed );
}

} // namespace

extern "C"
{

EnginePool* enginePoolCreate( int teamSize, int libraryWorkers, int spinBoundMicroseconds )
{
	if ( teamSize < 1 || libraryWorkers < 1 || libraryWorkers > teamSize )
	{
		std::fprintf( stderr, "engine pool: invalid team %d / library workers %d\n", teamSize, libraryWorkers );
		std::abort();
	}
	EnginePool* pool = new EnginePool();
	pool->teamSize = teamSize;
	pool->libraryWorkers = libraryWorkers;
	pool->spinBound = std::chrono::microseconds( spinBoundMicroseconds );
	// Worker 0 is the stepping thread; the others are threads of the team.
	for ( int index = 1; index < teamSize; ++index )
	{
		if ( index < libraryWorkers )
		{
			pool->threads.emplace_back( [pool] { LibraryWorkerMain( *pool ); } );
		}
		else
		{
			pool->threads.emplace_back( [pool, index] { SyntheticWorkerMain( *pool, index ); } );
		}
	}
	return pool;
}

void enginePoolDestroy( EnginePool* pool )
{
	if ( pool == nullptr )
	{
		return;
	}
	pool->quit.store( true, std::memory_order_seq_cst );
	{
		std::lock_guard<std::mutex> guard( pool->mutex );
		pool->wake.notify_all();
	}
	{
		std::lock_guard<std::mutex> guard( pool->syntheticMutex );
		pool->syntheticWake.notify_all();
	}
	for ( std::thread& thread : pool->threads )
	{
		thread.join();
	}
	delete pool;
}

void enginePoolBeginStep( EnginePool* pool )
{
	pool->steps.fetch_add( 1, std::memory_order_relaxed );
	if ( pool->libraryWorkers < pool->teamSize )
	{
		std::lock_guard<std::mutex> guard( pool->syntheticMutex );
		pool->stepActive.store( true, std::memory_order_release );
		pool->syntheticWake.notify_all();
	}
}

void enginePoolEndStep( EnginePool* pool )
{
	pool->stepActive.store( false, std::memory_order_release );
}

void* enginePoolEnqueue( EnginePoolTaskFcn* task, void* taskContext, void* userContext )
{
	EnginePool& pool = *static_cast<EnginePool*>( userContext );
	if ( pool.libraryWorkers == 1 )
	{
		// No other library worker: the adapter runs the task inline and tells
		// the library so (a null user task needs no finish).
		RunInline( pool, task, taskContext );
		return nullptr;
	}
	uint32_t slot = pool.reserved.fetch_add( 1, std::memory_order_relaxed );
	Item& item = pool.items[slot % RING];
	if ( item.state.load( std::memory_order_acquire ) != ITEM_DONE )
	{
		std::fprintf( stderr, "engine pool: ring overflow, a slot is reused while in flight\n" );
		std::abort();
	}
	item.task = task;
	item.context = taskContext;
	item.state.store( ITEM_PENDING, std::memory_order_relaxed );
	// Publish in reservation order so that a claimer never sees a hole.
	uint32_t expected = slot;
	while ( pool.tail.compare_exchange_weak( expected, slot + 1, std::memory_order_seq_cst, std::memory_order_relaxed ) == false )
	{
		expected = slot;
		Relax();
	}
	pool.enqueued.fetch_add( 1, std::memory_order_relaxed );
	if ( pool.parked.load( std::memory_order_seq_cst ) > 0 )
	{
		std::lock_guard<std::mutex> guard( pool.mutex );
		pool.wake.notify_one();
	}
	if ( pool.finishParked.load( std::memory_order_seq_cst ) > 0 )
	{
		std::lock_guard<std::mutex> guard( pool.finishMutex );
		pool.finishWake.notify_all();
	}
	return &item;
}

void enginePoolFinish( void* userTask, void* userContext )
{
	EnginePool& pool = *static_cast<EnginePool*>( userContext );
	Item& item = *static_cast<Item*>( userTask );
	Clock::time_point idleSince = Clock::now();
	for ( ;; )
	{
		if ( item.state.load( std::memory_order_acquire ) == ITEM_DONE )
		{
			return;
		}
		// Help with the library's pending tasks, never with anything else.
		if ( ClaimAndRun( pool, true ) )
		{
			idleSince = Clock::now();
			continue;
		}
		if ( Clock::now() - idleSince < pool.spinBound )
		{
			Relax();
			continue;
		}
		std::unique_lock<std::mutex> lock( pool.finishMutex );
		pool.finishParked.fetch_add( 1, std::memory_order_seq_cst );
		pool.finishParks.fetch_add( 1, std::memory_order_relaxed );
		pool.finishWake.wait( lock, [&] {
			return item.state.load( std::memory_order_seq_cst ) == ITEM_DONE ||
				   pool.tail.load( std::memory_order_seq_cst ) != pool.head.load( std::memory_order_seq_cst );
		} );
		pool.finishParked.fetch_sub( 1, std::memory_order_seq_cst );
		lock.unlock();
		idleSince = Clock::now();
	}
}

EnginePoolCounters enginePoolCounters( const EnginePool* pool )
{
	EnginePoolCounters counters;
	counters.enqueued = pool->enqueued.load();
	counters.ranByStepThread = pool->ranByStepThread.load();
	counters.ranByWorkers = pool->ranByWorkers.load();
	counters.workerParks = pool->workerParks.load();
	counters.finishParks = pool->finishParks.load();
	counters.syntheticChunks = pool->syntheticChunks.load();
	counters.syntheticPasses = pool->syntheticPasses.load();
	uint64_t steps = pool->steps.load();
	counters.syntheticChunksPerStep = steps == 0 ? 0.0 : (double)counters.syntheticChunks / (double)steps;
	return counters;
}

void enginePoolResetCounters( EnginePool* pool )
{
	pool->enqueued.store( 0 );
	pool->ranByStepThread.store( 0 );
	pool->ranByWorkers.store( 0 );
	pool->workerParks.store( 0 );
	pool->finishParks.store( 0 );
	pool->syntheticChunks.store( 0 );
	pool->syntheticPasses.store( 0 );
	pool->steps.store( 0 );
}

// --- Self-check ---------------------------------------------------------------

namespace
{

struct CheckTask
{
	std::atomic<int> runs{ 0 };
	std::atomic<int>* total = nullptr;
	int spinIterations = 0;
};

void CheckTaskRun( void* context )
{
	CheckTask* task = static_cast<CheckTask*>( context );
	volatile float sink = 0.0f;
	for ( int i = 0; i < task->spinIterations; ++i )
	{
		sink = sink + 1.0f;
	}
	task->runs.fetch_add( 1 );
	task->total->fetch_add( 1 );
}

// Every task enqueued runs exactly once and finish returns only once its task
// is done, over several steps, so that slots are reused.
int CheckTeam( int teamSize, int libraryWorkers )
{
	EnginePool* pool = enginePoolCreate( teamSize, libraryWorkers, 50 );
	std::atomic<int> total{ 0 };
	const int tasksPerStep = 200;
	const int steps = 5;
	std::vector<CheckTask> tasks( tasksPerStep );
	for ( int step = 0; step < steps; ++step )
	{
		enginePoolBeginStep( pool );
		std::vector<void*> handles( tasksPerStep );
		for ( int i = 0; i < tasksPerStep; ++i )
		{
			tasks[i].total = &total;
			tasks[i].spinIterations = ( i % 7 ) * 1000;
			handles[i] = enginePoolEnqueue( CheckTaskRun, &tasks[i], pool );
		}
		for ( int i = 0; i < tasksPerStep; ++i )
		{
			if ( handles[i] != nullptr )
			{
				enginePoolFinish( handles[i], pool );
			}
			if ( tasks[i].runs.load() != step + 1 )
			{
				std::fprintf( stderr, "self-check: team %d/%d, task %d ran %d times after finish at step %d\n", teamSize,
							  libraryWorkers, i, tasks[i].runs.load(), step );
				enginePoolDestroy( pool );
				return 1;
			}
		}
		enginePoolEndStep( pool );
	}
	if ( total.load() != tasksPerStep * steps )
	{
		std::fprintf( stderr, "self-check: team %d/%d ran %d tasks of %d\n", teamSize, libraryWorkers, total.load(),
					  tasksPerStep * steps );
		enginePoolDestroy( pool );
		return 1;
	}
	EnginePoolCounters counters = enginePoolCounters( pool );
	if ( counters.enqueued != (uint64_t)( tasksPerStep * steps ) ||
		 counters.ranByStepThread + counters.ranByWorkers != (uint64_t)( tasksPerStep * steps ) )
	{
		std::fprintf( stderr, "self-check: team %d/%d counters disagree with the tasks\n", teamSize, libraryWorkers );
		enginePoolDestroy( pool );
		return 1;
	}
	if ( libraryWorkers < teamSize && counters.syntheticChunks == 0 )
	{
		std::fprintf( stderr, "self-check: the synthetic half ran nothing during the steps\n" );
		enginePoolDestroy( pool );
		return 1;
	}
	if ( libraryWorkers < teamSize )
	{
		// Between steps the synthetic half is parked: the chunk count stays.
		std::this_thread::sleep_for( std::chrono::milliseconds( 20 ) );
		uint64_t before = enginePoolCounters( pool ).syntheticChunks;
		std::this_thread::sleep_for( std::chrono::milliseconds( 20 ) );
		uint64_t after = enginePoolCounters( pool ).syntheticChunks;
		if ( before != after )
		{
			std::fprintf( stderr, "self-check: the synthetic half ran between steps (%llu then %llu chunks)\n",
						  (unsigned long long)before, (unsigned long long)after );
			enginePoolDestroy( pool );
			return 1;
		}
	}
	enginePoolDestroy( pool );
	return 0;
}

} // namespace

int enginePoolSelfCheck( void )
{
	if ( CheckTeam( 1, 1 ) != 0 || CheckTeam( 2, 2 ) != 0 || CheckTeam( 4, 4 ) != 0 || CheckTeam( 4, 2 ) != 0 ||
		 CheckTeam( 8, 8 ) != 0 )
	{
		return 1;
	}
	return 0;
}

} // extern "C"

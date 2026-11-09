// See engine_job_system.h.

#include "engine_job_system.h"

#include <cstdio>

using namespace JPH;

EngineJobSystem::EngineJobSystem( uint maxJobs, uint maxBarriers, int threadCount )
{
	JobSystemWithBarrier::Init( maxBarriers );
	mJobs.Init( maxJobs, maxJobs );
	for ( int index = 1; index < threadCount; ++index )
	{
		mThreads.emplace_back( [this] { WorkerMain(); } );
	}
}

EngineJobSystem::~EngineJobSystem()
{
	{
		std::lock_guard<std::mutex> guard( mMutex );
		mQuit = true;
	}
	mWake.notify_all();
	for ( std::thread& thread : mThreads )
	{
		thread.join();
	}
	// No job may linger in the queue: run and release what is left.
	for ( Job* job : mQueue )
	{
		job->Execute();
		job->Release();
	}
	mQueue.clear();
}

JobSystem::JobHandle EngineJobSystem::CreateJob( const char* name, ColorArg color, const JobFunction& function,
												 uint32 dependencyCount )
{
	uint32 index = mJobs.ConstructObject( name, color, this, function, dependencyCount );
	if ( index == FixedSizeFreeList<Job>::cInvalidObjectIndex )
	{
		std::fprintf( stderr, "engine job system: no job slot left (%llu created, %llu freed)\n",
					  (unsigned long long)mCreated.load(), (unsigned long long)mFreed.load() );
		std::abort();
	}
	mCreated.fetch_add( 1, std::memory_order_relaxed );
	Job* job = &mJobs.Get( index );
	// The handle keeps a reference; the job is queued below and may complete at once.
	JobHandle handle( job );
	if ( dependencyCount == 0 )
	{
		QueueJob( job );
	}
	return handle;
}

void EngineJobSystem::FreeJob( Job* job )
{
	mJobs.DestructObject( job );
	mFreed.fetch_add( 1, std::memory_order_relaxed );
}

void EngineJobSystem::QueueJob( Job* job )
{
	mQueued.fetch_add( 1, std::memory_order_relaxed );
	// Without workers the barrier's wait runs the job on the stepping thread.
	if ( mThreads.empty() )
	{
		return;
	}
	job->AddRef();
	{
		std::lock_guard<std::mutex> guard( mMutex );
		mQueue.push_back( job );
	}
	mWake.notify_one();
}

void EngineJobSystem::QueueJobs( Job** jobs, uint count )
{
	mQueued.fetch_add( count, std::memory_order_relaxed );
	if ( mThreads.empty() )
	{
		return;
	}
	{
		std::lock_guard<std::mutex> guard( mMutex );
		for ( uint i = 0; i < count; ++i )
		{
			jobs[i]->AddRef();
			mQueue.push_back( jobs[i] );
		}
	}
	if ( count >= mThreads.size() )
	{
		mWake.notify_all();
	}
	else
	{
		for ( uint i = 0; i < count; ++i )
		{
			mWake.notify_one();
		}
	}
}

void EngineJobSystem::WorkerMain()
{
	for ( ;; )
	{
		Job* job = nullptr;
		{
			std::unique_lock<std::mutex> lock( mMutex );
			mWake.wait( lock, [this] { return mQuit || mQueue.empty() == false; } );
			if ( mQueue.empty() )
			{
				return; // quitting, and the destructor drains what is left
			}
			job = mQueue.front();
			mQueue.pop_front();
		}
		// Execute is a compare-and-swap on the job's state: a job the waiting
		// thread already ran from the barrier is skipped here.
		job->Execute();
		job->Release();
		mRanByWorkers.fetch_add( 1, std::memory_order_relaxed );
	}
}

// --- Self-check ---------------------------------------------------------------

namespace
{

int CheckThreads( int threadCount )
{
	std::fprintf( stderr, "self-check: job system at %d threads\n", threadCount );
	EngineJobSystem jobs( 256, 4, threadCount );
	std::atomic<int> firstRuns{ 0 };
	std::atomic<int> secondRuns{ 0 };
	std::atomic<int> orderViolations{ 0 };
	const int rounds = 20;
	const int jobsPerRound = 40;
	for ( int round = 0; round < rounds; ++round )
	{
		JobSystem::Barrier* barrier = jobs.CreateBarrier();
		for ( int i = 0; i < jobsPerRound; ++i )
		{
			// The second job depends on the first: it must run after it.
			JobHandle second = jobs.CreateJob(
				"second", Color::sRed,
				[&] {
					if ( firstRuns.load() == 0 )
					{
						orderViolations.fetch_add( 1 );
					}
					secondRuns.fetch_add( 1 );
				},
				1 );
			JobHandle first = jobs.CreateJob(
				"first", Color::sGreen,
				[&, second] {
					volatile float sink = 0.0f;
					for ( int k = 0; k < 2000; ++k )
					{
						sink = sink + 1.0f;
					}
					firstRuns.fetch_add( 1 );
					second.RemoveDependency();
				},
				0 );
			barrier->AddJob( first );
			barrier->AddJob( second );
		}
		jobs.WaitForJobs( barrier );
		jobs.DestroyBarrier( barrier );
	}
	if ( firstRuns.load() != rounds * jobsPerRound || secondRuns.load() != rounds * jobsPerRound ||
		 orderViolations.load() != 0 )
	{
		std::fprintf( stderr, "self-check: job system at %d threads ran %d first and %d second jobs of %d, %d order violations\n",
					  threadCount, firstRuns.load(), secondRuns.load(), rounds * jobsPerRound, orderViolations.load() );
		return 1;
	}
	if ( threadCount > 1 && jobs.JobsRunByWorkers() == 0 )
	{
		std::fprintf( stderr, "self-check: the workers of a %d-thread job system ran nothing\n", threadCount );
		return 1;
	}
	return 0;
}

} // namespace

int engineJobSystemSelfCheck()
{
	return CheckThreads( 1 ) != 0 || CheckThreads( 2 ) != 0 || CheckThreads( 4 ) != 0 ? 1 : 0;
}

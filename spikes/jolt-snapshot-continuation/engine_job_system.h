// The job system of the Jolt snapshot continuation spike: a throwaway spike of
// the physics research, never product code.
//
// Jolt's abstract job system implemented over a fixed pool of threads, the way
// an engine with a pool of its own would drive the library: the jobs the
// library queues go into one queue claimed by the pool's workers, and the
// thread that waits on a barrier runs the barrier's jobs itself while it waits
// (JobSystemWithBarrier does that part). One thread means no worker at all:
// every job runs on the stepping thread inside the barrier's wait.
#pragma once

#include <Jolt/Jolt.h>

#include <Jolt/Core/FixedSizeFreeList.h>
#include <Jolt/Core/JobSystemWithBarrier.h>

#include <atomic>
#include <condition_variable>
#include <cstdint>
#include <deque>
#include <mutex>
#include <thread>
#include <vector>

class EngineJobSystem final : public JPH::JobSystemWithBarrier
{
public:
	// `threadCount` counts the stepping thread: a count of four starts three
	// workers, so that the count compares with Box2D's and Rapier's.
	EngineJobSystem( JPH::uint maxJobs, JPH::uint maxBarriers, int threadCount );
	~EngineJobSystem() override;

	int GetMaxConcurrency() const override
	{
		return (int)mThreads.size() + 1;
	}

	JobHandle CreateJob( const char* name, JPH::ColorArg color, const JobFunction& function,
						 JPH::uint32 dependencyCount = 0 ) override;

	uint64_t QueuedJobs() const
	{
		return mQueued.load();
	}

	uint64_t JobsRunByWorkers() const
	{
		return mRanByWorkers.load();
	}

	uint64_t JobsCreated() const
	{
		return mCreated.load();
	}

	uint64_t JobsFreed() const
	{
		return mFreed.load();
	}

protected:
	void QueueJob( Job* job ) override;
	void QueueJobs( Job** jobs, JPH::uint count ) override;
	void FreeJob( Job* job ) override;

private:
	void WorkerMain();

	JPH::FixedSizeFreeList<Job> mJobs;
	std::vector<std::thread> mThreads;
	std::mutex mMutex;
	std::condition_variable mWake;
	std::deque<Job*> mQueue;
	bool mQuit = false;
	std::atomic<uint64_t> mQueued{ 0 };
	std::atomic<uint64_t> mRanByWorkers{ 0 };
	std::atomic<uint64_t> mCreated{ 0 };
	std::atomic<uint64_t> mFreed{ 0 };
};

// Runs the job system's own checks (jobs with dependencies complete through a
// barrier at one and four threads, every job exactly once). Returns 0 on success.
int engineJobSystemSelfCheck();

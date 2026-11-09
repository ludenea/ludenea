// The engine-shaped pool of the Box2D pool sharing spike: a throwaway spike of
// the physics research, never product code.
//
// A fixed team of workers shaped like the engine's frame pool, exposed to the
// C driver through the two callbacks Box2D expects (enqueue, finish) plus the
// step boundaries and the counters the measurement reports. See README.md.
#pragma once

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C"
{
#endif

typedef struct EnginePool EnginePool;

// The task Box2D hands to the enqueue callback: the callback runs it exactly
// once on one of the workers (the library distributes its work itself).
typedef void EnginePoolTaskFcn( void* taskContext );

typedef struct EnginePoolCounters
{
	uint64_t enqueued;			   // tasks the library enqueued
	uint64_t ranByStepThread;	   // tasks the stepping thread ran while helping in finish
	uint64_t ranByWorkers;		   // tasks the team's other workers ran
	uint64_t workerParks;		   // times a worker parked after spinning the bound
	uint64_t finishParks;		   // times the stepping thread parked in finish
	uint64_t syntheticChunks;	   // chunks of the synthetic pass the other half ran during the steps
	uint64_t syntheticPasses;	   // whole synthetic passes completed during the steps
	double syntheticChunksPerStep; // synthetic chunks per library step, over the measured steps
} EnginePoolCounters;

// A team of `teamSize` workers, the stepping thread counted as one of them, so
// that the team equals Box2D's worker count. `libraryWorkers` of them claim the
// library's tasks (the stepping thread always among them); when it is smaller
// than the team, the remaining workers run a synthetic parallel pass of the
// reference workload's kind for the whole duration of every step (the
// dedicated-subset variant). `spinBoundMicroseconds` is the bound of the spin
// before a worker parks.
EnginePool* enginePoolCreate( int teamSize, int libraryWorkers, int spinBoundMicroseconds );
void enginePoolDestroy( EnginePool* pool );

// The step boundaries: the synthetic pass of the other half runs between them.
void enginePoolBeginStep( EnginePool* pool );
void enginePoolEndStep( EnginePool* pool );

// The two callbacks for b2WorldDef (userContext is the pool).
void* enginePoolEnqueue( EnginePoolTaskFcn* task, void* taskContext, void* userContext );
void enginePoolFinish( void* userTask, void* userContext );

EnginePoolCounters enginePoolCounters( const EnginePool* pool );
void enginePoolResetCounters( EnginePool* pool );

// Runs the pool's own checks (tasks run exactly once, finish returns only when
// the task is complete, the synthetic half runs during a step and not between
// steps). Returns 0 when they pass.
int enginePoolSelfCheck( void );

#ifdef __cplusplus
}
#endif

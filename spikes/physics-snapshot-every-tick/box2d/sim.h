// The Box2D half of the physics snapshot spike: a throwaway measurement, never
// product code. This header holds the world the experiments step: entities
// with a definition, the components read back from the library after a step,
// the three parts a take writes (the library's image, the tables that map an
// entity to its library handle, the canonical bytes of the components), the
// ring that keeps them, the restore in place and the write-back.
#ifndef PHYSICS_SNAPSHOT_SIM_H
#define PHYSICS_SNAPSHOT_SIM_H

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

#define TIME_STEP ( 1.0f / 60.0f )
#define SUBSTEP_COUNT 4
#define FIXED_STEP_MS ( 1000.0 / 60.0 )
#define IMPULSES_PER_TICK 4

#define FNV_OFFSET_BASIS 0xcbf29ce484222325ULL
#define FNV_PRIME 0x00000100000001b3ULL

static void Fail( const char* message )
{
	fprintf( stderr, "physics-snapshot-every-tick-box2d: %s\n", message );
	exit( 1 );
}

static void* Allocate( size_t bytes )
{
	void* memory = malloc( bytes == 0 ? 1 : bytes );
	if ( memory == NULL )
	{
		Fail( "out of memory" );
	}
	return memory;
}

static void* Reallocate( void* memory, size_t bytes )
{
	void* grown = realloc( memory, bytes == 0 ? 1 : bytes );
	if ( grown == NULL )
	{
		Fail( "out of memory" );
	}
	return grown;
}

static uint64_t HashBytes( uint64_t hash, const void* data, size_t count )
{
	const uint8_t* bytes = data;
	for ( size_t i = 0; i < count; ++i )
	{
		hash = ( hash ^ bytes[i] ) * FNV_PRIME;
	}
	return hash;
}

// --- The seeded stream (SplitMix64, as the reference workload's) -------------

typedef struct Random
{
	uint64_t state;
} Random;

static uint64_t RandomNext( Random* random )
{
	random->state += 0x9e3779b97f4a7c15ULL;
	uint64_t z = random->state;
	z = ( z ^ ( z >> 30 ) ) * 0xbf58476d1ce4e5b9ULL;
	z = ( z ^ ( z >> 27 ) ) * 0x94d049bb133111ebULL;
	return z ^ ( z >> 31 );
}

// A float in [-1, 1) from 24 bits of the stream: exact on every platform.
static float RandomSigned( Random* random )
{
	uint32_t bits = (uint32_t)( RandomNext( random ) >> 40 );
	return (float)bits * ( 1.0f / 8388608.0f ) - 1.0f;
}

// --- Entities, definitions and components -----------------------------------

enum
{
	KIND_CIRCLE = 0,
	KIND_BOX = 1,
	KIND_CAPSULE = 2,
	// A body of a scene the library's own code built: the definition holds what
	// the library says of it (type, mass, rotational inertia).
	KIND_FOREIGN = 3,
};

// What the engine would own as the definition of a body. Written field by
// field into the canonical bytes, never as the struct's memory.
typedef struct Definition
{
	uint32_t kind;
	uint32_t bodyType;
	uint32_t collides;
	float a; // circle radius, box half width, capsule half length, or mass
	float b; // box half height, capsule radius, or rotational inertia
	float density;
	float friction;
	float restitution;
} Definition;

#define DEFINITION_BYTES 32

typedef struct Components
{
	float px, py, qc, qs;
	float vx, vy, w;
	uint8_t awake;
} Components;

#define TRANSFORM_BYTES 16
#define MOTION_BYTES 12
#define CANONICAL_ROW_BYTES ( 8 + DEFINITION_BYTES + TRANSFORM_BYTES + MOTION_BYTES + 1 )
#define TABLE_ROW_BYTES 24

typedef struct Entity
{
	uint64_t id;
	b2BodyId body;
	uint64_t digest;
	Definition definition;
} Entity;

typedef struct Sim
{
	b2WorldId world;
	int workers;
	bool sleep;
	Entity* entities;
	Components* components;
	int count;
	int capacity;
	uint64_t nextId;
	// The indexes of the entities an input may push (the dynamic ones).
	int* pushable;
	int pushableCount;
	uint64_t inputSeed;
	float impulseSpeed;
} Sim;

static void PutU32( uint8_t** cursor, uint32_t value )
{
	uint8_t* p = *cursor;
	p[0] = (uint8_t)value;
	p[1] = (uint8_t)( value >> 8 );
	p[2] = (uint8_t)( value >> 16 );
	p[3] = (uint8_t)( value >> 24 );
	*cursor = p + 4;
}

static void PutU64( uint8_t** cursor, uint64_t value )
{
	PutU32( cursor, (uint32_t)value );
	PutU32( cursor, (uint32_t)( value >> 32 ) );
}

static void PutF32( uint8_t** cursor, float value )
{
	uint32_t bits;
	memcpy( &bits, &value, sizeof( bits ) );
	PutU32( cursor, bits );
}

static uint32_t GetU32( const uint8_t** cursor )
{
	const uint8_t* p = *cursor;
	*cursor = p + 4;
	return (uint32_t)p[0] | ( (uint32_t)p[1] << 8 ) | ( (uint32_t)p[2] << 16 ) | ( (uint32_t)p[3] << 24 );
}

static uint64_t GetU64( const uint8_t** cursor )
{
	uint64_t low = GetU32( cursor );
	uint64_t high = GetU32( cursor );
	return low | ( high << 32 );
}

static float GetF32( const uint8_t** cursor )
{
	uint32_t bits = GetU32( cursor );
	float value;
	memcpy( &value, &bits, sizeof( value ) );
	return value;
}

static void PutDefinition( uint8_t** cursor, const Definition* d )
{
	PutU32( cursor, d->kind );
	PutU32( cursor, d->bodyType );
	PutU32( cursor, d->collides );
	PutF32( cursor, d->a );
	PutF32( cursor, d->b );
	PutF32( cursor, d->density );
	PutF32( cursor, d->friction );
	PutF32( cursor, d->restitution );
}

static Definition GetDefinition( const uint8_t** cursor )
{
	Definition d;
	d.kind = GetU32( cursor );
	d.bodyType = GetU32( cursor );
	d.collides = GetU32( cursor );
	d.a = GetF32( cursor );
	d.b = GetF32( cursor );
	d.density = GetF32( cursor );
	d.friction = GetF32( cursor );
	d.restitution = GetF32( cursor );
	return d;
}

static uint64_t DigestOf( const Definition* d )
{
	uint8_t bytes[DEFINITION_BYTES];
	uint8_t* cursor = bytes;
	PutDefinition( &cursor, d );
	return HashBytes( FNV_OFFSET_BASIS, bytes, sizeof( bytes ) );
}

// The largest extent of the body's bounding box at rest.
static float SizeOf( const Definition* d )
{
	switch ( d->kind )
	{
		case KIND_CIRCLE:
			return 2.0f * d->a;
		case KIND_BOX:
			return 2.0f * ( d->a > d->b ? d->a : d->b );
		case KIND_CAPSULE:
			return 2.0f * ( d->a + d->b );
		default:
			return 1.0f;
	}
}

static void SimInit( Sim* sim, b2Vec2 gravity, int workers, bool sleep, uint64_t inputSeed, float impulseSpeed )
{
	memset( sim, 0, sizeof( *sim ) );
	b2WorldDef def = b2DefaultWorldDef();
	def.gravity = gravity;
	def.workerCount = workers;
	def.enableSleep = sleep;
	sim->world = b2CreateWorld( &def );
	sim->workers = workers;
	sim->sleep = sleep;
	sim->inputSeed = inputSeed;
	sim->impulseSpeed = impulseSpeed;
}

static void SimDestroy( Sim* sim )
{
	if ( b2World_IsValid( sim->world ) )
	{
		b2DestroyWorld( sim->world );
	}
	free( sim->entities );
	free( sim->components );
	free( sim->pushable );
	memset( sim, 0, sizeof( *sim ) );
}

static void SimReserve( Sim* sim, int count )
{
	if ( count <= sim->capacity )
	{
		return;
	}
	int capacity = sim->capacity == 0 ? 256 : sim->capacity;
	while ( capacity < count )
	{
		capacity *= 2;
	}
	sim->entities = Reallocate( sim->entities, (size_t)capacity * sizeof( Entity ) );
	sim->components = Reallocate( sim->components, (size_t)capacity * sizeof( Components ) );
	sim->pushable = Reallocate( sim->pushable, (size_t)capacity * sizeof( int ) );
	sim->capacity = capacity;
}

static b2BodyId CreateStaticBox( b2WorldId world, b2Vec2 center, float halfWidth, float halfHeight, float friction,
								 float restitution )
{
	b2BodyDef bodyDef = b2DefaultBodyDef();
	bodyDef.position = b2ToPos( center );
	b2BodyId body = b2CreateBody( world, &bodyDef );
	b2ShapeDef shapeDef = b2DefaultShapeDef();
	shapeDef.material.friction = friction;
	shapeDef.material.restitution = restitution;
	b2Polygon box = b2MakeBox( halfWidth, halfHeight );
	b2CreatePolygonShape( body, &shapeDef, &box );
	return body;
}

// Creates the library's body for a definition and a state, as a world created
// from components does.
static b2BodyId CreateLibraryBody( Sim* sim, const Definition* d, const Components* c )
{
	b2BodyDef bodyDef = b2DefaultBodyDef();
	bodyDef.type = (b2BodyType)d->bodyType;
	bodyDef.position = b2ToPos( ( b2Vec2 ){ c->px, c->py } );
	bodyDef.rotation = ( b2Rot ){ c->qc, c->qs };
	bodyDef.linearVelocity = ( b2Vec2 ){ c->vx, c->vy };
	bodyDef.angularVelocity = c->w;
	bodyDef.enableSleep = sim->sleep;
	bodyDef.isAwake = c->awake != 0;
	b2BodyId body = b2CreateBody( sim->world, &bodyDef );

	b2ShapeDef shapeDef = b2DefaultShapeDef();
	shapeDef.density = d->density;
	shapeDef.material.friction = d->friction;
	shapeDef.material.restitution = d->restitution;
	if ( d->collides == 0 )
	{
		shapeDef.filter.maskBits = 0;
	}
	if ( d->kind == KIND_CIRCLE )
	{
		b2Circle circle = { { 0.0f, 0.0f }, d->a };
		b2CreateCircleShape( body, &shapeDef, &circle );
	}
	else if ( d->kind == KIND_BOX )
	{
		b2Polygon box = b2MakeBox( d->a, d->b );
		b2CreatePolygonShape( body, &shapeDef, &box );
	}
	else
	{
		b2Capsule capsule = { { -d->a, 0.0f }, { d->a, 0.0f }, d->b };
		b2CreateCapsuleShape( body, &shapeDef, &capsule );
	}
	return body;
}

// Adds an entity that wraps an existing library body.
static int SimAdopt( Sim* sim, b2BodyId body, const Definition* d )
{
	SimReserve( sim, sim->count + 1 );
	int index = sim->count++;
	Entity* entity = &sim->entities[index];
	entity->id = sim->nextId++;
	entity->body = body;
	entity->definition = *d;
	entity->digest = DigestOf( d );
	memset( &sim->components[index], 0, sizeof( Components ) );
	return index;
}

static int SimSpawn( Sim* sim, const Definition* d, const Components* c )
{
	b2BodyId body = CreateLibraryBody( sim, d, c );
	int index = SimAdopt( sim, body, d );
	sim->components[index] = *c;
	return index;
}

// Removes the entity at an index, keeping the entities in identifier order.
static void SimDespawn( Sim* sim, int index )
{
	b2DestroyBody( sim->entities[index].body );
	int after = sim->count - index - 1;
	memmove( &sim->entities[index], &sim->entities[index + 1], (size_t)after * sizeof( Entity ) );
	memmove( &sim->components[index], &sim->components[index + 1], (size_t)after * sizeof( Components ) );
	sim->count -= 1;
}

static void SimRefreshPushable( Sim* sim )
{
	sim->pushableCount = 0;
	for ( int i = 0; i < sim->count; ++i )
	{
		if ( sim->entities[i].definition.bodyType == (uint32_t)b2_dynamicBody )
		{
			sim->pushable[sim->pushableCount++] = i;
		}
	}
}

// The inputs of one tick: impulses on a few bodies, a pure function of the
// seed and the tick. The mispredicted form pushes the first body the other
// way and three times as hard, which is what a correction then amends.
static void SimApplyInputs( Sim* sim, int tick, bool mispredicted )
{
	if ( sim->pushableCount == 0 || sim->impulseSpeed == 0.0f )
	{
		return;
	}
	Random random = { sim->inputSeed ^ ( (uint64_t)tick * 0xd6e8feb86659fd93ULL ) };
	for ( int i = 0; i < IMPULSES_PER_TICK; ++i )
	{
		int index = sim->pushable[RandomNext( &random ) % (uint64_t)sim->pushableCount];
		float x = RandomSigned( &random ) * sim->impulseSpeed;
		float y = RandomSigned( &random ) * sim->impulseSpeed;
		if ( mispredicted && i == 0 )
		{
			x *= -3.0f;
			y *= -3.0f;
		}
		b2BodyId body = sim->entities[index].body;
		float mass = b2Body_GetMass( body );
		b2Body_ApplyLinearImpulseToCenter( body, ( b2Vec2 ){ mass * x, mass * y }, true );
	}
}

static void SimStep( Sim* sim )
{
	b2World_Step( sim->world, TIME_STEP, SUBSTEP_COUNT );
}

// Reads the components back from the library, as the engine's tick does after
// every step whether or not anything is taken.
static void SimSync( Sim* sim )
{
	for ( int i = 0; i < sim->count; ++i )
	{
		b2BodyId body = sim->entities[i].body;
		Components* c = &sim->components[i];
		b2WorldTransform transform = b2Body_GetTransform( body );
		b2Vec2 position = b2ToVec2( transform.p );
		b2Vec2 velocity = b2Body_GetLinearVelocity( body );
		c->px = position.x;
		c->py = position.y;
		c->qc = transform.q.c;
		c->qs = transform.q.s;
		c->vx = velocity.x;
		c->vy = velocity.y;
		c->w = b2Body_GetAngularVelocity( body );
		c->awake = b2Body_IsAwake( body ) ? 1 : 0;
	}
}

// The hash of the transforms and the motion, in entity order.
static uint64_t SimHash( const Sim* sim )
{
	uint64_t hash = FNV_OFFSET_BASIS;
	for ( int i = 0; i < sim->count; ++i )
	{
		const Components* c = &sim->components[i];
		float fields[7] = { c->px, c->py, c->qc, c->qs, c->vx, c->vy, c->w };
		hash = HashBytes( hash, fields, sizeof( fields ) );
	}
	return hash;
}

// --- The three parts of a take, and the ring ---------------------------------

typedef struct Buffer
{
	uint8_t* data;
	int size;
	int capacity;
} Buffer;

// Grows to the size asked and never beyond it, so that a ring's memory is what
// its takes weigh.
static void BufferFit( Buffer* buffer, int size )
{
	if ( size > buffer->capacity )
	{
		buffer->data = Reallocate( buffer->data, (size_t)size );
		buffer->capacity = size;
	}
}

typedef struct Take
{
	Buffer image;
	Buffer tables;
	Buffer canonical;
	int tick;
	bool held;
} Take;

typedef struct TakeTimes
{
	double imageMs;
	double tablesMs;
	double canonicalMs;
} TakeTimes;

static int TakeBytes( const Take* take )
{
	return take->image.size + take->tables.size + take->canonical.size;
}

static void TakeFree( Take* take )
{
	free( take->image.data );
	free( take->tables.data );
	free( take->canonical.data );
	memset( take, 0, sizeof( *take ) );
}

static void TakeImage( Sim* sim, Buffer* image )
{
	int needed = b2World_GetSnapshot( sim->world, image->data, image->capacity );
	if ( needed <= 0 )
	{
		Fail( "the library gave no snapshot" );
	}
	if ( needed > image->capacity || image->data == NULL )
	{
		BufferFit( image, needed );
		needed = b2World_GetSnapshot( sim->world, image->data, image->capacity );
		if ( needed > image->capacity )
		{
			Fail( "the snapshot grew between two calls" );
		}
	}
	image->size = needed;
}

static void TakeTables( const Sim* sim, Buffer* tables )
{
	int size = 8 + sim->count * TABLE_ROW_BYTES;
	BufferFit( tables, size );
	uint8_t* cursor = tables->data;
	PutU64( &cursor, (uint64_t)sim->count );
	for ( int i = 0; i < sim->count; ++i )
	{
		const Entity* entity = &sim->entities[i];
		PutU64( &cursor, entity->id );
		PutU64( &cursor, b2StoreBodyId( entity->body ) );
		PutU64( &cursor, entity->digest );
	}
	tables->size = size;
}

static void TakeCanonical( const Sim* sim, Buffer* canonical )
{
	int size = 8 + sim->count * CANONICAL_ROW_BYTES;
	BufferFit( canonical, size );
	uint8_t* cursor = canonical->data;
	PutU64( &cursor, (uint64_t)sim->count );
	for ( int i = 0; i < sim->count; ++i )
	{
		const Components* c = &sim->components[i];
		PutU64( &cursor, sim->entities[i].id );
		PutDefinition( &cursor, &sim->entities[i].definition );
		PutF32( &cursor, c->px );
		PutF32( &cursor, c->py );
		PutF32( &cursor, c->qc );
		PutF32( &cursor, c->qs );
		PutF32( &cursor, c->vx );
		PutF32( &cursor, c->vy );
		PutF32( &cursor, c->w );
		*cursor++ = c->awake;
	}
	canonical->size = size;
}

static double ElapsedMs( uint64_t* ticks )
{
	return (double)b2GetMillisecondsAndReset( ticks );
}

static TakeTimes SimTake( Sim* sim, Take* take, int tick )
{
	TakeTimes times;
	uint64_t ticks = b2GetTicks();
	TakeImage( sim, &take->image );
	times.imageMs = ElapsedMs( &ticks );
	TakeTables( sim, &take->tables );
	times.tablesMs = ElapsedMs( &ticks );
	TakeCanonical( sim, &take->canonical );
	times.canonicalMs = ElapsedMs( &ticks );
	take->tick = tick;
	take->held = true;
	return times;
}

// Reads the tables and the canonical bytes back into the entities and the
// components. The entity set of a world under correction does not change, so
// the rows are checked against the entities in place.
static void RestoreTablesAndComponents( Sim* sim, const Take* take )
{
	const uint8_t* cursor = take->tables.data;
	uint64_t count = GetU64( &cursor );
	if ( count != (uint64_t)sim->count )
	{
		Fail( "a restored table holds another entity count" );
	}
	for ( int i = 0; i < sim->count; ++i )
	{
		Entity* entity = &sim->entities[i];
		entity->id = GetU64( &cursor );
		entity->body = b2LoadBodyId( GetU64( &cursor ) );
		entity->digest = GetU64( &cursor );
	}
	cursor = take->canonical.data;
	count = GetU64( &cursor );
	if ( count != (uint64_t)sim->count )
	{
		Fail( "restored components hold another entity count" );
	}
	for ( int i = 0; i < sim->count; ++i )
	{
		Components* c = &sim->components[i];
		uint64_t id = GetU64( &cursor );
		if ( id != sim->entities[i].id )
		{
			Fail( "restored components are not in entity order" );
		}
		sim->entities[i].definition = GetDefinition( &cursor );
		c->px = GetF32( &cursor );
		c->py = GetF32( &cursor );
		c->qc = GetF32( &cursor );
		c->qs = GetF32( &cursor );
		c->vx = GetF32( &cursor );
		c->vy = GetF32( &cursor );
		c->w = GetF32( &cursor );
		c->awake = *cursor++;
	}
}

// The restore in place of the three parts.
static void SimRestore( Sim* sim, const Take* take )
{
	if ( take->held == false )
	{
		Fail( "a correction asked for a tick the ring does not hold" );
	}
	if ( b2World_RestoreFromSnapshot( sim->world, take->image.data, take->image.size ) == false )
	{
		Fail( "the library refused its own snapshot" );
	}
	RestoreTablesAndComponents( sim, take );
}

// The correction that needs no image: the components are restored, the
// library's world stays where it is, and the restored transforms and motion
// are written into its bodies.
static void SimWriteBack( Sim* sim, const Take* take )
{
	RestoreTablesAndComponents( sim, take );
	for ( int i = 0; i < sim->count; ++i )
	{
		const Components* c = &sim->components[i];
		b2BodyId body = sim->entities[i].body;
		if ( sim->entities[i].definition.bodyType != (uint32_t)b2_dynamicBody )
		{
			continue;
		}
		b2Body_SetTransform( body, b2ToPos( ( b2Vec2 ){ c->px, c->py } ), ( b2Rot ){ c->qc, c->qs } );
		b2Body_SetLinearVelocity( body, ( b2Vec2 ){ c->vx, c->vy } );
		b2Body_SetAngularVelocity( body, c->w );
	}
}

typedef struct Ring
{
	Take* slots;
	int slotCount;
	int cadence;
} Ring;

static void RingInit( Ring* ring, int slotCount, int cadence )
{
	ring->slots = Allocate( (size_t)slotCount * sizeof( Take ) );
	memset( ring->slots, 0, (size_t)slotCount * sizeof( Take ) );
	ring->slotCount = slotCount;
	ring->cadence = cadence;
}

static void RingFree( Ring* ring )
{
	for ( int i = 0; i < ring->slotCount; ++i )
	{
		TakeFree( &ring->slots[i] );
	}
	free( ring->slots );
	memset( ring, 0, sizeof( *ring ) );
}

static Take* RingSlot( Ring* ring, int tick )
{
	return &ring->slots[( tick / ring->cadence ) % ring->slotCount];
}

// The take of a tick the ring holds, or a failure: a correction never starts
// from a tick that was overwritten.
static Take* RingHeld( Ring* ring, int tick )
{
	Take* take = RingSlot( ring, tick );
	if ( take->held == false || take->tick != tick )
	{
		Fail( "the ring no longer holds the tick a correction starts from" );
	}
	return take;
}

// The memory the ring holds: the capacity of every buffer of every slot.
static int64_t RingBytes( const Ring* ring )
{
	int64_t bytes = 0;
	for ( int i = 0; i < ring->slotCount; ++i )
	{
		const Take* take = &ring->slots[i];
		bytes += take->image.capacity + take->tables.capacity + take->canonical.capacity;
	}
	return bytes;
}

#endif

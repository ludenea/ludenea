// The scenes of the Box2D half: three that scale with a body count (a pile at
// rest, bodies in motion that seldom touch, chains of jointed bodies), the
// churn that destroys and creates bodies, and the library's own benchmark
// scenes the snapshot continuation spike ran, adopted as entities.
#ifndef PHYSICS_SNAPSHOT_SCENES_H
#define PHYSICS_SNAPSHOT_SCENES_H

#include "benchmarks.h"
#include "sim.h"

typedef enum SceneKind
{
	SCENE_PILE = 0,
	SCENE_MOTION = 1,
	SCENE_CHAINS = 2,
	SCENE_KIND_COUNT = 3,
} SceneKind;

static const char* const SCENE_NAMES[SCENE_KIND_COUNT] = { "pile_at_rest", "bodies_in_motion", "jointed_chains" };

#define SCENE_SEED 0x4c7564656e656132ULL
#define INPUT_SEED 0x696e707574733031ULL
#define PYRAMID_BASE 10
#define CHAIN_LINKS 10

static int CeilSqrt( int value )
{
	int side = 1;
	while ( side * side < value )
	{
		side += 1;
	}
	return side;
}

static Components RestingAt( float x, float y )
{
	Components c;
	memset( &c, 0, sizeof( c ) );
	c.px = x;
	c.py = y;
	c.qc = 1.0f;
	c.awake = 1;
	return c;
}

// Pyramids of boxes resting on a ground, ten boxes at the base, built row by
// row until the body count is reached.
static void BuildPile( Sim* sim, int bodies )
{
	int pyramids = ( bodies + 54 ) / 55;
	float pitch = 2.0f * 0.5f * PYRAMID_BASE + 2.0f;
	float halfGround = 0.5f * pitch * (float)pyramids + 20.0f;
	CreateStaticBox( sim->world, ( b2Vec2 ){ 0.0f, -1.0f }, halfGround, 1.0f, 0.6f, 0.0f );
	Definition box = { KIND_BOX, (uint32_t)b2_dynamicBody, 1, 0.5f, 0.5f, 1.0f, 0.6f, 0.0f };
	float a = 0.5f;
	int created = 0;
	for ( int p = 0; p < pyramids && created < bodies; ++p )
	{
		float origin = pitch * ( (float)p - 0.5f * (float)( pyramids - 1 ) );
		for ( int i = 0; i < PYRAMID_BASE && created < bodies; ++i )
		{
			float y = ( 2.0f * (float)i + 1.0f ) * a;
			for ( int j = i; j < PYRAMID_BASE && created < bodies; ++j )
			{
				float x = origin + ( (float)i + 1.0f ) * a + 2.0f * (float)( j - i ) * a - a * PYRAMID_BASE;
				Components c = RestingAt( x, y );
				SimSpawn( sim, &box, &c );
				created += 1;
			}
		}
	}
}

static Definition MotionDefinition( int kind )
{
	Definition d = { (uint32_t)kind, (uint32_t)b2_dynamicBody, 1, 0.5f, 0.5f, 1.0f, 0.0f, 1.0f };
	if ( kind == KIND_CAPSULE )
	{
		d.a = 0.3f;
		d.b = 0.3f;
	}
	return d;
}

// The four walls that keep the bodies in motion inside a square.
static void BuildMotionWalls( Sim* sim, int side )
{
	float extent = 4.0f * (float)side;
	float middle = 0.5f * extent - 2.0f;
	float half = 0.5f * extent + 4.0f;
	CreateStaticBox( sim->world, ( b2Vec2 ){ -4.0f, middle }, 1.0f, half, 0.0f, 1.0f );
	CreateStaticBox( sim->world, ( b2Vec2 ){ extent, middle }, 1.0f, half, 0.0f, 1.0f );
	CreateStaticBox( sim->world, ( b2Vec2 ){ middle, -4.0f }, half, 1.0f, 0.0f, 1.0f );
	CreateStaticBox( sim->world, ( b2Vec2 ){ middle, extent }, half, 1.0f, 0.0f, 1.0f );
}

// Circles, boxes and capsules on a grid four units apart, each with a seeded
// velocity, with no gravity, no friction and elastic contacts.
static void BuildMotion( Sim* sim, int bodies )
{
	int side = CeilSqrt( bodies );
	BuildMotionWalls( sim, side );
	Random random = { SCENE_SEED };
	for ( int i = 0; i < bodies; ++i )
	{
		Definition d = MotionDefinition( i % 3 );
		Components c = RestingAt( 4.0f * (float)( i % side ), 4.0f * (float)( i / side ) );
		c.vx = RandomSigned( &random );
		c.vy = RandomSigned( &random );
		c.w = RandomSigned( &random );
		SimSpawn( sim, &d, &c );
	}
}

// Chains of ten boxes linked by revolute joints, each hung by its first link
// from a static anchor and released horizontal; the links collide with
// nothing, so the work is the joints'.
static void BuildChains( Sim* sim, int bodies )
{
	b2BodyDef anchorDef = b2DefaultBodyDef();
	b2BodyId anchor = b2CreateBody( sim->world, &anchorDef );
	Definition link = { KIND_BOX, (uint32_t)b2_dynamicBody, 0, 0.5f, 0.125f, 1.0f, 0.6f, 0.0f };
	int created = 0;
	for ( int chain = 0; created < bodies; ++chain )
	{
		b2Vec2 hook = { 12.0f * (float)( chain % 16 ), 12.0f * (float)( chain / 16 ) };
		b2BodyId previous = anchor;
		for ( int k = 0; k < CHAIN_LINKS && created < bodies; ++k )
		{
			Components c = RestingAt( hook.x + 0.5f + (float)k, hook.y );
			int index = SimSpawn( sim, &link, &c );
			b2BodyId body = sim->entities[index].body;
			b2RevoluteJointDef joint = b2DefaultRevoluteJointDef();
			joint.base.bodyIdA = previous;
			joint.base.bodyIdB = body;
			joint.base.localFrameA.q = b2Rot_identity;
			joint.base.localFrameB.q = b2Rot_identity;
			joint.base.localFrameA.p = k == 0 ? hook : ( b2Vec2 ){ 0.5f, 0.0f };
			joint.base.localFrameB.p = ( b2Vec2 ){ -0.5f, 0.0f };
			b2CreateRevoluteJoint( sim->world, &joint );
			previous = body;
			created += 1;
		}
	}
}

static float SceneImpulseSpeed( SceneKind scene )
{
	return scene == SCENE_PILE ? 0.02f : 0.1f;
}

static void BuildScene( Sim* sim, SceneKind scene, int bodies, int workers, bool sleep, bool inputs )
{
	b2Vec2 gravity = scene == SCENE_MOTION ? ( b2Vec2 ){ 0.0f, 0.0f } : ( b2Vec2 ){ 0.0f, -10.0f };
	SimInit( sim, gravity, workers, sleep, INPUT_SEED, inputs ? SceneImpulseSpeed( scene ) : 0.0f );
	if ( scene == SCENE_PILE )
	{
		BuildPile( sim, bodies );
	}
	else if ( scene == SCENE_MOTION )
	{
		BuildMotion( sim, bodies );
	}
	else
	{
		BuildChains( sim, bodies );
	}
	SimRefreshPushable( sim );
	SimSync( sim );
}

// One tick of the churn: one body in a hundred is destroyed and as many are
// created, at seeded places inside the walls, the count held constant.
static void ChurnTick( Sim* sim, Random* random, int side )
{
	int changes = sim->count / 100;
	float extent = 4.0f * (float)side;
	for ( int i = 0; i < changes; ++i )
	{
		SimDespawn( sim, (int)( RandomNext( random ) % (uint64_t)sim->count ) );
	}
	for ( int i = 0; i < changes; ++i )
	{
		Definition d = MotionDefinition( (int)( RandomNext( random ) % 3 ) );
		float middle = 0.5f * extent - 2.0f;
		float reach = 0.5f * extent - 2.0f;
		Components c = RestingAt( middle + reach * RandomSigned( random ), middle + reach * RandomSigned( random ) );
		c.vx = RandomSigned( random );
		c.vy = RandomSigned( random );
		c.w = RandomSigned( random );
		SimSpawn( sim, &d, &c );
	}
	SimRefreshPushable( sim );
}

// --- The library's own benchmark scenes, adopted as entities ------------------

typedef b2Capacity CapacityFcn( void );
typedef void CreateFcn( b2WorldId worldId );

typedef struct LibraryScene
{
	const char* name;
	CapacityFcn* capacity;
	CreateFcn* create;
} LibraryScene;

// The five scenes the Box2D snapshot continuation spike ran, at their sizes.
static const LibraryScene LIBRARY_SCENES[] = {
	{ "joint_grid", GetJointGridCapacity, CreateJointGrid },
	{ "large_pyramid", GetLargePyramidCapacity, CreateLargePyramid },
	{ "many_pyramids", GetManyPyramidsCapacity, CreateManyPyramids },
	{ "smash", GetSmashCapacity, CreateSmash },
	{ "tumbler", GetTumblerCapacity, CreateTumbler },
};

#define LIBRARY_SCENE_COUNT ( (int)( sizeof( LIBRARY_SCENES ) / sizeof( LIBRARY_SCENES[0] ) ) )

typedef struct IdList
{
	uint64_t* ids;
	int count;
	int capacity;
} IdList;

static bool CollectBody( b2ShapeId shapeId, void* context )
{
	IdList* list = context;
	if ( list->count == list->capacity )
	{
		list->capacity = list->capacity == 0 ? 1024 : 2 * list->capacity;
		list->ids = Reallocate( list->ids, (size_t)list->capacity * sizeof( uint64_t ) );
	}
	list->ids[list->count++] = b2StoreBodyId( b2Shape_GetBody( shapeId ) );
	return true;
}

static int CompareIds( const void* a, const void* b )
{
	uint64_t x = *(const uint64_t*)a;
	uint64_t y = *(const uint64_t*)b;
	return x < y ? -1 : ( x > y ? 1 : 0 );
}

// Builds one of the library's scenes and adopts every body that carries a
// shape as an entity, in the order of the stored identifiers, which is the
// order the snapshot continuation spike hashed them in.
static void BuildLibraryScene( Sim* sim, const LibraryScene* scene, int workers )
{
	memset( sim, 0, sizeof( *sim ) );
	b2WorldDef def = b2DefaultWorldDef();
	def.workerCount = workers;
	def.capacity = scene->capacity();
	sim->world = b2CreateWorld( &def );
	sim->workers = workers;
	sim->sleep = true;
	sim->inputSeed = INPUT_SEED;
	sim->impulseSpeed = 0.5f;
	scene->create( sim->world );

	IdList list = { 0 };
	b2QueryFilter filter = b2DefaultQueryFilter();
	filter.categoryBits = UINT64_MAX;
	filter.maskBits = UINT64_MAX;
	b2AABB everything = { { -100000.0f, -100000.0f }, { 100000.0f, 100000.0f } };
	b2World_OverlapAABB( sim->world, b2Pos_zero, everything, filter, CollectBody, &list );
	qsort( list.ids, (size_t)list.count, sizeof( uint64_t ), CompareIds );
	for ( int i = 0; i < list.count; ++i )
	{
		if ( i > 0 && list.ids[i] == list.ids[i - 1] )
		{
			continue;
		}
		b2BodyId body = b2LoadBodyId( list.ids[i] );
		Definition d = { KIND_FOREIGN, (uint32_t)b2Body_GetType( body ), 1, b2Body_GetMass( body ),
						 b2Body_GetRotationalInertia( body ), 0.0f, 0.0f, 0.0f };
		SimAdopt( sim, body, &d );
	}
	free( list.ids );
	SimRefreshPushable( sim );
	SimSync( sim );
}

#endif

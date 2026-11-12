// Prepared collision geometry, the Box2D half: a throwaway spike of the assets
// and content pipelines research, never product code.
//
// Builds, with Box2D v3.2.0, the two shape kinds an importer makes from large
// inputs: convex polygons from point clouds and a chain from a polyline. For
// each kind it records whether a form built outside a world can be written and
// read back, what building costs against reading at three input sizes, the
// hash of the written bytes, and whether a world that reads the form steps
// like a world that built its shapes itself, for forms written by this run and
// by other cells.
//
// Usage: physics-prepared-geometry-box2d <cell> [<forms out directory>
//        [<forms from other cells directory> [<cell name> ...]]] [--quick]
//        physics-prepared-geometry-box2d --self-check
//
// Standard C has no directory listing, so the parent process names the cell
// directories found under the forms from other cells directory. Prints one
// JSON document on standard output and its progress on standard error. See
// ../README.md.

#if defined( _MSC_VER )
// fopen is the portable function this program needs; the secure variants the
// Microsoft runtime recommends are not standard C.
#define _CRT_SECURE_NO_WARNINGS
#endif

#include "box2d/base.h"
#include "box2d/box2d.h"
#include "box2d/collision.h"
#include "box2d/id.h"
#include "box2d/math_functions.h"
#include "box2d/types.h"

#include <inttypes.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define LADDER_COUNT 3
#define MAX_REPETITIONS 7
#define MAX_OTHER_CELLS 16
#define MAX_SOURCES ( MAX_OTHER_CELLS + 2 )
#define PATH_CAPACITY 1024
#define POINTS_PER_CLOUD B2_MAX_POLYGON_VERTICES

// field by field: count, radius, centroid, eight vertices, eight normals
#define POLYGON_FORM_BYTES ( 4u + 4u + 8u + 8u * B2_MAX_POLYGON_VERTICES + 8u * B2_MAX_POLYGON_VERTICES )

static const int SCHEMA_VERSION = 1;
static const int LADDER[LADDER_COUNT] = { 1000, 10000, 100000 };
static const float TIME_STEP = 1.0f / 60.0f;
static const double FIXED_STEP_MS = 1000.0 / 60.0;
static const int SUBSTEP_COUNT = 4;
static const int WORKER_COUNT = 1;
static const int STEPS = 600;
static const int COLUMNS = 25;

// A timed build or read is repeated: 7 times in an optimized build, 3 times
// in an unoptimized one.
#if defined( NDEBUG )
static const int REPETITIONS = 7;
#else
static const int REPETITIONS = 3;
#endif

static const uint64_t FNV_OFFSET_BASIS = 0xcbf29ce484222325ULL;
static const uint64_t FNV_PRIME = 0x00000100000001b3ULL;

static const uint64_t CLOUD_SEED = 0x436c6f7564733244ULL;
static const uint64_t CHAIN_SEED = 0x436861696e733244ULL;
static const uint64_t PLACEMENT_SEED = 0x506c6163656d3244ULL;

static const char* const POLYGON_FILE = "box2d-convex-polygons.bin";
static const char* const CHAIN_IMAGE_FILE = "box2d-chain-world-image.bin";

// --- Small helpers -----------------------------------------------------------

static void Fail( const char* message )
{
	fprintf( stderr, "%s\n", message );
	exit( 1 );
}

// Zeroed memory, so that no byte of an array is ever read before it is set.
static void* Allocate( size_t size )
{
	void* memory = calloc( 1, size > 0 ? size : 1 );
	if ( memory == NULL )
	{
		Fail( "out of memory" );
	}
	return memory;
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

static uint32_t FloatBits( float value )
{
	uint32_t bits;
	memcpy( &bits, &value, sizeof( bits ) );
	return bits;
}

static float FloatFromBits( uint32_t bits )
{
	float value;
	memcpy( &value, &bits, sizeof( value ) );
	return value;
}

// Values are hashed and written as little-endian bytes whatever the machine.
static uint64_t HashU32( uint64_t hash, uint32_t value )
{
	uint8_t bytes[4];
	bytes[0] = (uint8_t)value;
	bytes[1] = (uint8_t)( value >> 8 );
	bytes[2] = (uint8_t)( value >> 16 );
	bytes[3] = (uint8_t)( value >> 24 );
	return HashBytes( hash, bytes, sizeof( bytes ) );
}

static uint64_t HashU64( uint64_t hash, uint64_t value )
{
	hash = HashU32( hash, (uint32_t)value );
	return HashU32( hash, (uint32_t)( value >> 32 ) );
}

static uint64_t HashF32( uint64_t hash, float value )
{
	return HashU32( hash, FloatBits( value ) );
}

// --- Bytes written and read ----------------------------------------------------

typedef struct Bytes
{
	uint8_t* data;
	size_t count;
	size_t capacity;
} Bytes;

static void Reserve( Bytes* bytes, size_t extra )
{
	if ( bytes->count + extra <= bytes->capacity )
	{
		return;
	}
	size_t capacity = bytes->capacity == 0 ? 4096 : bytes->capacity;
	while ( capacity < bytes->count + extra )
	{
		capacity *= 2;
	}
	bytes->data = realloc( bytes->data, capacity );
	if ( bytes->data == NULL )
	{
		Fail( "out of memory" );
	}
	bytes->capacity = capacity;
}

static void PutU32( Bytes* bytes, uint32_t value )
{
	Reserve( bytes, 4 );
	bytes->data[bytes->count + 0] = (uint8_t)value;
	bytes->data[bytes->count + 1] = (uint8_t)( value >> 8 );
	bytes->data[bytes->count + 2] = (uint8_t)( value >> 16 );
	bytes->data[bytes->count + 3] = (uint8_t)( value >> 24 );
	bytes->count += 4;
}

static void PutF32( Bytes* bytes, float value )
{
	PutU32( bytes, FloatBits( value ) );
}

static void PutBytes( Bytes* bytes, const uint8_t* data, size_t count )
{
	Reserve( bytes, count );
	if ( count > 0 )
	{
		memcpy( bytes->data + bytes->count, data, count );
	}
	bytes->count += count;
}

static void FreeBytes( Bytes* bytes )
{
	free( bytes->data );
	bytes->data = NULL;
	bytes->count = 0;
	bytes->capacity = 0;
}

static bool SameBytes( const uint8_t* a, size_t countA, const uint8_t* b, size_t countB )
{
	return countA == countB && ( countA == 0 || memcmp( a, b, countA ) == 0 );
}

// The count of bytes that differ between two forms of one size, or -1 when
// their sizes differ: a form that differs by a few bytes at a regular stride
// is not the same finding as a form that differs throughout.
static int64_t CountDifferingBytes( const uint8_t* a, size_t countA, const uint8_t* b, size_t countB )
{
	if ( countA != countB )
	{
		return -1;
	}
	int64_t differing = 0;
	for ( size_t i = 0; i < countA; ++i )
	{
		differing += a[i] != b[i] ? 1 : 0;
	}
	return differing;
}

typedef struct Reader
{
	const uint8_t* data;
	size_t size;
	size_t cursor;
	bool ok;
} Reader;

static uint32_t TakeU32( Reader* reader )
{
	if ( reader->ok == false || reader->size - reader->cursor < 4 )
	{
		reader->ok = false;
		return 0;
	}
	const uint8_t* at = reader->data + reader->cursor;
	reader->cursor += 4;
	return (uint32_t)at[0] | ( (uint32_t)at[1] << 8 ) | ( (uint32_t)at[2] << 16 ) | ( (uint32_t)at[3] << 24 );
}

static float TakeF32( Reader* reader )
{
	return FloatFromBits( TakeU32( reader ) );
}

// --- Files ---------------------------------------------------------------------

static void JoinPath( char* out, const char* directory, const char* middle, const char* file )
{
	int written = middle == NULL ? snprintf( out, PATH_CAPACITY, "%s/%s", directory, file )
								 : snprintf( out, PATH_CAPACITY, "%s/%s/%s", directory, middle, file );
	if ( written < 0 || written >= PATH_CAPACITY )
	{
		Fail( "a path is too long" );
	}
}

static bool WriteBytesToFile( const char* path, const uint8_t* data, size_t count )
{
	FILE* file = fopen( path, "wb" );
	if ( file == NULL )
	{
		return false;
	}
	size_t written = count == 0 ? 0 : fwrite( data, 1, count, file );
	bool closed = fclose( file ) == 0;
	return written == count && closed;
}

static bool ReadBytesFromFile( const char* path, Bytes* out )
{
	FILE* file = fopen( path, "rb" );
	if ( file == NULL )
	{
		return false;
	}
	uint8_t chunk[65536];
	size_t read = fread( chunk, 1, sizeof( chunk ), file );
	while ( read > 0 )
	{
		PutBytes( out, chunk, read );
		read = fread( chunk, 1, sizeof( chunk ), file );
	}
	bool failed = ferror( file ) != 0;
	fclose( file );
	return failed == false;
}

// --- Timings -------------------------------------------------------------------

typedef struct Timing
{
	double median;
	double fastest;
	double slowest;
} Timing;

static int CompareFloats( const void* a, const void* b )
{
	float x = *(const float*)a;
	float y = *(const float*)b;
	return x < y ? -1 : ( x > y ? 1 : 0 );
}

static Timing SummarizeSamples( float* samples, int count )
{
	qsort( samples, (size_t)count, sizeof( float ), CompareFloats );
	Timing timing;
	timing.fastest = (double)samples[0];
	timing.median = (double)samples[count / 2];
	timing.slowest = (double)samples[count - 1];
	return timing;
}

// --- The seeded generator ------------------------------------------------------

// The SplitMix64 stream of the spikes' reference workload, ported as written
// there, so that the two halves of the spike draw from the same generator.
typedef struct SplitMix64
{
	uint64_t state;
} SplitMix64;

static uint64_t NextU64( SplitMix64* random )
{
	random->state += 0x9e3779b97f4a7c15ULL;
	uint64_t z = random->state;
	z = ( z ^ ( z >> 30 ) ) * 0xbf58476d1ce4e5b9ULL;
	z = ( z ^ ( z >> 27 ) ) * 0x94d049bb133111ebULL;
	return z ^ ( z >> 31 );
}

// A uniform value in [0, 1) with 24 significant bits: exact in float.
static float NextU01( SplitMix64* random )
{
	return (float)( NextU64( random ) >> 40 ) * ( 1.0f / 16777216.0f );
}

// A value in [-0.5, 0.5), exact.
static float NextCentered( SplitMix64* random )
{
	return NextU01( random ) - 0.5f;
}

// --- Inputs --------------------------------------------------------------------

// Every input is computed with sums and with single products, never a product
// followed by a sum that a compiler could fuse, so that every platform builds
// from the same input bytes; the hash of the bytes is recorded so that the
// cells can be compared on it.

static uint64_t HashPoints( const b2Vec2* points, int count )
{
	uint64_t hash = FNV_OFFSET_BASIS;
	for ( int i = 0; i < count; ++i )
	{
		hash = HashF32( hash, points[i].x );
		hash = HashF32( hash, points[i].y );
	}
	return hash;
}

// The corners of an octagon about one unit across, exact in float.
static const float OCTAGON[POINTS_PER_CLOUD][2] = {
	{ 0.5f, 0.0f },	 { 0.375f, 0.375f },   { 0.0f, 0.5f },	{ -0.375f, 0.375f },
	{ -0.5f, 0.0f }, { -0.375f, -0.375f }, { 0.0f, -0.5f }, { 0.375f, -0.375f },
};

// The point clouds of the polygon kind: eight points per cloud, each cloud
// around its own center, each point a corner of the octagon moved by up to an
// eighth of a unit on each axis, so that most hulls have six to eight points.
static b2Vec2* MakeCloudPoints( int cloudCount, uint64_t seed )
{
	b2Vec2* points = Allocate( (size_t)cloudCount * POINTS_PER_CLOUD * sizeof( b2Vec2 ) );
	SplitMix64 random;
	random.state = seed;
	for ( int cloud = 0; cloud < cloudCount; ++cloud )
	{
		float centerX = NextCentered( &random ) * 0.5f;
		float centerY = NextCentered( &random ) * 0.5f;
		for ( int k = 0; k < POINTS_PER_CLOUD; ++k )
		{
			float moveX = NextCentered( &random ) * 0.25f;
			float moveY = NextCentered( &random ) * 0.25f;
			b2Vec2* point = &points[cloud * POINTS_PER_CLOUD + k];
			point->x = ( centerX + OCTAGON[k][0] ) + moveX;
			point->y = ( centerY + OCTAGON[k][1] ) + moveY;
		}
	}
	return points;
}

// The polyline of the chain kind: pointCount points and a ghost point at each
// end (index 0 and index pointCount + 1), x growing by half a unit, y within
// plus or minus 0.3.
static b2Vec2* MakeTerrainPolyline( int pointCount, uint64_t seed )
{
	b2Vec2* points = Allocate( ( (size_t)pointCount + 2 ) * sizeof( b2Vec2 ) );
	SplitMix64 random;
	random.state = seed;
	for ( int i = 0; i < pointCount + 2; ++i )
	{
		points[i].x = 0.5f * (float)( i - 1 );
		points[i].y = NextCentered( &random ) * 0.6f;
	}
	return points;
}

// --- The polygon kind ------------------------------------------------------------

// Builds one polygon per cloud as the library asks: the hull of the cloud, then
// the polygon of the hull. A cloud whose hull the library refuses (fewer than
// three points left after welding) gives no polygon and is counted.
static int BuildPolygons( const b2Vec2* points, int cloudCount, b2Polygon* polygons, int* refusedCount )
{
	int count = 0;
	int refused = 0;
	for ( int cloud = 0; cloud < cloudCount; ++cloud )
	{
		b2Hull hull = b2ComputeHull( points + cloud * POINTS_PER_CLOUD, POINTS_PER_CLOUD );
		if ( hull.count < 3 )
		{
			refused += 1;
			continue;
		}
		polygons[count] = b2MakePolygon( &hull, 0.0f );
		count += 1;
	}
	*refusedCount = refused;
	return count;
}

// The written form of a set of polygons: the spike's own bytes, the count of
// polygons then every field of each in little-endian order. Never the memory
// of the struct, whose padding is not defined.
static void WritePolygons( Bytes* out, const b2Polygon* polygons, int count )
{
	PutU32( out, (uint32_t)count );
	for ( int i = 0; i < count; ++i )
	{
		const b2Polygon* polygon = &polygons[i];
		PutU32( out, (uint32_t)polygon->count );
		PutF32( out, polygon->radius );
		PutF32( out, polygon->centroid.x );
		PutF32( out, polygon->centroid.y );
		for ( int v = 0; v < B2_MAX_POLYGON_VERTICES; ++v )
		{
			PutF32( out, polygon->vertices[v].x );
			PutF32( out, polygon->vertices[v].y );
		}
		for ( int v = 0; v < B2_MAX_POLYGON_VERTICES; ++v )
		{
			PutF32( out, polygon->normals[v].x );
			PutF32( out, polygon->normals[v].y );
		}
	}
}

// The count of polygons a form declares, or -1 when its size does not match.
static int PolygonCountOfForm( const uint8_t* data, size_t size )
{
	Reader reader = { 0 };
	reader.data = data;
	reader.size = size;
	reader.ok = true;
	uint32_t count = TakeU32( &reader );
	if ( reader.ok == false || count > (uint32_t)INT32_MAX / POLYGON_FORM_BYTES ||
		 size != 4 + (size_t)count * POLYGON_FORM_BYTES )
	{
		return -1;
	}
	return (int)count;
}

// Reads a form into an array of at least PolygonCountOfForm polygons. A form
// may come from another machine: a vertex count the library does not take is
// refused instead of being handed to it.
static bool ReadPolygons( const uint8_t* data, size_t size, b2Polygon* polygons )
{
	Reader reader = { 0 };
	reader.data = data;
	reader.size = size;
	reader.ok = true;
	uint32_t count = TakeU32( &reader );
	for ( uint32_t i = 0; i < count && reader.ok; ++i )
	{
		b2Polygon* polygon = &polygons[i];
		uint32_t vertexCount = TakeU32( &reader );
		if ( vertexCount < 3 || vertexCount > B2_MAX_POLYGON_VERTICES )
		{
			return false;
		}
		polygon->count = (int)vertexCount;
		polygon->radius = TakeF32( &reader );
		polygon->centroid.x = TakeF32( &reader );
		polygon->centroid.y = TakeF32( &reader );
		for ( int v = 0; v < B2_MAX_POLYGON_VERTICES; ++v )
		{
			polygon->vertices[v].x = TakeF32( &reader );
			polygon->vertices[v].y = TakeF32( &reader );
		}
		for ( int v = 0; v < B2_MAX_POLYGON_VERTICES; ++v )
		{
			polygon->normals[v].x = TakeF32( &reader );
			polygon->normals[v].y = TakeF32( &reader );
		}
	}
	return reader.ok && reader.cursor == size;
}

// Equality field by field, on the bits of every float.
static bool PolygonsEqual( const b2Polygon* a, const b2Polygon* b )
{
	if ( a->count != b->count || FloatBits( a->radius ) != FloatBits( b->radius ) ||
		 FloatBits( a->centroid.x ) != FloatBits( b->centroid.x ) || FloatBits( a->centroid.y ) != FloatBits( b->centroid.y ) )
	{
		return false;
	}
	for ( int v = 0; v < B2_MAX_POLYGON_VERTICES; ++v )
	{
		if ( FloatBits( a->vertices[v].x ) != FloatBits( b->vertices[v].x ) ||
			 FloatBits( a->vertices[v].y ) != FloatBits( b->vertices[v].y ) ||
			 FloatBits( a->normals[v].x ) != FloatBits( b->normals[v].x ) ||
			 FloatBits( a->normals[v].y ) != FloatBits( b->normals[v].y ) )
		{
			return false;
		}
	}
	return true;
}

// --- Worlds and steps ------------------------------------------------------------

// The library logs on standard output by default, where the report goes.
static void LogToStandardError( const char* message )
{
	fprintf( stderr, "Box2D: %s\n", message );
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
			Fail( "out of memory" );
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

// As the Box2D snapshot continuation spike does: every shape is enumerated
// through a broadphase query covering the whole scene and reduced to its body,
// the bodies are sorted by their stored id (the creation slot comes first in
// it, and a world image preserves slots and generations, so two worlds compare
// body by body), and position and rotation are hashed field by field.
static void CollectSortedBodies( b2WorldId worldId, BodyList* list )
{
	list->count = 0;
	b2QueryFilter filter = b2DefaultQueryFilter();
	filter.categoryBits = UINT64_MAX;
	filter.maskBits = UINT64_MAX;
	b2AABB everything = { { -100000.0f, -100000.0f }, { 100000.0f, 100000.0f } };
	b2World_OverlapAABB( worldId, b2Pos_zero, everything, filter, CollectBody, list );
	qsort( list->ids, (size_t)list->count, sizeof( uint64_t ), CompareIds );
}

static uint64_t HashBodyTransforms( b2WorldId worldId, BodyList* list, int* hashedBodyCount )
{
	CollectSortedBodies( worldId, list );

	uint64_t hash = FNV_OFFSET_BASIS;
	int bodyCount = 0;
	for ( int i = 0; i < list->count; ++i )
	{
		if ( i > 0 && list->ids[i] == list->ids[i - 1] )
		{
			continue;
		}
		b2WorldTransform transform = b2Body_GetTransform( b2LoadBodyId( list->ids[i] ) );
		hash = HashF32( hash, transform.p.x );
		hash = HashF32( hash, transform.p.y );
		hash = HashF32( hash, transform.q.c );
		hash = HashF32( hash, transform.q.s );
		bodyCount += 1;
	}
	*hashedBodyCount = bodyCount;
	return hash;
}

// The height of the lowest body: far below the ground when a body fell
// through it.
static float LowestBodyHeight( b2WorldId worldId, BodyList* list )
{
	CollectSortedBodies( worldId, list );
	float lowest = 0.0f;
	for ( int i = 0; i < list->count; ++i )
	{
		float height = (float)b2Body_GetTransform( b2LoadBodyId( list->ids[i] ) ).p.y;
		lowest = i == 0 || height < lowest ? height : lowest;
	}
	return lowest;
}

static void StepAndHash( b2WorldId worldId, int steps, uint64_t* hashes, BodyList* list, int* hashedBodyCount )
{
	for ( int step = 0; step < steps; ++step )
	{
		b2World_Step( worldId, TIME_STEP, SUBSTEP_COUNT );
		hashes[step] = HashBodyTransforms( worldId, list, hashedBodyCount );
	}
}

static int FirstDifferingStep( const uint64_t* reference, const uint64_t* run, int steps )
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

// The hash of a run: FNV-1a 64 over the step hashes as little-endian bytes.
static uint64_t RunHash( const uint64_t* hashes, int steps )
{
	uint64_t hash = FNV_OFFSET_BASIS;
	for ( int step = 0; step < steps; ++step )
	{
		hash = HashU64( hash, hashes[step] );
	}
	return hash;
}

// Gravity 10 downwards (the library's default), one worker.
static b2WorldId CreateEmptyWorld( void )
{
	b2WorldDef def = b2DefaultWorldDef();
	def.workerCount = WORKER_COUNT;
	b2WorldId worldId = b2CreateWorld( &def );
	if ( b2World_IsValid( worldId ) == false )
	{
		Fail( "the world could not be created" );
	}
	return worldId;
}

static void AddGroundBox( b2WorldId worldId )
{
	b2BodyDef bodyDef = b2DefaultBodyDef();
	bodyDef.position.x = 0.0f;
	bodyDef.position.y = -1.0f;
	b2BodyId groundId = b2CreateBody( worldId, &bodyDef );
	b2ShapeDef shapeDef = b2DefaultShapeDef();
	b2Polygon box = b2MakeBox( 100.0f, 1.0f );
	b2CreatePolygonShape( groundId, &shapeDef, &box );
}

// One dynamic body per polygon, in columns two units apart around centerX and
// rows two and a half units apart, at staggered heights, each position moved
// by up to a quarter of a unit by the seeded generator.
static void AddPolygonBodies( b2WorldId worldId, const b2Polygon* polygons, int count, float centerX )
{
	SplitMix64 random;
	random.state = PLACEMENT_SEED;
	int columns = count < COLUMNS ? count : COLUMNS;
	b2ShapeDef shapeDef = b2DefaultShapeDef();
	for ( int i = 0; i < count; ++i )
	{
		int column = i % columns;
		int row = i / columns;
		float offsetX = 2.0f * (float)column - (float)( columns - 1 );
		float height = 3.0f + 2.5f * (float)row + 0.5f * (float)( column % 3 );
		float moveX = NextCentered( &random ) * 0.5f;
		float moveY = NextCentered( &random ) * 0.5f;
		b2BodyDef bodyDef = b2DefaultBodyDef();
		bodyDef.type = b2_dynamicBody;
		bodyDef.position.x = ( centerX + offsetX ) + moveX;
		bodyDef.position.y = height + moveY;
		b2BodyId bodyId = b2CreateBody( worldId, &bodyDef );
		b2CreatePolygonShape( bodyId, &shapeDef, &polygons[i] );
	}
}

// The input of a chain: the polyline walked from its highest x to its lowest,
// because a chain is one-sided and its solid side is on the right of the walk,
// with the ghost point that precedes the first point and the one that follows
// the last.
typedef struct ChainInput
{
	b2Vec2* points;
	int pointCount;
	b2Vec2 ghost1;
	b2Vec2 ghost2;
	uint64_t inputHash;
} ChainInput;

static ChainInput MakeChainInput( int pointCount )
{
	b2Vec2* polyline = MakeTerrainPolyline( pointCount, CHAIN_SEED + (uint64_t)pointCount );
	ChainInput input;
	input.pointCount = pointCount;
	input.inputHash = HashPoints( polyline, pointCount + 2 );
	input.points = Allocate( (size_t)pointCount * sizeof( b2Vec2 ) );
	for ( int i = 0; i < pointCount; ++i )
	{
		input.points[i] = polyline[pointCount - i];
	}
	input.ghost1 = polyline[pointCount + 1];
	input.ghost2 = polyline[0];
	free( polyline );
	return input;
}

// Creates a static body and the chain on it, and times b2CreateChain alone.
static b2ChainId CreateChainOnStaticBody( b2WorldId worldId, const ChainInput* input, float* createMs )
{
	b2BodyDef bodyDef = b2DefaultBodyDef();
	b2BodyId bodyId = b2CreateBody( worldId, &bodyDef );
	b2ChainDef chainDef = b2DefaultChainDef();
	chainDef.points = input->points;
	chainDef.pointCount = input->pointCount;
	chainDef.ghost1 = input->ghost1;
	chainDef.ghost2 = input->ghost2;
	chainDef.isLoop = false;
	uint64_t ticks = b2GetTicks();
	b2ChainId chainId = b2CreateChain( bodyId, &chainDef );
	*createMs = b2GetMilliseconds( ticks );
	if ( b2Chain_IsValid( chainId ) == false )
	{
		Fail( "the chain could not be created" );
	}
	return chainId;
}

// The image of a whole world, the size query and the write timed together as
// the Box2D snapshot continuation spike times them.
static void TakeWorldImage( b2WorldId worldId, Bytes* image, float* takeMs )
{
	FreeBytes( image );
	uint64_t ticks = b2GetTicks();
	int size = b2World_GetSnapshot( worldId, NULL, 0 );
	if ( size <= 0 )
	{
		Fail( "the world image size query failed" );
	}
	Reserve( image, (size_t)size );
	int written = b2World_GetSnapshot( worldId, image->data, size );
	*takeMs = b2GetMilliseconds( ticks );
	if ( written != size )
	{
		Fail( "the world image was not written in full" );
	}
	image->count = (size_t)size;
}

// --- What is measured --------------------------------------------------------------

typedef struct Options
{
	const char* cell;
	const char* formsOut;			 // NULL: the forms are not written
	const char* otherCellsDirectory; // NULL: no form from another cell
	const char* otherCells[MAX_OTHER_CELLS];
	int otherCellCount;
	bool quick;
	int steps;
	int ladder[LADDER_COUNT];
} Options;

typedef struct LadderRow
{
	int elements;
	uint64_t inputHash;
	int built;	 // polygons, or chain segments
	int refused; // clouds whose hull the library refused
	Timing build;
	size_t formBytes;
	uint64_t formHash;
	Timing read;
	bool sameBytesTwice;
	bool sameBytesAfterRoundTrip;
	bool readBackEqual; // field by field
} LadderRow;

typedef struct ImageRow
{
	int elements;
	size_t imageBytes;
	uint64_t imageHash;
	Timing take;
	Timing create;
	bool createSucceeded;
	bool sameBytesTwice;
} ImageRow;

typedef struct SourceRow
{
	char source[160];
	bool readSucceeded; // the form was read, or the world was created from the image
	bool bytesEqualLocal;
	int64_t bytesDiffering; // from the local form; -1 when the sizes differ or nothing was read
	int firstDifferingStep;
	uint64_t runHash;
} SourceRow;

typedef struct Behavior
{
	int steps;
	int bodies;
	SourceRow sources[MAX_SOURCES];
	int sourceCount;
} Behavior;

typedef struct WrittenFile
{
	bool written;
	size_t bytes;
} WrittenFile;

typedef struct PolygonKind
{
	int hullOfTheWholeInput[LADDER_COUNT]; // hull points returned for the input given as one cloud
	int hullOfEightPoints;
	bool everyFormReadsBackEqual;
	LadderRow ladder[LADDER_COUNT];
	Behavior behavior;
	WrittenFile file;
} PolygonKind;

typedef struct ChainKind
{
	LadderRow ladder[LADDER_COUNT];
	ImageRow images[LADDER_COUNT];
	Behavior behavior;
	WrittenFile file;
} ChainKind;

// The scratch memory of the step loops.
typedef struct Scratch
{
	BodyList list;
	uint64_t* reference;
	uint64_t* hashes;
} Scratch;

static SourceRow* AddSource( Behavior* behavior, const char* prefix, const char* cell )
{
	if ( behavior->sourceCount == MAX_SOURCES )
	{
		Fail( "too many sources" );
	}
	SourceRow* row = &behavior->sources[behavior->sourceCount];
	behavior->sourceCount += 1;
	memset( row, 0, sizeof( *row ) );
	snprintf( row->source, sizeof( row->source ), "%s%s", prefix, cell );
	row->firstDifferingStep = -1;
	row->bytesDiffering = -1;
	return row;
}

// Writes a form under the forms directory when one was given.
static WrittenFile WriteForm( const Options* options, const char* name, const Bytes* form )
{
	WrittenFile file = { 0 };
	file.bytes = form->count;
	if ( options->formsOut != NULL )
	{
		char path[PATH_CAPACITY];
		JoinPath( path, options->formsOut, NULL, name );
		file.written = WriteBytesToFile( path, form->data, form->count );
		if ( file.written == false )
		{
			fprintf( stderr, "%s could not be written\n", path );
		}
	}
	return file;
}

// The bytes a world of this run reads: the file just written when there is
// one, so that the form goes through the file system as another cell's does,
// the bytes in memory otherwise.
static bool ReadOwnForm( const Options* options, const char* name, const WrittenFile* file, const Bytes* inMemory, Bytes* out )
{
	if ( file->written == false )
	{
		PutBytes( out, inMemory->data, inMemory->count );
		return true;
	}
	char path[PATH_CAPACITY];
	JoinPath( path, options->formsOut, NULL, name );
	return ReadBytesFromFile( path, out );
}

// --- The polygon kind, measured ------------------------------------------------------

static void MeasurePolygonLadder( const Options* options, PolygonKind* kind, b2Polygon** smallestPolygons, int* smallestCount,
								  Bytes* smallestForm )
{
	float samples[MAX_REPETITIONS];
	kind->everyFormReadsBackEqual = true;
	for ( int s = 0; s < LADDER_COUNT; ++s )
	{
		LadderRow* row = &kind->ladder[s];
		memset( row, 0, sizeof( *row ) );
		row->elements = options->ladder[s];
		int cloudCount = row->elements / POINTS_PER_CLOUD;
		int pointCount = cloudCount * POINTS_PER_CLOUD;
		b2Vec2* points = MakeCloudPoints( cloudCount, CLOUD_SEED + (uint64_t)row->elements );
		row->inputHash = HashPoints( points, pointCount );

		// The first step: the library's hull function on the whole input as one
		// cloud, then on the eight points of the first cloud.
		kind->hullOfTheWholeInput[s] = b2ComputeHull( points, pointCount ).count;
		if ( s == 0 )
		{
			kind->hullOfEightPoints = b2ComputeHull( points, POINTS_PER_CLOUD ).count;
		}

		b2Polygon* polygons = Allocate( (size_t)cloudCount * sizeof( b2Polygon ) );
		b2Polygon* others = Allocate( (size_t)cloudCount * sizeof( b2Polygon ) );
		for ( int repetition = 0; repetition < REPETITIONS; ++repetition )
		{
			uint64_t ticks = b2GetTicks();
			row->built = BuildPolygons( points, cloudCount, polygons, &row->refused );
			samples[repetition] = b2GetMilliseconds( ticks );
		}
		row->build = SummarizeSamples( samples, REPETITIONS );

		Bytes form = { 0 };
		WritePolygons( &form, polygons, row->built );
		row->formBytes = form.count;
		row->formHash = HashBytes( FNV_OFFSET_BASIS, form.data, form.count );

		// A second build in the same process must give the same bytes.
		int otherRefused = 0;
		int otherCount = BuildPolygons( points, cloudCount, others, &otherRefused );
		Bytes second = { 0 };
		WritePolygons( &second, others, otherCount );
		row->sameBytesTwice = SameBytes( form.data, form.count, second.data, second.count );
		FreeBytes( &second );

		// The form read back into an array of polygons.
		bool readSucceeded = PolygonCountOfForm( form.data, form.count ) == row->built;
		memset( others, 0, (size_t)cloudCount * sizeof( b2Polygon ) );
		for ( int repetition = 0; repetition < REPETITIONS && readSucceeded; ++repetition )
		{
			uint64_t ticks = b2GetTicks();
			readSucceeded = ReadPolygons( form.data, form.count, others );
			samples[repetition] = b2GetMilliseconds( ticks );
		}
		if ( readSucceeded )
		{
			row->read = SummarizeSamples( samples, REPETITIONS );
			Bytes rewritten = { 0 };
			WritePolygons( &rewritten, others, row->built );
			row->sameBytesAfterRoundTrip = SameBytes( form.data, form.count, rewritten.data, rewritten.count );
			FreeBytes( &rewritten );
			row->readBackEqual = true;
			for ( int i = 0; i < row->built; ++i )
			{
				row->readBackEqual = row->readBackEqual && PolygonsEqual( &polygons[i], &others[i] );
			}
		}
		kind->everyFormReadsBackEqual = kind->everyFormReadsBackEqual && row->readBackEqual && row->sameBytesAfterRoundTrip;

		fprintf( stderr, "box2d convex_hull %d points: %d polygons, build %.4f ms, read %.4f ms, %zu bytes\n", row->elements,
				 row->built, row->build.median, row->read.median, row->formBytes );

		free( others );
		free( points );
		if ( s == 0 )
		{
			*smallestPolygons = polygons;
			*smallestCount = row->built;
			*smallestForm = form;
		}
		else
		{
			free( polygons );
			FreeBytes( &form );
		}
	}
}

// Steps a world whose polygons were read from a form and compares it with the
// world that built them.
static void RunPolygonSource( SourceRow* row, const Bytes* form, const Bytes* localForm, int steps, Scratch* scratch )
{
	row->bytesEqualLocal = SameBytes( form->data, form->count, localForm->data, localForm->count );
	row->bytesDiffering = CountDifferingBytes( form->data, form->count, localForm->data, localForm->count );
	int count = PolygonCountOfForm( form->data, form->count );
	if ( count < 0 )
	{
		return;
	}
	b2Polygon* polygons = Allocate( (size_t)count * sizeof( b2Polygon ) );
	if ( ReadPolygons( form->data, form->count, polygons ) )
	{
		row->readSucceeded = true;
		b2WorldId worldId = CreateEmptyWorld();
		AddGroundBox( worldId );
		AddPolygonBodies( worldId, polygons, count, 0.0f );
		int hashedBodies = 0;
		StepAndHash( worldId, steps, scratch->hashes, &scratch->list, &hashedBodies );
		row->firstDifferingStep = FirstDifferingStep( scratch->reference, scratch->hashes, steps );
		row->runHash = RunHash( scratch->hashes, steps );
		b2DestroyWorld( worldId );
	}
	free( polygons );
}

static void MeasurePolygonBehavior( const Options* options, PolygonKind* kind, const b2Polygon* polygons, int count,
									const Bytes* localForm, Scratch* scratch )
{
	Behavior* behavior = &kind->behavior;
	behavior->steps = options->steps;

	b2WorldId worldId = CreateEmptyWorld();
	AddGroundBox( worldId );
	AddPolygonBodies( worldId, polygons, count, 0.0f );
	StepAndHash( worldId, options->steps, scratch->reference, &scratch->list, &behavior->bodies );
	b2DestroyWorld( worldId );
	SourceRow* local = AddSource( behavior, "built_locally", "" );
	local->readSucceeded = true;
	local->bytesEqualLocal = true;
	local->bytesDiffering = 0;
	local->runHash = RunHash( scratch->reference, options->steps );

	SourceRow* own = AddSource( behavior, "read_from_this_run", "" );
	Bytes form = { 0 };
	if ( ReadOwnForm( options, POLYGON_FILE, &kind->file, localForm, &form ) )
	{
		RunPolygonSource( own, &form, localForm, options->steps, scratch );
	}
	FreeBytes( &form );

	for ( int c = 0; c < options->otherCellCount; ++c )
	{
		SourceRow* other = AddSource( behavior, "read_from_", options->otherCells[c] );
		char path[PATH_CAPACITY];
		JoinPath( path, options->otherCellsDirectory, options->otherCells[c], POLYGON_FILE );
		if ( ReadBytesFromFile( path, &form ) )
		{
			RunPolygonSource( other, &form, localForm, options->steps, scratch );
		}
		else
		{
			fprintf( stderr, "%s could not be read\n", path );
		}
		FreeBytes( &form );
	}

	for ( int i = 0; i < behavior->sourceCount; ++i )
	{
		fprintf( stderr, "box2d convex_hull world, %s: first differing step %d\n", behavior->sources[i].source,
				 behavior->sources[i].firstDifferingStep );
	}
}

// --- The chain kind, measured ----------------------------------------------------------

static b2WorldId CreateChainWorld( const ChainInput* input )
{
	b2WorldId worldId = CreateEmptyWorld();
	float createMs = 0.0f;
	CreateChainOnStaticBody( worldId, input, &createMs );
	return worldId;
}

static void MeasureChainLadder( const Options* options, ChainKind* kind, ChainInput* smallestInput, Bytes* smallestImage )
{
	float samples[MAX_REPETITIONS];
	for ( int s = 0; s < LADDER_COUNT; ++s )
	{
		LadderRow* row = &kind->ladder[s];
		memset( row, 0, sizeof( *row ) );
		row->elements = options->ladder[s];
		ChainInput input = MakeChainInput( row->elements );
		row->inputHash = input.inputHash;

		// The build at load: b2CreateChain alone, on a static body of a new world
		// each time, the creation of the world outside the timed region.
		for ( int repetition = 0; repetition < REPETITIONS; ++repetition )
		{
			b2WorldId worldId = CreateEmptyWorld();
			b2ChainId chainId = CreateChainOnStaticBody( worldId, &input, &samples[repetition] );
			row->built = b2Chain_GetSegmentCount( chainId );
			b2DestroyWorld( worldId );
		}
		row->build = SummarizeSamples( samples, REPETITIONS );

		// Beside it, the one written form the library has that holds a built
		// chain: the image of a whole world, here a world that holds only the
		// static body and its chain.
		ImageRow* image = &kind->images[s];
		memset( image, 0, sizeof( *image ) );
		image->elements = row->elements;
		b2WorldId worldId = CreateChainWorld( &input );
		Bytes bytes = { 0 };
		for ( int repetition = 0; repetition < REPETITIONS; ++repetition )
		{
			TakeWorldImage( worldId, &bytes, &samples[repetition] );
		}
		image->take = SummarizeSamples( samples, REPETITIONS );
		image->imageBytes = bytes.count;
		image->imageHash = HashBytes( FNV_OFFSET_BASIS, bytes.data, bytes.count );
		b2DestroyWorld( worldId );

		// A second world built the same way must give the same image bytes.
		b2WorldId otherId = CreateChainWorld( &input );
		Bytes second = { 0 };
		float unused = 0.0f;
		TakeWorldImage( otherId, &second, &unused );
		image->sameBytesTwice = SameBytes( bytes.data, bytes.count, second.data, second.count );
		FreeBytes( &second );
		b2DestroyWorld( otherId );

		image->createSucceeded = true;
		for ( int repetition = 0; repetition < REPETITIONS; ++repetition )
		{
			uint64_t ticks = b2GetTicks();
			b2WorldId createdId = b2CreateWorldFromSnapshot( bytes.data, (int)bytes.count, WORKER_COUNT );
			samples[repetition] = b2GetMilliseconds( ticks );
			if ( b2World_IsValid( createdId ) )
			{
				b2DestroyWorld( createdId );
			}
			else
			{
				image->createSucceeded = false;
			}
		}
		image->create = SummarizeSamples( samples, REPETITIONS );

		fprintf( stderr, "box2d chain %d points: create %.4f ms; world image %zu bytes, take %.4f ms, create a world from it %.4f ms\n",
				 row->elements, row->build.median, image->imageBytes, image->take.median, image->create.median );

		if ( s == 0 )
		{
			*smallestInput = input;
			*smallestImage = bytes;
		}
		else
		{
			free( input.points );
			FreeBytes( &bytes );
		}
	}
}

// Steps a world created from an image, to which the bodies are then added,
// and compares it with the world whose chain was created locally.
static void RunChainImageSource( SourceRow* row, const Bytes* image, const Bytes* localImage, const b2Polygon* polygons, int count,
								 float centerX, int steps, Scratch* scratch )
{
	row->bytesEqualLocal = SameBytes( image->data, image->count, localImage->data, localImage->count );
	row->bytesDiffering = CountDifferingBytes( image->data, image->count, localImage->data, localImage->count );
	if ( image->count == 0 || image->count > (size_t)INT32_MAX )
	{
		return;
	}
	b2WorldId worldId = b2CreateWorldFromSnapshot( image->data, (int)image->count, WORKER_COUNT );
	if ( b2World_IsValid( worldId ) == false )
	{
		return;
	}
	row->readSucceeded = true;
	AddPolygonBodies( worldId, polygons, count, centerX );
	int hashedBodies = 0;
	StepAndHash( worldId, steps, scratch->hashes, &scratch->list, &hashedBodies );
	row->firstDifferingStep = FirstDifferingStep( scratch->reference, scratch->hashes, steps );
	row->runHash = RunHash( scratch->hashes, steps );
	b2DestroyWorld( worldId );
}

static void MeasureChainBehavior( const Options* options, ChainKind* kind, const ChainInput* input, const Bytes* localImage,
								  const b2Polygon* polygons, int count, Scratch* scratch )
{
	Behavior* behavior = &kind->behavior;
	behavior->steps = options->steps;
	// The bodies are dropped over the middle of the polyline.
	float centerX = 0.25f * (float)( input->pointCount - 1 );

	b2WorldId worldId = CreateChainWorld( input );
	AddPolygonBodies( worldId, polygons, count, centerX );
	StepAndHash( worldId, options->steps, scratch->reference, &scratch->list, &behavior->bodies );
	b2DestroyWorld( worldId );
	SourceRow* local = AddSource( behavior, "created_locally", "" );
	local->readSucceeded = true;
	local->bytesEqualLocal = true;
	local->bytesDiffering = 0;
	local->runHash = RunHash( scratch->reference, options->steps );

	SourceRow* own = AddSource( behavior, "created_from_the_image_of_this_run", "" );
	Bytes image = { 0 };
	if ( ReadOwnForm( options, CHAIN_IMAGE_FILE, &kind->file, localImage, &image ) )
	{
		RunChainImageSource( own, &image, localImage, polygons, count, centerX, options->steps, scratch );
	}
	FreeBytes( &image );

	for ( int c = 0; c < options->otherCellCount; ++c )
	{
		SourceRow* other = AddSource( behavior, "created_from_the_image_of_", options->otherCells[c] );
		char path[PATH_CAPACITY];
		JoinPath( path, options->otherCellsDirectory, options->otherCells[c], CHAIN_IMAGE_FILE );
		if ( ReadBytesFromFile( path, &image ) )
		{
			RunChainImageSource( other, &image, localImage, polygons, count, centerX, options->steps, scratch );
		}
		else
		{
			fprintf( stderr, "%s could not be read\n", path );
		}
		FreeBytes( &image );
	}

	for ( int i = 0; i < behavior->sourceCount; ++i )
	{
		fprintf( stderr, "box2d chain world, %s: first differing step %d\n", behavior->sources[i].source,
				 behavior->sources[i].firstDifferingStep );
	}
}

// --- The report -------------------------------------------------------------------------

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
		putchar( (unsigned char)*c < 0x20 ? ' ' : *c );
	}
	putchar( '"' );
}

static const char* Boolean( bool value )
{
	return value ? "true" : "false";
}

static void PrintTiming( const char* prefix, const Timing* timing )
{
	printf( "\"%s_ms_median\":%.4f,\"%s_ms_fastest\":%.4f,\"%s_ms_slowest\":%.4f", prefix, timing->median, prefix, timing->fastest,
			prefix, timing->slowest );
}

static void PrintBehavior( const Behavior* behavior, const Options* options, const char* bytesKey, const char* readKey )
{
	printf( "\"behavior\":{\"steps\":%d,\"bodies\":%d,\"forms_from_other_cells_present\":%s,\"sources\":[", behavior->steps,
			behavior->bodies, Boolean( options->otherCellCount > 0 ) );
	for ( int i = 0; i < behavior->sourceCount; ++i )
	{
		const SourceRow* row = &behavior->sources[i];
		printf( "%s{\"source\":", i == 0 ? "" : "," );
		PrintJsonString( row->source );
		printf( ",\"%s\":%s,\"%s\":%s,\"bytes_differing_from_local\":%" PRId64
				",\"first_differing_step\":%d,\"run_hash\":\"0x%016" PRIx64 "\"}",
				readKey, Boolean( row->readSucceeded ), bytesKey, Boolean( row->bytesEqualLocal ), row->bytesDiffering,
				row->firstDifferingStep, row->runHash );
	}
	printf( "]}" );
}

static void PrintPolygonKind( const PolygonKind* kind, const Options* options )
{
	printf( "{\"kind\":\"convex_hull\",\"variant\":\"\",\"prepared_form\":" );
	PrintJsonString( kind->everyFormReadsBackEqual ? "yes, as the application's own bytes of a plain value"
												   : "no: the written polygons did not read back equal" );
	printf( ",\"how\":" );
	PrintJsonString( "b2ComputeHull takes at most B2_MAX_POLYGON_VERTICES points and b2MakePolygon makes a b2Polygon of the hull, "
					 "a plain value built with no world. The library has no function that writes or reads one, so the form is "
					 "the spike's own bytes of its fields, written field by field in little-endian order." );
	printf( ",\"largest_cloud_the_library_takes\":%d,\"hull_points_returned_for_eight_points\":%d,"
			"\"hull_points_returned_for_the_whole_input\":[",
			B2_MAX_POLYGON_VERTICES, kind->hullOfEightPoints );
	for ( int s = 0; s < LADDER_COUNT; ++s )
	{
		printf( "%s{\"points\":%d,\"hull_points\":%d}", s == 0 ? "" : ",",
				( kind->ladder[s].elements / POINTS_PER_CLOUD ) * POINTS_PER_CLOUD, kind->hullOfTheWholeInput[s] );
	}
	printf( "],\"the_ladder_as_the_library_takes_it\":" );
	PrintJsonString( "N input points are N/8 clouds of 8 points, one polygon per cloud" );
	printf( ",\"not_measured\":" );
	PrintJsonString( "the reduction of a cloud above eight points to a polygon the library takes, which is the importer's own "
					 "computation" );
	printf( ",\"form_file\":\"%s\",\"form_file_bytes\":%zu,\"form_file_written\":%s,\"ladder\":[", POLYGON_FILE, kind->file.bytes,
			Boolean( kind->file.written ) );
	for ( int s = 0; s < LADDER_COUNT; ++s )
	{
		const LadderRow* row = &kind->ladder[s];
		printf( "%s{\"elements\":%d,\"input_hash\":\"0x%016" PRIx64 "\",\"built\":%d,\"clouds_refused\":%d,", s == 0 ? "" : ",",
				row->elements, row->inputHash, row->built, row->refused );
		PrintTiming( "build", &row->build );
		printf( ",\"build_over_fixed_step\":%.4f,\"form_bytes\":%zu,\"form_hash\":\"0x%016" PRIx64 "\",",
				row->build.median / FIXED_STEP_MS, row->formBytes, row->formHash );
		PrintTiming( "read", &row->read );
		printf( ",\"same_bytes_twice\":%s,\"same_bytes_after_a_round_trip\":%s,\"read_back_equal_field_by_field\":%s}",
				Boolean( row->sameBytesTwice ), Boolean( row->sameBytesAfterRoundTrip ), Boolean( row->readBackEqual ) );
	}
	printf( "]," );
	PrintBehavior( &kind->behavior, options, "form_bytes_equal_local", "read_succeeded" );
	printf( "}" );
}

static void PrintChainKind( const ChainKind* kind, const Options* options )
{
	printf( "{\"kind\":\"chain\",\"variant\":\"\",\"prepared_form\":\"none\",\"how\":" );
	PrintJsonString( "A chain exists only as chain segment shapes created on a body of a world by b2CreateChain or "
					 "b2CreateChainSegmentShape; the headers and src/shape.c hold no function that builds a chain outside a world, "
					 "and none that writes or reads a chain or a segment. The build is an action at load on each machine. The one "
					 "written form that holds a built chain is the image of a whole world, measured beside it as such." );
	printf( ",\"ladder\":[" );
	for ( int s = 0; s < LADDER_COUNT; ++s )
	{
		const LadderRow* row = &kind->ladder[s];
		printf( "%s{\"elements\":%d,\"input_hash\":\"0x%016" PRIx64 "\",\"built\":%d,", s == 0 ? "" : ",", row->elements,
				row->inputHash, row->built );
		PrintTiming( "build", &row->build );
		printf( ",\"build_over_fixed_step\":%.4f}", row->build.median / FIXED_STEP_MS );
	}
	printf( "],\"world_image_is\":" );
	PrintJsonString( "the image of a whole world that holds only a static body and its chain (b2World_GetSnapshot, "
					 "b2CreateWorldFromSnapshot): not a prepared form of the kind" );
	printf( ",\"world_image_file\":\"%s\",\"world_image_file_bytes\":%zu,\"world_image_file_written\":%s,\"world_image\":[",
			CHAIN_IMAGE_FILE, kind->file.bytes, Boolean( kind->file.written ) );
	for ( int s = 0; s < LADDER_COUNT; ++s )
	{
		const ImageRow* image = &kind->images[s];
		printf( "%s{\"elements\":%d,\"image_bytes\":%zu,\"image_hash\":\"0x%016" PRIx64 "\",", s == 0 ? "" : ",", image->elements,
				image->imageBytes, image->imageHash );
		PrintTiming( "take", &image->take );
		printf( ",\"take_over_fixed_step\":%.4f,", image->take.median / FIXED_STEP_MS );
		PrintTiming( "create_world", &image->create );
		printf( ",\"create_world_over_fixed_step\":%.4f,\"create_world_succeeded\":%s,\"same_bytes_twice\":%s}",
				image->create.median / FIXED_STEP_MS, Boolean( image->createSucceeded ), Boolean( image->sameBytesTwice ) );
	}
	printf( "]," );
	PrintBehavior( &kind->behavior, options, "image_bytes_equal_local", "restore_succeeded" );
	printf( "}" );
}

static void PrintReport( const Options* options, const PolygonKind* polygonKind, const ChainKind* chainKind )
{
	b2Version version = b2GetVersion();
	printf( "{\"ran\":true,\"schema_version\":%d,\"cell\":", SCHEMA_VERSION );
	PrintJsonString( options->cell );
	printf( ",\"version\":\"%d.%d.%d\",\"git_tag\":\"v3.2.0\",\"license\":\"MIT\",\"compiler\":", version.major, version.minor,
			version.revision );
	PrintCompiler();
#if defined( NDEBUG )
	printf( ",\"profile\":\"release\"" );
#else
	printf( ",\"profile\":\"dev\"" );
#endif
	printf( ",\"quick\":%s,\"repetitions\":%d,\"double_precision\":%s,\"avx2_available\":%s,", Boolean( options->quick ), REPETITIONS,
			Boolean( b2IsDoublePrecision() ), Boolean( b2IsAVX2Available() ) );
	printf( "\"platform\":{\"os\":\"%s\",\"arch\":\"%s\"},", OperatingSystem(), Architecture() );
	printf( "\"settings\":{\"time_step\":%.9g,\"substeps\":%d,\"workers\":%d,\"steps\":%d},", (double)TIME_STEP, SUBSTEP_COUNT,
			WORKER_COUNT, options->steps );
	printf( "\"forms_from_other_cells\":[" );
	for ( int c = 0; c < options->otherCellCount; ++c )
	{
		printf( "%s{\"cell\":", c == 0 ? "" : "," );
		PrintJsonString( options->otherCells[c] );
		printf( "}" );
	}
	printf( "],\"kinds\":[" );
	PrintPolygonKind( polygonKind, options );
	printf( "," );
	PrintChainKind( chainKind, options );
	printf( "]}\n" );
}

// --- Self-check: the driver's own logic, not the hypotheses ----------------------------

static int SelfCheck( void )
{
	if ( HashBytes( FNV_OFFSET_BASIS, "", 0 ) != 0xcbf29ce484222325ULL ||
		 HashBytes( FNV_OFFSET_BASIS, "a", 1 ) != 0xaf63dc4c8601ec8cULL )
	{
		fprintf( stderr, "self-check: FNV-1a vectors failed\n" );
		return 1;
	}

	// The published first outputs of SplitMix64 from a zero seed.
	SplitMix64 random;
	random.state = 0;
	if ( NextU64( &random ) != 0xe220a8397b1dcdafULL || NextU64( &random ) != 0x6e789e6aa1b965f4ULL ||
		 NextU64( &random ) != 0x06c45d188009454fULL )
	{
		fprintf( stderr, "self-check: SplitMix64 vectors failed\n" );
		return 1;
	}

	// A polygon written and read back is equal field by field, and the bytes
	// written again are the same.
	enum { CLOUDS = 16 };
	b2Vec2* points = MakeCloudPoints( CLOUDS, CLOUD_SEED );
	b2Polygon polygons[CLOUDS];
	b2Polygon readBack[CLOUDS];
	int refused = 0;
	int count = BuildPolygons( points, CLOUDS, polygons, &refused );
	Bytes form = { 0 };
	WritePolygons( &form, polygons, count );
	memset( readBack, 0, sizeof( readBack ) );
	if ( count == 0 || form.count != 4 + (size_t)count * POLYGON_FORM_BYTES || PolygonCountOfForm( form.data, form.count ) != count ||
		 ReadPolygons( form.data, form.count, readBack ) == false )
	{
		fprintf( stderr, "self-check: the polygons were not written or read\n" );
		return 1;
	}
	for ( int i = 0; i < count; ++i )
	{
		if ( PolygonsEqual( &polygons[i], &readBack[i] ) == false )
		{
			fprintf( stderr, "self-check: polygon %d read back differs\n", i );
			return 1;
		}
	}
	Bytes rewritten = { 0 };
	WritePolygons( &rewritten, readBack, count );
	if ( SameBytes( form.data, form.count, rewritten.data, rewritten.count ) == false )
	{
		fprintf( stderr, "self-check: the bytes written again differ\n" );
		return 1;
	}

	// A truncated form and a vertex count the library does not take are refused.
	form.data[4] = 9;
	if ( PolygonCountOfForm( form.data, form.count - 1 ) != -1 || ReadPolygons( form.data, form.count, readBack ) )
	{
		fprintf( stderr, "self-check: a damaged form was accepted\n" );
		return 1;
	}

	// The library refuses a cloud above its limit and takes eight points.
	if ( b2ComputeHull( points, CLOUDS * POINTS_PER_CLOUD ).count != 0 || b2ComputeHull( points, POINTS_PER_CLOUD ).count < 3 )
	{
		fprintf( stderr, "self-check: the hull function did not behave as read in its source\n" );
		return 1;
	}

	// Two worlds built the same way step to the same hashes, a changed polygon
	// is seen once the bodies have landed, and a world created from the image
	// of a chain world holds the chain's body.
	enum { CHECK_STEPS = 120 };
	uint64_t a[CHECK_STEPS];
	uint64_t b[CHECK_STEPS];
	BodyList list = { 0 };
	int hashedA = 0;
	int hashedB = 0;
	b2WorldId worldA = CreateEmptyWorld();
	AddGroundBox( worldA );
	AddPolygonBodies( worldA, polygons, count, 0.0f );
	StepAndHash( worldA, CHECK_STEPS, a, &list, &hashedA );
	b2WorldId worldB = CreateEmptyWorld();
	AddGroundBox( worldB );
	AddPolygonBodies( worldB, polygons, count, 0.0f );
	StepAndHash( worldB, CHECK_STEPS, b, &list, &hashedB );
	if ( hashedA != count + 1 || hashedB != hashedA || FirstDifferingStep( a, b, CHECK_STEPS ) != -1 )
	{
		fprintf( stderr, "self-check: two worlds built the same way differ, or a body was not hashed\n" );
		return 1;
	}
	b2DestroyWorld( worldB );
	polygons[0] = b2MakeBox( 0.25f, 0.25f );
	worldB = CreateEmptyWorld();
	AddGroundBox( worldB );
	AddPolygonBodies( worldB, polygons, count, 0.0f );
	StepAndHash( worldB, CHECK_STEPS, b, &list, &hashedB );
	if ( FirstDifferingStep( a, b, CHECK_STEPS ) == -1 )
	{
		fprintf( stderr, "self-check: a changed polygon was not seen in the hashes\n" );
		return 1;
	}
	b2DestroyWorld( worldA );
	b2DestroyWorld( worldB );

	ChainInput input = MakeChainInput( 64 );
	b2WorldId chainWorld = CreateChainWorld( &input );
	Bytes image = { 0 };
	float takeMs = 0.0f;
	TakeWorldImage( chainWorld, &image, &takeMs );
	b2WorldId fromImage = b2CreateWorldFromSnapshot( image.data, (int)image.count, WORKER_COUNT );
	if ( b2World_IsValid( fromImage ) == false )
	{
		fprintf( stderr, "self-check: no world was created from the image of a chain world\n" );
		return 1;
	}
	HashBodyTransforms( fromImage, &list, &hashedA );
	if ( hashedA != 1 || b2World_GetCounters( fromImage ).shapeCount != 63 )
	{
		fprintf( stderr, "self-check: the world created from the image does not hold the chain\n" );
		return 1;
	}
	b2DestroyWorld( fromImage );
	b2DestroyWorld( chainWorld );

	// The chain and the ground box face upwards: the bodies dropped onto them
	// rest on them, where they would be far below after four seconds.
	enum { GROUND_POINTS = 200, RESTING_STEPS = 240 };
	ChainInput ground = MakeChainInput( GROUND_POINTS );
	b2WorldId onChain = CreateChainWorld( &ground );
	AddPolygonBodies( onChain, polygons, count, 0.25f * (float)( GROUND_POINTS - 1 ) );
	b2WorldId onBox = CreateEmptyWorld();
	AddGroundBox( onBox );
	AddPolygonBodies( onBox, polygons, count, 0.0f );
	for ( int step = 0; step < RESTING_STEPS; ++step )
	{
		b2World_Step( onChain, TIME_STEP, SUBSTEP_COUNT );
		b2World_Step( onBox, TIME_STEP, SUBSTEP_COUNT );
	}
	if ( LowestBodyHeight( onChain, &list ) < -2.0f || LowestBodyHeight( onBox, &list ) < -2.0f )
	{
		fprintf( stderr, "self-check: a body fell through the chain or the ground\n" );
		return 1;
	}
	b2DestroyWorld( onChain );
	b2DestroyWorld( onBox );
	free( ground.points );

	FreeBytes( &image );
	FreeBytes( &rewritten );
	FreeBytes( &form );
	free( input.points );
	free( points );
	free( list.ids );
	fprintf( stderr, "self-check passed\n" );
	return 0;
}

// --- Main -------------------------------------------------------------------------------

int main( int argc, char** argv )
{
	b2SetLogCallback( LogToStandardError );

	Options options = { 0 };
	options.cell = "unspecified";
	bool selfCheck = false;
	int positional = 0;
	for ( int i = 1; i < argc; ++i )
	{
		if ( strcmp( argv[i], "--self-check" ) == 0 )
		{
			selfCheck = true;
		}
		else if ( strcmp( argv[i], "--quick" ) == 0 )
		{
			options.quick = true;
		}
		else if ( positional == 0 )
		{
			options.cell = argv[i];
			positional += 1;
		}
		else if ( positional == 1 )
		{
			options.formsOut = argv[i];
			positional += 1;
		}
		else if ( positional == 2 )
		{
			options.otherCellsDirectory = argv[i];
			positional += 1;
		}
		else if ( options.otherCellCount < MAX_OTHER_CELLS )
		{
			options.otherCells[options.otherCellCount] = argv[i];
			options.otherCellCount += 1;
		}
		else
		{
			fprintf( stderr, "more than %d cell directories: %s is ignored\n", MAX_OTHER_CELLS, argv[i] );
		}
	}
	if ( selfCheck )
	{
		return SelfCheck();
	}

	// A quick run divides the ladder and the steps by ten.
	int divisor = options.quick ? 10 : 1;
	options.steps = STEPS / divisor;
	for ( int s = 0; s < LADDER_COUNT; ++s )
	{
		options.ladder[s] = LADDER[s] / divisor;
	}

	Scratch scratch = { 0 };
	scratch.reference = Allocate( (size_t)options.steps * sizeof( uint64_t ) );
	scratch.hashes = Allocate( (size_t)options.steps * sizeof( uint64_t ) );

	PolygonKind* polygonKind = Allocate( sizeof( PolygonKind ) );
	ChainKind* chainKind = Allocate( sizeof( ChainKind ) );
	memset( polygonKind, 0, sizeof( *polygonKind ) );
	memset( chainKind, 0, sizeof( *chainKind ) );

	b2Polygon* polygons = NULL;
	int polygonCount = 0;
	Bytes polygonForm = { 0 };
	MeasurePolygonLadder( &options, polygonKind, &polygons, &polygonCount, &polygonForm );
	polygonKind->file = WriteForm( &options, POLYGON_FILE, &polygonForm );
	MeasurePolygonBehavior( &options, polygonKind, polygons, polygonCount, &polygonForm, &scratch );

	ChainInput chainInput = { 0 };
	Bytes chainImage = { 0 };
	MeasureChainLadder( &options, chainKind, &chainInput, &chainImage );
	chainKind->file = WriteForm( &options, CHAIN_IMAGE_FILE, &chainImage );
	MeasureChainBehavior( &options, chainKind, &chainInput, &chainImage, polygons, polygonCount, &scratch );

	PrintReport( &options, polygonKind, chainKind );

	free( chainInput.points );
	FreeBytes( &chainImage );
	FreeBytes( &polygonForm );
	free( polygons );
	free( polygonKind );
	free( chainKind );
	free( scratch.reference );
	free( scratch.hashes );
	free( scratch.list.ids );
	return 0;
}

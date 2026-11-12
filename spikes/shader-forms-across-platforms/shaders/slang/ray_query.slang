// A compute shader that asks one ray of an acceleration structure: compiled
// where a chain claims ray queries, never run.
[[vk::binding(0, 0)]]
RaytracingAccelerationStructure scene_structure : register(t0);
[[vk::binding(1, 0)]]
RWStructuredBuffer<uint> hits : register(u0);

[shader("compute")]
[numthreads(1, 1, 1)]
void ray_main(uint3 invocation : SV_DispatchThreadID)
{
    RayDesc ray;
    ray.Origin = float3(0.0, 0.0, 0.0);
    ray.Direction = float3(0.0, 0.0, 1.0);
    ray.TMin = 0.0;
    ray.TMax = 100.0;
    RayQuery<RAY_FLAG_NONE> query;
    query.TraceRayInline(scene_structure, RAY_FLAG_NONE, 0xff, ray);
    query.Proceed();
    hits[invocation.x] = query.CommittedStatus() == COMMITTED_TRIANGLE_HIT ? 1u : 0u;
}

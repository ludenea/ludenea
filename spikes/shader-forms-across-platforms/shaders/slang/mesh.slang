// A mesh shader that emits one triangle: compiled where a chain claims the
// stage, never run.
struct MeshVertex
{
    float4 position : SV_Position;
};

[shader("mesh")]
[outputtopology("triangle")]
[numthreads(1, 1, 1)]
void mesh_main(out indices uint3 triangles[1], out vertices MeshVertex corners[3])
{
    SetMeshOutputCounts(3, 1);
    corners[0].position = float4(-1.0, -1.0, 0.0, 1.0);
    corners[1].position = float4(3.0, -1.0, 0.0, 1.0);
    corners[2].position = float4(-1.0, 3.0, 0.0, 1.0);
    triangles[0] = uint3(0, 1, 2);
}

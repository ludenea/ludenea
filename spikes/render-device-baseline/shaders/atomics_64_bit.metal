// A Metal kernel that takes the maximum into a 64-bit atomic, compiled at run
// time by the system's Metal compiler and never dispatched: whether the
// compiler accepts it and whether the device builds its pipeline is what the
// spike reports for 64-bit atomics.
#include <metal_stdlib>
using namespace metal;

kernel void atomics_64_bit_probe(uint cell [[thread_position_in_grid]], device atomic_ulong* cells [[buffer(0)]]) {
    atomic_max_explicit(&cells[cell], (ulong)cell, memory_order_relaxed);
}

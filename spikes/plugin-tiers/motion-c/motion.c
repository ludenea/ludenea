/* The reference workload's motion pass in C, for the plugin tiers spike of the
 * plugin architecture research. Throwaway, never product code.
 *
 * The same pass as the Rust crate, with the specification's order of
 * floating-point operations. Built twice at run time by the spike with CMake:
 * once with the C compiler's default flags, once with the permitted
 * configuration (no contraction). The energy's `(vx * vx) + (vy * vy)` is the
 * contraction candidate a compiler may fuse into a multiply-add when its
 * default allows it, which changes the result in the last bit. */

#include <stdint.h>

#if defined(_WIN32)
#define MOTION_EXPORT __declspec(dllexport)
#else
#define MOTION_EXPORT __attribute__((visibility("default")))
#endif

static const float TIME_STEP = 0.015625f; /* 2^-6 */
static const float WORLD_SIDE = 1024.0f;

static void bounce(float *p, float *v) {
    if (*p < 0.0f) {
        *p = -*p;
        *v = -*v;
    }
    if (*p >= WORLD_SIDE) {
        *p = (2.0f * WORLD_SIDE) - *p;
        *v = -*v;
    }
    if (*p >= WORLD_SIDE) {
        *p = WORLD_SIDE - 0.0625f;
    }
}

MOTION_EXPORT float motion(const float *x, const float *y, const float *mass, const uint8_t *alive, float *next_x,
                           float *next_y, float *next_vx, float *next_vy, uint32_t count, uint32_t tick) {
    (void)tick;
    float sum = 0.0f;
    for (uint32_t k = 0; k < count; k++) {
        if (alive[k] == 0) {
            continue;
        }
        float vx = next_vx[k];
        float vy = next_vy[k];
        float px = x[k] + (vx * TIME_STEP);
        float py = y[k] + (vy * TIME_STEP);
        bounce(&px, &vx);
        bounce(&py, &vy);
        next_x[k] = px;
        next_y[k] = py;
        next_vx[k] = vx;
        next_vy[k] = vy;
        sum += ((vx * vx) + (vy * vy)) * (0.5f * mass[k]);
    }
    return sum;
}

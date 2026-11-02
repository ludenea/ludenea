# Platform facts

Throwaway spike of the cross-platform compatibility research. It is the first spike of the research CI and doubles as its smoke test.

## Hypothesis

On the five cells of the desktop boundary (Windows, macOS and Linux on x86_64 and ARM64, macOS on ARM64 only), compiled with one pinned Rust toolchain at default target features:

1. The results of the operations IEEE 754 defines exactly (basic arithmetic, remainder, square root, fused multiply-add, rounding, conversions) are bit-identical on every cell.
2. The compiler neither contracts `a * b + c` into a fused multiply-add nor reassociates sums, on any cell, even where the hardware has fused multiply-add instructions.
3. The transcendental functions served by the platform math library (`sin`, `cos`, `exp`, `ln`, `powf` and the others) are not bit-identical across cells.
4. The bit pattern of the NaN produced by an invalid operation differs between x86_64 and ARM64.

An instrumental hypothesis comes first: standard GitHub-hosted runners, ARM64 Linux and Windows included, build and run the spike in a public repository at no cost.

## Method

The program prints one JSON fact sheet: platform and build facts, then the bit patterns of a fixed set of floating-point probes grouped as `exact`, `codegen`, `libm` and `nan`, each group hashed (FNV-1a 64) so that cells compare at a glance. Every probe input passes through `std::hint::black_box`, otherwise the compiler would fold the probe at build time with the build machine's math library and hide the runtime fact. A fixed arithmetic loop is timed five times to document the noise of the machine. The script `ci/run-spike.sh` records the environment (operating system, CPU, C library, compiler versions) next to the facts.

`f32` values are printed as their 32-bit pattern, zero-extended to 64 bits. Integer results of casts are printed as their two's complement 64-bit pattern.

## Environment

Research CI, workflow `research-ci.yml`, five cells (see [`spikes/README.md`](../README.md)). Rust toolchain pinned in [`rust-toolchain.toml`](../rust-toolchain.toml). Release profile, default target features of each target: no `target-cpu` override, so x86_64 builds target the baseline x86-64 instruction set (SSE2, no FMA) and ARM64 builds target the baseline ARMv8 (NEON).

## Result

Pending: the first run of the research CI records the per-cell facts here.

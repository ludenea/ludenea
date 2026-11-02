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

First [run of the research CI](https://github.com/ludenea/ludenea/actions/runs/41419772718) on 2026-11-02, rustc 1.99.1 with LLVM 23.1.1 on every cell:

| Cell             | Machine                      | Operating system                           | `exact`              | `codegen`            | `libm`               | `nan`                |
| ---------------- | ---------------------------- | ------------------------------------------ | -------------------- | -------------------- | -------------------- | -------------------- |
| `linux-x86_64`   | AMD EPYC 7763                | Ubuntu 24.04.5, glibc 2.39                 | `0x4b06bfeb1b00c754` | `0x4035b998dcfeb02c` | `0x1b0e6a4442b20501` | `0xd8a41e3e092f1770` |
| `linux-arm64`    | Arm Neoverse N2              | Ubuntu 24.04.5, glibc 2.39                 | `0x4b06bfeb1b00c754` | `0x4035b998dcfeb02c` | `0x1b0e6a4442b20501` | `0xd4890260925bddf0` |
| `windows-x86_64` | AMD EPYC 7763                | Windows Server 2025 Datacenter, 10.0.26100 | `0x4b06bfeb1b00c754` | `0x4035b998dcfeb02c` | `0x82807dc9a3b78b2d` | `0xd8a41e3e092f1770` |
| `windows-arm64`  | Microsoft Cobalt 100         | Windows 11 Enterprise, 10.0.26200          | `0x4b06bfeb1b00c754` | `0x4035b998dcfeb02c` | `0x82807dc9a3b78b2d` | `0xd4890260925bddf0` |
| `macos-arm64`    | Apple M1 (virtual)           | macOS 15.8.2                               | `0x4b06bfeb1b00c754` | `0x4035b998dcfeb02c` | `0x68255a5365fd7eb2` | `0xd4890260925bddf0` |

- Exact operations (hypothesis 1) hold: the `exact` group is bit-identical on the five cells.
- No contraction (hypothesis 2) holds: the `codegen` group is bit-identical and every contraction probe is zero, although every machine has fused multiply-add hardware.
- Math library variance (hypothesis 3) holds with a nuance: the `libm` group differs per operating system family only, never per architecture (Linux x86_64 equals Linux ARM64, Windows x86_64 equals Windows ARM64). Of 31 probes, 5 differ by one unit in the last place: `acos`, `asin` and `tan` on macOS; `cbrt` and `ln_1p` on Windows. The other 26, the eight `f32` probes included, agree everywhere.
- NaN sign (hypothesis 4) holds: a generated NaN is `0xfff8000000000000` on x86_64 and `0x7ff8000000000000` on ARM64 (`0xffc00000` against `0x7fc00000` in `f32`), whatever the operating system; an existing NaN payload propagates identically everywhere.
- Runner availability (the instrumental hypothesis) holds: all five cells ran on standard runners at no cost.
- Timing of the scalar loop, median nanoseconds per iteration: 8.32 (Linux x86_64), 8.40 (Windows x86_64), 6.50 (Linux ARM64), 6.48 (Windows ARM64), 7.39 (macOS ARM64); within-run spread under 2 percent on four cells and 7 percent on macOS.

The raw facts and the environment of each cell are the artifacts of the run. The result holds for this toolchain and these runner images; a toolchain bump re-runs the spike.

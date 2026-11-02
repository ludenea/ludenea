# Reference hashes

The hashes of the sequential reference implementation of the reference workload, measured on the five cells of the research CI, per workload version and toolchain version. A spike that runs the workload compares its hashes with these: a difference locates either a mistake in the spike or a platform fact worth recording. Hashes of different workload versions are never compared.

## Workload version 1, Rust 1.99.1

Research CI [run](https://github.com/ludenea/ludenea/actions/runs/41492132541) of 2026-11-02, release profile, default target features of each target, seed `0x4c7564656e656121`, on Linux x86_64 (AMD EPYC 7763, Ubuntu 24.04.5), Linux ARM64 (Neoverse N2, Ubuntu 24.04.5), Windows x86_64 (AMD EPYC 7763, Windows Server 2025), Windows ARM64 (Cobalt 100, Windows 11) and macOS ARM64 (Apple M1 virtual machine, macOS 15.8.2). An unoptimized build (the `dev` profile, debug assertions on) on Linux x86_64 ([run](https://github.com/ludenea/ludenea/actions/runs/41492184955)) gave the same hashes for every row, and so did a rebuild of the default size with `-C target-cpu=x86-64-v3` on the two x86_64 cells ([run](https://github.com/ludenea/ludenea/actions/runs/41492168053)) and a local run on an Apple M5 under macOS 26.

### Wander system off: one value on every cell

| Size | Entities | Ticks | Run hash `H` | Initial hash `h_0` | Final hash `h_T` |
| --- | --- | --- | --- | --- | --- |
| small | 1 000 | 1 000 | `0xc6e6180d911f9dd0` | `0x1c645a13a62c9cd9` | `0x3488484e98147817` |
| default | 10 000 | 1 000 | `0x9f50cc4763ae733c` | `0x868005e0d753a5ae` | `0xecb0609f0d23f6a1` |
| large | 100 000 | 100 | `0xe7e7430ee2cdbc69` | `0x544ff1d7033f2cf5` | `0xef07802c0e7be41f` |

The initial hashes and the first ticks of the small and default sizes are also the values the independent Python implementation (`oracle.py`) computes.

### Wander system on: one value per platform math library

The wander system calls the platform's `f32` cosine and sine on 4 096 distinct arguments every tick. Three classes of results appeared, the third one separating the two Windows architectures:

| Size | Ticks | Linux x86_64, Linux ARM64, Windows ARM64 | macOS ARM64 (and the local Apple M5) | Windows x86_64 |
| --- | --- | --- | --- | --- |
| small | 1 000 | `H` `0x1dcfd45c551f44bf`, `h_T` `0x6dde9afba89c0f86` | `H` `0x41ddf0b01c6a6a82`, `h_T` `0x30d4e8b1b829ac2b` | `H` `0x774d93f714a47053`, `h_T` `0xeb936ab1153a45c0` |
| default | 1 000 | `H` `0x756937193f1eb200`, `h_T` `0x6f3deb4f09ead4a3` | `H` `0xd4f14b2dd11228d1`, `h_T` `0x44c393f6e0019ce0` | `H` `0x66f88ba45f145858`, `h_T` `0x9931fd137929fdde` |
| large | 100 | `H` `0xb53bc13976de17cf`, `h_T` `0x22e1a33d14f9b9e2` | `H` `0xd603bd67df895ee6`, `h_T` `0x1f8edce134360f81` | `H` `0x6ad82fbc0a0b8b0d`, `h_T` `0xd6c12535c1f81548` |

### Throughput of the sequential reference, for orientation

Ticks per second of the default size with the wander system off, release profile: 500 on Linux x86_64 (EPYC 7763), 755 on Linux ARM64 (Neoverse N2), 514 on Windows x86_64 (EPYC 7763), 735 on Windows ARM64 (Cobalt 100), 482 on the macOS runner (M1 virtual), 1 033 on the local Apple M5; 148 in the unoptimized build on Linux x86_64 (EPYC 9V45). Timings from hosted runners compare per processor only.

# Visual reference round trip

Throwaway spike of the game testing research. It lays a visual reference out as reviewed files (a content part that is the same on every device, a byte part per device key, a pixel file), carries those files between jobs and between cells, runs the three comparisons of a visual test against them (content equality, byte equality, a bounded difference) with the cause each one reports, and measures what a reference costs.

It stands on the [device baseline spike](../render-device-baseline/README.md): its filtering and blending scene, its native path to Direct3D 12, Vulkan and Metal, and its PNG writer. It measures nothing of a renderer. Its content hash is a stand-in, the hash of the draw list the spike's own code builds, so it cannot show that a real renderer's record lacks something. It is evidence, never product code.

## Layout

- `src/frame.rs`: the frame as the spike's own draw list (one view of three passes, the device baseline's ten draws plus one triangle that lies beyond the view), its content hash with a part per view, per pass and per asset, the hash of each asset's payload, the changes brought about on purpose, and the adapter that turns the list into what the device baseline's native path draws.
- `src/reference.rs`: the parts as text files and a set as a directory.
- `src/comparison.rs`: the three comparisons, the facts that make a cause, the pending parts and the group a review puts them in.
- `src/pixels.rs`: the pixel file, its strict reader, the numbers of a difference, the independent decoder and the repository's packed form.
- `src/capability.rs`: Mesa's software device held to a stated processor capability, one capture per setting, and the processor's reciprocal square root estimate.
- `src/main.rs`: the measurements and the report.
- `tools/system-drawing.ps1`: the independent decoder of the Windows cells.
- `setup.sh`: what a machine of the research CI needs installed (below).
- `references/from-<cell>/`: the reference set each cell wrote on its first run, accepted into the repository for the later runs.
- `key-record.sh`: reads, from the runs of the research CI, the key, the processor and the bytes each cell reported at each run, and counts per cell the changes of key and the changes of bytes under an unchanged key (`bash key-record.sh`, with `gh` and `jq`).

## A reference set

A set is a directory:

- `content.txt`, the content part: the engine version, the declared level, the hash of the fixture's starting state, the tick, the target with its size, format, encoding and sample count, the hash of the presentation settings, the content hash with its part per view and per pass, and for each asset its identifier with the hash of what its payload feeds into the content (its size, the count of its levels, its encoding).
- `bytes-<hash of the key>.txt`, a byte part per key: the key in full, the hash of the frame's bytes, the hash of each payload the key's device bound, the name of the pixel file, whether the part is the set's base, and the processor of the machine that drew the frame, recorded beside the key and not part of it.
- `<hash of the file's bytes>.png`, a pixel file: 8-bit RGBA, no filter, one stream of stored (uncompressed) blocks, the plainest lossless form of the format.

A part is text, one `name = value` per line in one order. A pending part is the same file with `.pending` after its name; it is written into the directory a run is given, here the run's artifacts, and never over an accepted part.

The key is the interface, the device's name, its implementation and version, the version of the interface it offers, its settings (for Mesa's software device the `LP_` variables of the environment, the thread count among them) and the processor architecture. The run states four device threads: it sets `LP_NUM_THREADS` before any device exists where the environment has none.

## Hypotheses

Stated in the game testing findings before the spike was built. A hypothesis with no threshold is a figure to record.

1. **The round trip of the files.** A reference set written by one job of a cell is read by a later job of the same cell on a fresh runner, where the same key is expected and the bytes are equal, the key and the runner's processor being recorded since the machines of a hosted cell are not one model; and by the jobs of the other cells, where the byte comparison is "not compared", the content is equal, and a pending byte part appears among the run's artifacts.
2. **The causes.** Each fact of the table of causes, brought about on purpose, is reported with the verdicts of its row for the three comparisons: an item moved inside a view, and outside every view; a texture encoded again to other bytes with the same part in the content; a fault of the adapter beneath the content, a blend state it sets wrongly; a run under another key; and a key that moved together with a changed item, which falls outside the review's group of a key that moved and nothing else.
3. **The pixel file.** A file written in the plainest lossless form is decoded to the same pixels by an independent decoder the run's setup installs, and read back by the spike to the bytes it wrote on every cell; a file a tool rewrote without loss is refused by the spike's reader; and a file whose pixels changed is caught by the hash of its byte part.
4. **Its weight**, with no threshold: the size of such a file as written, in a checkout and in the repository's packed form, beside the same picture compressed in the file.
5. **The costs**, with no threshold: one capture (the render, the read-back, the hash) at three target sizes on each software device, at one and at four device threads where the device has the setting; and the bounded difference per million pixels.
6. **The container on a developer's machine.** A run inside the Linux container on a machine of the same processor architecture reports the key of the cell's run in that image and gives its bytes.
7. **How often a key moves**, with no threshold: the key each cell reports is recorded at every run of the research CI for eight weeks, with the count of its changes.
8. **The device held to a stated processor capability**, added on 2026-11-13 after the first result: Mesa's software device has a setting, a stated vector width with processor capabilities held to a named level, under which a host with an AMD processor and a host with an Intel processor draw the same bytes on the Linux x86_64 cell. Refuted if at every setting the two draw two pictures.

## Method

The research CI builds and runs the binary on the five cells after `setup.sh`; locally it runs the same way (`ci/run-spike.sh visual-reference-round-trip <cell>`). On each cell the spike opens the software device of the platform's interface (Mesa's software device through Vulkan on Linux, the Windows software adapter through Direct3D 12), or failing one the device the virtual machine presents (Metal on the macOS cell), and captures the frame once.

**The round trip.** The spike reads every set under `references/` and its own cell's set whether or not it exists, as a run that takes references, and runs the three comparisons of its capture against each. On a cell's first run its own set does not exist: the three comparisons fail and leave a pending content part, a pending byte part and a pixel file under `out/<cell>/pending/from-<cell>/`. Those files are then taken from the run's artifacts and accepted into the repository as `references/from-<cell>/`, which is the review's accept. A later run of the research CI, on fresh runners, reads the five sets from its checkout.

**The causes.** The spike accepts its own capture as a set under its key, and the same parts under a key named otherwise, in a temporary directory. Each case then builds its frame with one option, captures it on a device created from scratch and compares it with one of the two sets. Eleven cases: nothing changed; the six of the second hypothesis, the texture encoded again taken twice (where a draw samples it, and where none does); and three rows of the table the hypothesis does not list (no reference, a target of another size, a device that draws nothing). A case is reported as designed when the verdicts of the three comparisons, the pending parts of the byte comparison and the review group are those of its row; where the row says "decided on its numbers", when the bounded difference was made against the base and its verdict is what the numbers give. The test's numbers are a largest difference of 16 per channel, a second of 4, and 1 024 pixels allowed beyond the second.

**The pixel file.** The frame is written, read back from disk by the strict reader, and decoded by the platform's independent decoder. The platform's tool writes the picture again, compressed; the decoder reads that file and the strict reader is asked to. One channel of one pixel is changed in a file kept in the writer's form, which is read and hashed.

**The weight and the costs.** At 256 by 256, 1 024 by 1 024 and 1 920 by 1 080 the frame and the frame with an item moved are written as pixel files; the first is measured as written, compressed in the file by the platform's tool, and as the pack `git pack-objects` writes for it alone and for the two revisions under one path. One child process per size and per setting then captures the frame five times on one device: as the run states it, at one and four device threads on Mesa's software device, and with the process restricted to one and four processors on the Windows software adapter, which documents no thread setting. Each repetition times the render to the completion of the device's work, the read-back, the hash of the bytes (64-bit FNV-1a, one byte at a time: a stand-in), the content hash, and the bounded difference of the picture against a copy a tenth of whose pixels differ.

**The device held to a stated processor capability**, on Mesa's software device on x86_64 only. Mesa 25.2.8, read in its sources, has two settings that bear on what its generated code may use: `LP_NATIVE_VECTOR_WIDTH`, the width of the vectors (256 bits at most by default), and `GALLIUM_OVERRIDE_CPU_CAPS`, which clears the processor capabilities above a named level (`avx`, `sse4.1`, `sse2`, `nosse` among them) before the device hands them to the code generator. The processor's name, which the code generator takes from the host and tunes for, has no setting. The spike captures the frame at 256 by 256 once per setting, each in a process of its own with a shader cache directory of its own, since the cache keys its entries on neither setting: as started (the control), the width at 128 and at 256 bits, the capabilities held below AVX-512 at both widths, below AVX, to SSE2, and with no SSE stated, and two captures without the optimization passes (`GALLIVM_PERF=nopt`, which states no capability and is there to say whether the hosts part in optimized code). Each capture is compared with the run's own, its picture is kept under `held/` in the run's artifacts, its render is timed (fastest of five after the first), and the spike records whether the reference key states the setting. Beside them it records what the device reads of the processor (vendor, family, model, stepping and the capabilities, from `/proc/cpuinfo`) and the results of the processor's reciprocal square root estimate instruction (`rsqrtss` over 2^20 inputs spread over two binades, as a hash, and at 2 and at 3). A run has one host: whether a setting makes two hosts alike is read across runs of the cell, each on the host the runner pool gives. `--difference <pixel file> <pixel file>` prints the numbers of the difference between two pictures taken from two runs.

**Departures from the specification**, each stated here and in the findings:

- The specification has each cell run two jobs in one run, the second reading the first's references from the run's artifacts. The research CI has one job per cell, and no second job was added to it: the second job is a later run of the research CI, and the references reach it through the first run's artifacts accepted into the repository. The files still travel from one runner to a fresh one, and through the path a reference takes in a project (an artifact of a run, a review, a commit).
- "Each cause is a build option of the spike" is read as an option of the frame's build, chosen at run time in one binary, not as a compilation option.
- A run under another key is brought about in two ways: within a cell by accepting the cell's own part under a key named otherwise, and across cells by the sets the other cells wrote, whose keys are those of other devices.
- The device baseline spike's scene gained a target size of its own for the three sizes; at 256 by 256 it renders the bytes it rendered before.

`setup.sh` installs, on a Linux machine, the Vulkan loader, Mesa's Vulkan drivers, the GLSL compiler, Netpbm and git from the distribution's packages; on macOS, Netpbm from Homebrew; on Windows nothing.

The self-checks (`cargo test -p visual-reference-round-trip`) verify the frame's content parts under each change, the text form of the parts, the three comparisons on each row of the table with pictures made by hand, the strict reader, and the numbers of a difference.

## Environment

The five cells of the research CI; the `rust:1.99.1-slim-trixie` container on the Linux x86_64 cell and on a developer machine (Windows 11, under its Linux subsystem) for the sixth hypothesis.

No third-party library is linked or vendored. The independent decoders are tools started as processes: on Linux and macOS `pngtopam` and `pamtopng` of Netpbm (the distribution's package states for both the permissive notice it labels MIT: permission to use, copy, modify and distribute for any purpose and without fee, provided the notices are kept), which read and write with libpng (the libpng license) and zlib (the zlib license); on Windows the PNG codec of GDI+ reached through `System.Drawing` in Windows PowerShell, both parts of Windows. Mesa (MIT license) and the glslang command-line tool (BSD and Apache 2.0 licenses) are those of the device baseline spike; git (GPL 2.0) is run as a tool to write a pack.

## Result

The eighth hypothesis was measured on 2026-11-13 and its runs are named with its result. The others were measured on 2026-11-12 on the five cells of the research CI: a [first run](https://github.com/ludenea/ludenea/actions/runs/43281455488), in which each cell took its references; a [second run](https://github.com/ludenea/ludenea/actions/runs/43282602225) on fresh runners after those references were accepted into the repository, whose macOS cell found no runner and ran [alone](https://github.com/ludenea/ludenea/actions/runs/43285128958); four more runs of the Linux x86_64 cell ([one](https://github.com/ludenea/ludenea/actions/runs/43284674109), [two](https://github.com/ludenea/ludenea/actions/runs/43284688936), [three](https://github.com/ludenea/ludenea/actions/runs/43284701183), [four](https://github.com/ludenea/ludenea/actions/runs/43284713860)); the two Linux cells in the `rust:1.99.1-slim-trixie` container ([run](https://github.com/ludenea/ludenea/actions/runs/43282634519)); and a developer machine (AMD Ryzen 9 9950X3D, Windows 11), natively and in that container, named by its digest, under the Linux subsystem. Rust 1.99.1, release profile. The raw measurements, the pending parts and the pictures are in the runs' artifacts.

The content hash is one value on every machine, as a hash of the spike's own draw list must be. The pictures:

| Machine | Device and implementation | Processor | Frame bytes |
| --- | --- | --- | --- |
| `linux-x86_64`, three runs | llvmpipe (LLVM 20.1.2, 256 bits), Mesa 25.2.8 | AMD EPYC 7763, AMD EPYC 9V74 (twice) | `e3a283950520fad1` |
| `linux-x86_64`, three runs | the same device, under the same key | Intel Xeon 6973P-C, Intel Xeon 6975P-C (twice) | `1109dde3338ef346` |
| `linux-arm64`, two runs | llvmpipe (LLVM 20.1.2, 128 bits), Mesa 25.2.8 | Arm Neoverse N2 | `a96b67d1d9209ee0` |
| `windows-x86_64`, two runs | Microsoft Basic Render Driver 10.0.26100.33648 | AMD EPYC 7763 | `f81b63a351b9eedc` |
| `windows-arm64`, two runs | Microsoft Basic Render Driver 10.0.26100.9711 | Cobalt 100 | `f81b63a351b9eedc` |
| `macos-arm64`, two runs in two data centers | Apple Paravirtual device, macOS 15.8.2 | Apple M1 (Virtual) | `e46b7fa74288cad1` |
| container on the Linux x86_64 cell | llvmpipe (LLVM 19.1.7, 256 bits), Mesa 25.0.7 | AMD EPYC 9V74 | `e3a283950520fad1` |
| container on the Linux ARM64 cell | llvmpipe (LLVM 19.1.7, 128 bits), Mesa 25.0.7 | Arm Neoverse N2 | `a96b67d1d9209ee0` |
| container on the developer machine | llvmpipe (LLVM 19.1.7, 256 bits), Mesa 25.0.7 | AMD Ryzen 9 9950X3D | `e3a283950520fad1` |
| developer machine | Microsoft Basic Render Driver 10.0.26100.9818 | AMD Ryzen 9 9950X3D | `f81b63a351b9eedc` |

1. **The round trip of the files: holds on four cells and between cells, refuted on the Linux x86_64 cell.**
   - On a later run, on a fresh runner, the two Windows cells, the Linux ARM64 cell and the macOS cell report the key of their reference and its bytes: the three comparisons pass and no pending part is written. The two macOS runs landed on hosts in two data centers.
   - The Linux x86_64 cell reports one key in its six runs and two pictures. On the hosts with an AMD processor it draws the reference's bytes; on the hosts with an Intel processor, three runs of six, it draws another picture, 854 pixels of 65 536 apart by one level per channel. There the byte comparison fails under the reference's own key, with the cause "the bytes differ" (content and payloads equal), and leaves a pending byte part under that same key; the bounded difference passes. The key as designed (the interface, the device, its implementation and version, its settings and thread count, the processor architecture) does not tell the two hosts apart; the processor recorded beside it does.
   - Read by another cell, each of the twenty pairs is reported as designed: the content is equal, the byte comparison is "not compared", and a pending byte part with its pixel file under the reading cell's key is among the run's artifacts.
   - The two Windows cells hold two keys, two versions of the adapter, and draw the same bytes: each is "not compared" against the other's part, and the bounded difference against it measures no differing pixel. The two versions of Mesa and LLVM, on the runner and in the container, are two keys with the same bytes as well, on each architecture.
   - The bounded difference against another cell's base, under the test's numbers (16, 4, 1 024 pixels beyond 4):

     | Capture and base | Differing pixels of 65 536 | Largest difference per channel | Verdict |
     | --- | --- | --- | --- |
     | Linux x86_64 and Linux ARM64 | 11 448 on an AMD host, 11 713 on an Intel host | 4, 4, 4, 0 | passes |
     | The Windows cells and the macOS cell | 10 532 | 6, 7, 7, 0 | passes |
     | The Windows cells and Linux x86_64 | 16 549 on an AMD host, 16 634 on an Intel host | 8, 8, 8, 0 | fails |
     | The Windows cells and Linux ARM64 | 16 650 | 9, 9, 9, 0 | fails |
     | The macOS cell and Linux x86_64 | 16 330 on an AMD host, 16 408 on an Intel host | 11, 10, 10, 0 | fails |
     | The macOS cell and Linux ARM64 | 16 426 | 11, 12, 12, 0 | fails |

2. **The causes: holds.** The eleven cases are reported with the verdicts of their row on the five cells, in the container and on the developer machine:

   | Case | Content equality | Byte equality | Pending part of the byte comparison | Bounded difference |
   | --- | --- | --- | --- | --- |
   | Nothing changed | passes | passes | none | passes |
   | The reference does not exist | fails | fails | byte part | fails |
   | A target of another size | fails | fails | byte part | fails |
   | An item moved inside the view | fails | fails | byte part | fails on its numbers |
   | An item moved outside every view | fails | passes | refresh of the content part | passes |
   | A texture encoded again | passes | fails | byte part | passes on its numbers |
   | A texel no draw samples encoded again | passes | passes | refresh of the byte part | passes |
   | The adapter sets a blend state wrongly | passes | fails | byte part | fails on its numbers |
   | A run under another key | passes | not compared | byte part under a new key | passes |
   | A key that moved with a changed item | fails | not compared, fails on the content | byte part under a new key | fails on its numbers |
   | A device that draws nothing | passes | not made | none | not made |

   The item moved inside the view changes 693 pixels on every device and is named in the first pass of the view. The item moved outside every view and the texel no draw samples change no byte on any device. The texture encoded again changes 4 521 to 4 532 pixels by at most 5 levels, 156 to 208 of them beyond the test's second number: the byte comparison fails with the cause "a payload differs" and the bounded difference passes. The blend state set wrongly changes 4 096 pixels with content and payloads equal: the byte comparison alone catches it, with the cause "the bytes differ". The key that moved with a changed item is put in the review group "the content differs and the key moved", not in the group of a key that moved alone.
3. **The pixel file: holds on every cell.** The file is read back by the spike to the bytes it wrote, and the independent decoder gives the same pixels: `pngtopam` of Netpbm 11.05.02 with libpng 1.6.43 on the Linux cells, of Netpbm 11.10.02 with libpng 1.6.48 in the container, of Netpbm 10.86.51 with libpng 1.6.59 on the macOS cell, and the PNG codec of GDI+ on the two Windows cells. The file each platform's tool wrote again decodes to the same pixels and is refused by the strict reader ("a compressed deflate block"). The file with one changed pixel is read, and its hash is not the byte part's.
4. **The weight**, in bytes, the packed form within 4 percent from one cell to another:

   | Target | As written | Compressed in the file, libpng | Compressed in the file, GDI+ | Packed | Packed, two revisions |
   | --- | --- | --- | --- | --- | --- |
   | 256 by 256 | 262 488 | 35 842 to 37 074 | 71 648 | 62 936 to 63 404 | 64 173 to 64 641 |
   | 1 024 by 1 024 | 4 195 716 | 343 048 to 377 712 | 615 398 | 578 661 to 601 166 | 583 243 to 605 721 |
   | 1 920 by 1 080 | 8 296 178 | 499 345 to 570 484 | 931 471 | 887 390 to 923 095 | 892 751 to 928 403 |

   The packed form of one picture is 24, 14 and 11 percent of the file as written; a second revision in which one triangle moved adds 1 to 5 kilobytes to the pack. A checkout holds the file as written. The content part is 683 bytes and a byte part 559 to 823. The scene is flat colors and five small textures: a frame of a game compresses less.
5. **The costs**, one capture in milliseconds, fastest of five (the render to the completion of the device's work, the read-back and the hash of the bytes):

   | Cell, device and setting | 256 by 256 | 1 024 by 1 024 | 1 920 by 1 080 | Of which the hash at 1 920 by 1 080 |
   | --- | --- | --- | --- | --- |
   | `linux-x86_64`, llvmpipe, four threads | 3.6 | 55.7 | 101.8 | 10.4 |
   | `linux-x86_64`, llvmpipe, one thread | 5.2 | 67.3 | 134.6 | 10.4 |
   | `linux-arm64`, llvmpipe, four threads | 3.6 | 46.5 | 86.7 | 7.4 |
   | `linux-arm64`, llvmpipe, one thread | 7.1 | 100.6 | 193.6 | 7.4 |
   | `windows-x86_64`, software adapter, as started | 2.1 | 26.3 | 52.2 | 10.3 |
   | `windows-x86_64`, software adapter, one processor | 4.2 | 38.0 | 74.3 | 10.3 |
   | `windows-arm64`, software adapter, as started | 3.2 | 45.2 | 89.5 | 7.4 |
   | `windows-arm64`, software adapter, one processor | 4.7 | 71.6 | 141.7 | 7.4 |
   | `macos-arm64`, virtual device | 16.3 | 32.3 | 57.5 | 11.7 |

   Every setting of a cell gives one picture per size. The four-processor setting of the Windows software adapter costs what the cell's four processors do as started. The read-back is 2.5 to 7.7 milliseconds at 1 920 by 1 080. The hash, 64-bit FNV-1a one byte at a time, is 8 to 20 percent of a capture at that size on a software device. The content hash of the frame takes 7 to 14 microseconds at any size. The bounded difference of two pictures takes 1.5 milliseconds per million pixels on the two x86_64 cells, 0.8 on the Linux and Windows ARM64 cells and 0.7 on the macOS cell.
6. **The container on a developer's machine: holds for the pair measured.** The developer machine, in the container named by the digest the cell's run recorded, reports the key of the cell's run in that image and draws its bytes. Both machines have an AMD processor: by the first result, the same container on an Intel host would report that key and is not expected to draw those bytes, which was not measured. The device's version in the container is the one the distribution's packages gave when the setup installed them, not one the image's digest fixes.
7. **How often a key moves: not measured yet.** It needs eight weeks of runs. The record begins with one day: each cell reported one key in each of its runs, and on the Linux x86_64 cell the bytes moved under that key four times in six runs. Since 13 November 2026 the research CI runs the spike on the five cells every day, until 7 January 2027; `key-record.sh` reads the record, to be read before the artifacts of its first runs expire, ninety days after them.
8. **The device held to a stated processor capability: refuted.** Measured on 2026-11-13 in fourteen runs of the Linux x86_64 cell, each on the host the pool gave: six with the settings down to SSE2 ([one](https://github.com/ludenea/ludenea/actions/runs/43442440954), [two](https://github.com/ludenea/ludenea/actions/runs/43442448513), [three](https://github.com/ludenea/ludenea/actions/runs/43442455841), [four](https://github.com/ludenea/ludenea/actions/runs/43442461640), [five](https://github.com/ludenea/ludenea/actions/runs/43442466787), [six](https://github.com/ludenea/ludenea/actions/runs/43442471767)), then eight with every setting, the render times and the estimate ([one](https://github.com/ludenea/ludenea/actions/runs/43444067042), [two](https://github.com/ludenea/ludenea/actions/runs/43444071779), [three](https://github.com/ludenea/ludenea/actions/runs/43444077768), [four](https://github.com/ludenea/ludenea/actions/runs/43444082950), [five](https://github.com/ludenea/ludenea/actions/runs/43444088450), [six](https://github.com/ludenea/ludenea/actions/runs/43444093635), [seven](https://github.com/ludenea/ludenea/actions/runs/43444098850), [eight](https://github.com/ludenea/ludenea/actions/runs/43444105024)). Nine hosts had an AMD processor (EPYC 7763 three times, EPYC 9V45 four times, EPYC 9V74 twice) and five an Intel processor (Xeon Platinum 8370C, Xeon Platinum 8573C three times, Xeon 6975P-C); llvmpipe of Mesa 25.2.8 with LLVM 20.1.2 on all.

   | Setting | AMD hosts, frame bytes | Intel hosts, frame bytes | Pixels apart between the two, of 65 536 | Pixels apart from the host's own unheld picture | Render, ms |
   | --- | --- | --- | --- | --- | --- |
   | As started | `e3a283950520fad1` | `1109dde3338ef346` | 854 | 0 | 2.0 to 3.2 |
   | 128-bit vectors | `e3a283950520fad1` | `1109dde3338ef346` | 854 | 0 | 2.4 to 3.8 |
   | 256-bit vectors, stated | `e3a283950520fad1` | `1109dde3338ef346` | 854 | 0 | 2.1 to 3.2 |
   | Below AVX-512, at either width | `e3a283950520fad1` | `1109dde3338ef346` | 854 | 0 | 2.1 to 4.9 |
   | Below AVX, 128-bit vectors | `aab17ea949f9d5bb` | `35b5f39616927970` | 854 | 5 | 2.7 to 4.3 |
   | SSE2, 128-bit vectors | `dabd2f4bbb735ce5` | `b545d8980935af51` | 842 | 11 253 on AMD, 11 258 on Intel | 3.2 to 4.9 |
   | No SSE stated, 128-bit vectors | `301b8ef4282fe10d` | `9e08ac037896bd5c` | 28 | 11 445 on AMD, 11 711 on Intel | 42 to 51 on AMD, 16 to 22 on Intel |
   | No optimization passes | `e3a283950520fad1` | `1109dde3338ef346` | 854 | 0 | 2.2 to 3.6 |
   | SSE2, 128-bit vectors, no optimization passes | `dabd2f4bbb735ce5` | `b545d8980935af51` | 842 | as at SSE2 | 3.9 to 5.9 |

   - At every setting the hosts with an AMD processor draw one picture and the hosts with an Intel processor another, each pixel apart by one level per channel. No setting of the device makes the two alike. Within a vendor every host draws the same bytes at every setting it ran: three AMD models, one without AVX-512 and two with it, and three Intel models down to SSE2, two models of each with no SSE stated; with the runs of 2026-11-12 the unheld picture is one on four AMD models (the developer machine's Ryzen 9 9950X3D among them) and one on four Intel models.
   - Neither the vector width, nor the capabilities, nor the optimization passes are where the hosts part: the EPYC 9V45, the EPYC 9V74 and the three Intel models report the same capabilities to the device, and holding all hosts to SSE2 leaves 842 pixels apart.
   - **The reciprocal square root estimate.** The code Mesa generates for this scene, disassembled in the container on the developer machine (`GALLIVM_DEBUG=asm`), holds the processor's reciprocal square root estimate instruction at every level that states SSE: 180 times at the host's capabilities and 360 times at SSE2 (Mesa 25.0.7 and LLVM 19.1.7 there, which draw the cell's bytes). Mesa's sources emit it wherever the device knows of SSE or AVX and state its precision as about ten bits. The instruction's results are the processor's own: measured over 2^20 inputs, the AMD hosts give one set of results (hash `54628c0e06333525`, `3f350000` for 2) and the Intel hosts another (`ce66ce7a5b1f9b25`, `3f34f800` for 2), each the same on the two models of the vendor the eight runs landed on, and the developer machine's AMD processor gives the AMD results.
   - With no SSE stated the generated code holds no estimate instruction (it computes on the x87 stack) and the two vendors come within 28 pixels of each other, in another part of the picture; they are still not alike, and the cause of those 28 pixels was not found. That setting costs 7 times a render on the Intel hosts and 19 to 23 times on the AMD hosts, and draws a picture 11 445 to 11 711 pixels away from the unheld one, by up to 4 levels.
   - `GALLIUM_OVERRIDE_CPU_CAPS` is not a variable the key reads (it reads the `LP_` and `GALLIVM_` variables and the cache switch): a capture under a held capability carries the key of an unheld one. Had a level made the hosts alike, the key would have had to state it.
   - The carried references behave as on 2026-11-12: on the nine AMD hosts the cell's reference compares equal, on the five Intel hosts the byte comparison fails under the reference's own key.

Not measured here: a content hash delivered by a renderer; a hardware device; the Windows cells and the container on a host with an Intel processor; the cause of the 28 pixels that stay apart with no SSE stated; a processor of a third vendor; whether a processor's name changes the generated code where the estimate is absent; a target the format cannot hold, a float target among them; the weight of a picture of a game; the review of the pending parts by a tool; and eight weeks of keys.

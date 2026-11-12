# Visual reference round trip

Throwaway spike of the game testing research. It lays a visual reference out as reviewed files (a content part that is the same on every device, a byte part per device key, a pixel file), carries those files between jobs and between cells, runs the three comparisons of a visual test against them (content equality, byte equality, a bounded difference) with the cause each one reports, and measures what a reference costs.

It stands on the [device baseline spike](../render-device-baseline/README.md): its filtering and blending scene, its native path to Direct3D 12, Vulkan and Metal, and its PNG writer. It measures nothing of a renderer. Its content hash is a stand-in, the hash of the draw list the spike's own code builds, so it cannot show that a real renderer's record lacks something. It is evidence, never product code.

## Layout

- `src/frame.rs`: the frame as the spike's own draw list (one view of three passes, the device baseline's ten draws plus one triangle that lies beyond the view), its content hash with a part per view, per pass and per asset, the hash of each asset's payload, the changes brought about on purpose, and the adapter that turns the list into what the device baseline's native path draws.
- `src/reference.rs`: the parts as text files and a set as a directory.
- `src/comparison.rs`: the three comparisons, the facts that make a cause, the pending parts and the group a review puts them in.
- `src/pixels.rs`: the pixel file, its strict reader, the numbers of a difference, the independent decoder and the repository's packed form.
- `src/main.rs`: the measurements and the report.
- `tools/system-drawing.ps1`: the independent decoder of the Windows cells.
- `setup.sh`: what a machine of the research CI needs installed (below).
- `references/from-<cell>/`: the reference set each cell wrote on its first run, accepted into the repository for the later runs.

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

## Method

The research CI builds and runs the binary on the five cells after `setup.sh`; locally it runs the same way (`ci/run-spike.sh visual-reference-round-trip <cell>`). On each cell the spike opens the software device of the platform's interface (Mesa's software device through Vulkan on Linux, the Windows software adapter through Direct3D 12), or failing one the device the virtual machine presents (Metal on the macOS cell), and captures the frame once.

**The round trip.** The spike reads every set under `references/` and its own cell's set whether or not it exists, as a run that takes references, and runs the three comparisons of its capture against each. On a cell's first run its own set does not exist: the three comparisons fail and leave a pending content part, a pending byte part and a pixel file under `out/<cell>/pending/from-<cell>/`. Those files are then taken from the run's artifacts and accepted into the repository as `references/from-<cell>/`, which is the review's accept. A later run of the research CI, on fresh runners, reads the five sets from its checkout.

**The causes.** The spike accepts its own capture as a set under its key, and the same parts under a key named otherwise, in a temporary directory. Each case then builds its frame with one option, captures it on a device created from scratch and compares it with one of the two sets. Eleven cases: nothing changed; the six of the second hypothesis, the texture encoded again taken twice (where a draw samples it, and where none does); and three rows of the table the hypothesis does not list (no reference, a target of another size, a device that draws nothing). A case is reported as designed when the verdicts of the three comparisons, the pending parts of the byte comparison and the review group are those of its row; where the row says "decided on its numbers", when the bounded difference was made against the base and its verdict is what the numbers give. The test's numbers are a largest difference of 16 per channel, a second of 4, and 1 024 pixels allowed beyond the second.

**The pixel file.** The frame is written, read back from disk by the strict reader, and decoded by the platform's independent decoder. The platform's tool writes the picture again, compressed; the decoder reads that file and the strict reader is asked to. One channel of one pixel is changed in a file kept in the writer's form, which is read and hashed.

**The weight and the costs.** At 256 by 256, 1 024 by 1 024 and 1 920 by 1 080 the frame and the frame with an item moved are written as pixel files; the first is measured as written, compressed in the file by the platform's tool, and as the pack `git pack-objects` writes for it alone and for the two revisions under one path. One child process per size and per setting then captures the frame five times on one device: as the run states it, at one and four device threads on Mesa's software device, and with the process restricted to one and four processors on the Windows software adapter, which documents no thread setting. Each repetition times the render to the completion of the device's work, the read-back, the hash of the bytes (64-bit FNV-1a, one byte at a time: a stand-in), the content hash, and the bounded difference of the picture against a copy a tenth of whose pixels differ.

**Departures from the specification**, each stated here and in the findings:

- The specification has each cell run two jobs in one run, the second reading the first's references from the run's artifacts. The research CI has one job per cell and its workflow was left as it is: the second job is a later run of the research CI, and the references reach it through the first run's artifacts accepted into the repository. The files still travel from one runner to a fresh one, and through the path a reference takes in a project (an artifact of a run, a review, a commit).
- "Each cause is a build option of the spike" is read as an option of the frame's build, chosen at run time in one binary, not as a compilation option.
- A run under another key is brought about in two ways: within a cell by accepting the cell's own part under a key named otherwise, and across cells by the sets the other cells wrote, whose keys are those of other devices.
- The device baseline spike's scene gained a target size of its own for the three sizes; at 256 by 256 it renders the bytes it rendered before.

`setup.sh` installs, on a Linux machine, the Vulkan loader, Mesa's Vulkan drivers, the GLSL compiler, Netpbm and git from the distribution's packages; on macOS, Netpbm from Homebrew; on Windows nothing.

The self-checks (`cargo test -p visual-reference-round-trip`) verify the frame's content parts under each change, the text form of the parts, the three comparisons on each row of the table with pictures made by hand, the strict reader, and the numbers of a difference.

## Environment

The five cells of the research CI; the `rust:1.99.1-slim-trixie` container on the Linux x86_64 cell and on a developer machine (Windows 11, under its Linux subsystem) for the sixth hypothesis.

No third-party library is linked or vendored. The independent decoders are tools started as processes: on Linux and macOS `pngtopam` and `pamtopng` of Netpbm (the distribution's package states for both the permissive notice it labels MIT: permission to use, copy, modify and distribute for any purpose and without fee, provided the notices are kept), which read and write with libpng (the libpng license) and zlib (the zlib license); on Windows the PNG codec of GDI+ reached through `System.Drawing` in Windows PowerShell, both parts of Windows. Mesa (MIT license) and the glslang command-line tool (BSD and Apache 2.0 licenses) are those of the device baseline spike; git (GPL 2.0) is run as a tool to write a pack.

## Result

Not measured yet.

# Audio forms and threads

Throwaway spike of the audio research. It answers one question: with one mixer fed by commands derived from the simulation's state, pulled either by a device or by the host after each tick, what audio output do the cells offer, which threads does an output library create, does the offline form give a count of frames that depends on the ticks alone and samples that two runs reproduce, do the null, offline and device forms leave the simulation's hashes alone, and what has a process loaded when it takes the null form?

## Hypotheses

1. **The cells' audio facts.** On each cell: whether a default output device exists, what building and starting a stream returns, and whether samples are asked for. Recorded as facts.
2. **The thread set.** The process's thread count before the output library is touched, with its host, with a stream built, with the stream running, after the stream is dropped and after the host is dropped. Recorded per cell, to be compared with the threads the library's source creates on each platform.
3. **The offline count is a function of the ticks.** For 600 ticks at five combinations of rate and step, two of which do not divide (a ninetieth of a second at 48 000 hertz, a sixty-fourth at 44 100), the frames of a tick are the floor of the rate times the elapsed time at its end minus the same at its start; the total is the floor of the rate times the run's length; the mixer's position after each tick is that figure; a run resumed at tick 300 mixes the same frames per tick. Refuted by one frame.
4. **Offline samples are reproducible on one machine.** Two runs give identical bytes, with voices from a triangle wave in exact arithmetic and with voices from the platform's sine function, each with one streamed sound decoded as the mixer pulls. Refuted by one differing sample. The hashes are also compared across cells, with nothing promised.
5. **The forms do not touch the simulation.** The reference workload's hash at every tick is the same alone, with a sink in the null form, with the offline form mixing after every tick and, where a stream runs, with the device form. Refuted by one differing hash.
6. **The null form loads nothing.** A child process that never calls the output library reports the modules it has loaded whose name marks an audio stack. Refuted by one such module.
7. **The hand-off does not allocate on the mixer's thread.** A counting allocator reports what is allocated or released while a thread is inside the mixer, in the offline form and in the device callback. Refuted by one.

Recorded without a threshold: the scheduling of the callback thread, the frames per callback, the lead the library predicts between a callback and its playback, and the mixer's time for a buffer of 512 frames with 64 voices.

## Method

- `src/lib.rs` holds the model: a ring between one producer and one consumer that neither waits nor allocates after creation; a mixer with 64 fixed voices that drains the ring at the start of a buffer and applies each command at its target sample inside the buffer, with linear ramps for gain and pan; the offline count; the allocator; and the extraction, which reads one emitter per entity of the reference workload's first sixteen slots (gain from its speed, pan from its position) and writes nothing.
- After each tick, the difference between the previous and the current extracted state becomes commands (start, ramp to a target, stop). A script adds a one-shot sound every ten ticks and one streamed sound, whose samples a stand-in decoder produces in integer arithmetic: synchronously with the pull in the offline form, from a decoding thread through a ring in the device form.
- The parent process never calls the output library. It measures the offline form and the hashes, then starts two children of itself: one that takes the null form, one that opens the default output device through cpal 0.18.2 (Apache-2.0), the library being the subject of the device and thread measurements, steps the simulation for 90 paced ticks and hands its changes to the stream's mixer with a target fifty milliseconds ahead.
- Threads, loaded modules and the calling thread's scheduling come from the operating system without a third-party crate (`src/sys.rs`).
- On Linux the output library links the ALSA library, which `setup.sh` installs; the script also prints the sound devices and the audio daemons it finds.

`cargo test` runs the self-check: the frames of a tick sum to the floor of the elapsed time, a command is applied at its sample inside a buffer, and the ring gives back what it was given and refuses when full.

## Environment

The five cells of the research CI (Windows Server 2025 and Windows 11 ARM64, Ubuntu 24.04 on x86_64 and ARM64, macOS 15 on Apple silicon), Rust 1.99.1, release profile; one developer machine (Windows 11, x86_64) with an audio device, for a local run.

## Result

Measured on 2026-11-11 on the developer machine and on the five cells of the research CI.

On the developer machine every check with a threshold holds except the sixth hypothesis, which is refuted:

- The offline count is exact at the five combinations, the two that do not divide included (533 or 534 frames per tick; 689 or 690), and a resumed run agrees.
- Two offline runs give the same samples with both kinds of voice, and the mixer allocates nothing.
- The simulation's hashes are the reference's alone, in the null form, in the offline form and in the device form.
- **The null form loads an audio module**: a process that never calls the output library has the system's audio endpoint library (`mmdevapi.dll`) loaded, because the executable imports it through the output library it links.
- The device form: the WASAPI host, a default device at 48 000 hertz in two channels of 32-bit floats; the stream asks for 480 to 1 056 frames per callback and predicts 32 milliseconds between a callback and its playback. The process goes from 5 threads to 13 when the stream is built, and 12 remain after the stream and the host are dropped. The callback thread runs at the normal priority. Nothing is allocated in the mixer and the streamed sound never underruns.
- The mixer takes 29 microseconds for a buffer of 512 frames with 64 voices, 0.3 percent of the buffer's duration.

On the five cells (research CI run 43019340212) every check with a threshold holds:

- **The offline count** is exact at the five combinations on every cell, and the simulation's hashes are the reference's alone, in the null form and in the offline form (final hash `0x83e0403cfdd589e8` everywhere).
- **Offline samples across cells.** Two runs agree on every cell. With the triangle in exact arithmetic the 480 000 frames have one hash on the five cells and the developer machine (`0x84b047f535b60c3e`). With the platform's sine function they have three: the two Linux cells and Windows on ARM64 together, macOS, and Windows on x86_64 alone.
- **The null form loads an audio library on every cell**: the ALSA library on Linux, the system's audio endpoint library on Windows, the Core Audio and Audio Toolbox frameworks and five more on macOS. The executable imports them through the output library it links, whether or not it ever calls it. On Linux a binary that links the ALSA library does not start where that library is absent, which is why `setup.sh` installs it.
- **What the cells offer.** Only the macOS cell has an output device. On the Linux cells there is no sound device and no audio daemon; the library's ALSA host names a default device, asking its configuration fails with a device-not-available error, no thread is created, and the ALSA library prints its own messages on the standard error stream. On the Windows cells the library finds no default output device and creates no thread.
- **The device form on the macOS cell**: Core Audio, 48 000 hertz in two channels of 32-bit floats, 512 frames per callback, 12.75 milliseconds predicted between a callback and its playback. The process goes from 2 threads to 10 with the stream built and 11 with it running; 7 remain after the stream and the host are dropped. The callback thread runs under the round-robin real-time policy at priority 63. The mixer allocates nothing and the hashes are the reference's.
- The mixer takes 57 to 85 microseconds for a buffer of 512 frames with 64 voices on the cells, 0.5 to 0.8 percent of the buffer's duration.

The thread set of a device stream was therefore measured on two platforms only: macOS on the cell and Windows on the developer machine. Linux with a sound device was not measured.

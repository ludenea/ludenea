# Windowed presentation

Throwaway spike of the rendering research. It opens a window through each of [winit](https://github.com/rust-windowing/winit) and [SDL3](https://www.libsdl.org/), and presents a swapchain to it from a thread that is not the platform thread: natively on Direct3D 12, Vulkan and Metal, each reached with declarations written by hand, and through the surface of [wgpu](https://github.com/gfx-rs/wgpu). While frames are presented the platform thread pumps the library's events and the window is moved, resized, minimized and occluded.

It is evidence, never product code. winit, SDL3 and wgpu are candidates under study: each is linked inside this spike only, and nothing here adopts one.

## Layout

- `src/main.rs`, `src/plan.rs`: the plan of a machine and the report. Each measurement runs in a child process, so that a deadlock, a crash or a library's threads stay in their own process.
- `src/host.rs`, `src/host_winit.rs`, `src/host_sdl.rs`: what the spike asks of a windowing library, said once: a window, its native handles, a pump the host calls from its own loop, and the time each library gives an event.
- `src/presenter.rs`, `src/d3d12.rs`, `src/vulkan.rs`, `src/metal.rs`, `src/wgpu_surface.rs`: what the spike asks of a presentation path, said once, and the four paths.
- `src/soak.rs`, `src/gestures.rs`, `src/scenario.rs`: frames presented while the window lives through gestures, with the watchdog that calls a deadlock.
- `src/pace.rs`: a fixed-step loop alone, beside a presentation thread, and presenting itself.
- `src/thread_steps.rs`, `src/threads.rs`: the threads of the process at each step, from the system's list.
- `src/events.rs`: an injected key and the time each library gives it.
- `src/modal.rs`: the host's loop while Windows moves or resizes the window.
- `src/win32.rs`: the gestures injected on Windows, the observation of the system's own loop, the facts of the display.
- `src/bin/`: two probes that link one windowing library each, for what a library does with no display and for what a binary that links it imports.
- `setup.sh`: what a Linux machine of the research CI needs installed.

## What a frame is here

A frame clears the swapchain's image to a color that changes with the frame's number, and presents it: a clear command on Direct3D 12 and Vulkan, a render pass that draws nothing on Metal and through wgpu. No shader is involved. Each turn of the presenter also draws one frame of an **offscreen stream** on the same device (a clear of a target of 256 by 256, then a wait for the device to finish it), which stands for the offscreen worlds one presenter serves beside a window.

Four times are taken per turn: the **acquisition** of the swapchain's image, the recording and **submission** of the frame's commands, the **wait for the device** where a path waits for commands to finish, and the **presentation** call. An acquisition that gives nothing is an outcome, not an error.

Three presentation modes are asked where a path offers them: **synchronized** (in step with the display, frames queued), **latest** (in step with the display, the latest frame replacing a waiting one) and **unsynchronized**.

| Path | Synchronized | Latest | Unsynchronized |
| --- | --- | --- | --- |
| Direct3D 12 | sync interval 1 | sync interval 0 | sync interval 0 with tearing allowed |
| Vulkan | first in first out | mailbox | immediate |
| Metal | display sync on | not offered | display sync off |
| wgpu | `Fifo` | `Mailbox` | `Immediate` |

## Hypotheses

Stated before the first recorded run. A hypothesis with no threshold is a figure to record.

1. On the three platforms, and on Linux under each of its two display systems, a presentation thread presents to a windowed and to a borderless swapchain for ten thousand frames while the platform thread pumps events and the window is moved, resized, minimized and occluded, with no deadlock and no error other than an acquisition that gives nothing.
2. The time spent in acquisition and in presentation is recorded per interface and per presentation mode, with no threshold, and with it the delay an occluded window adds to an offscreen stream of the same device, served by the same presenter.
3. Each platform's presentation times are recorded where it gives them, and on a real display compared with its refresh, with no threshold.
4. The three hypotheses above hold through wgpu's surface as they do natively.
5. With the presenter served inside the platform thread's event loop, the same frames are presented with no deadlock.
6. The threads a driver, a software device and each windowing library create are counted at each step, with the processor time they take over an idle second; a set of threads is said to wait when it takes under one millisecond of that second.
7. A fixed-step loop at 60 ticks a second, beside a presentation thread that presents in step with a display at 60 hertz, keeps the 99th percentile of its ticks' lateness within the spread of a control loop that presents nothing, while the window is shown, occluded and minimized; the same loop presenting itself in that mode does not.
8. With no display each library returns an error and the process goes on, its binary importing no display library; under a compositor with no display each brings the windowed form up; where a Wayland connection fails beside a running X11 server, the host's second request brings it up there.
9. The delay between an event injected at the platform's level and the time each library gives it is recorded at pump periods of 1, 4 and 16 milliseconds, with no threshold, with whether each library lets the host reach the platform's own event.
10. During a move or a resize on Windows, whether each library returns to the host's loop, and with which of its remedies, is recorded, with no threshold.

## Method

**Threads.** The platform thread is the process's main thread: it starts the library, opens the window and pumps. The presenter runs on a thread named `presentation`, which creates the device and the swapchain itself, except on macOS, where a view takes its layer on the main thread and wgpu makes its surface there. The platform thread publishes the size of the window's drawable area; the presenter reads it at each turn and resizes its swapchain when it changed. The size is also published from the callback each library runs inside the system's own resize loop (winit's application handler, SDL's event watch), so that the swapchain is resized from the presentation thread while the platform thread is held in that loop.

**A deadlock** is a watchdog thread seeing no frame presented and no event pumped for five seconds. It ends the process with what is known. The same thread reports a **stall** when the presenter finishes no turn for twenty seconds while the platform thread still pumps: a call of the presenter that does not return is no deadlock by that definition, and no run either.

**Gestures on Windows** are produced without a person and without the keyboard focus. A move or a resize is started as the window's system menu starts it, by the system command posted to the window: that enters the system's own loop, which a hook on the platform thread's window procedures observes (the two messages that mark its entry and its exit). The arrow keys that drive the loop and the return key that ends it are posted to the window's thread, so that nothing is sent to whatever program has the keyboard; the system moves the pointer to the window during the gesture, and the spike puts it back. A window moved by a call never enters that loop, which is why no call is used. The window is minimized by the system command its caption's button posts, and restored by a call that does not give it the focus; it is occluded by a plain window of the spike's own shown over the whole of it, above it. A borderless window that covers its display is minimized and occluded, and neither moved nor resized. A run "for ten thousand frames" goes on until that many frames are presented and one cycle of gestures is complete; a long run lives through a cycle every fifteen seconds.

**Gestures elsewhere.** On the Linux cells the window is resized by two calls of the library and nothing else: a virtual display has no window manager to minimize or occlude a window, and under a compositor a client cannot move itself. On macOS the window is resized and minimized by calls.

**The window** is 640 by 360, placed at the top left, above the other windows, and shown without taking the keyboard focus, so that a run on a machine in use does not take the keys of the person at work. With winit the borderless window is not asked to stay above the others: DXGI refuses a swapchain for a winit window that is both full screen and above the others, which the first runs showed on three devices. The run that injects input of the system asks for the focus, since that input goes to the window that has it.

**The pace.** A stepping thread runs a loop at 60 ticks a second: it sleeps until a tick's due instant, takes the tick's lateness (its start minus its due instant), and does a fixed computation. Four loops are compared, for twenty seconds each after one second that is not counted: the control presents nothing; beside a presentation thread, the loop hands each frame to that thread through one place, and makes no frame while the place is taken; presenting itself, the loop presents each frame in the synchronized mode before it sleeps, unless the next tick is already due; and the same with the swapchain's queue of frames filled first by four frames presented back to back, the state a loop that outruns the display reaches. The four run in each of three states of the window (shown, occluded, minimized) and in three rounds, their order turning with the round. Twenty seconds is one full beat of 60 ticks a second against a display of 59.95 hertz.

The comparison is defined here, before the run. Per state of the window, the control's **spread** is the interval between the smallest and the largest 99th percentile of its three rounds. A loop is **within the control** when the median of its rounds' 99th percentiles is not above the control's largest, and **beyond** it otherwise. The gain of a presentation thread is the median 99th percentile of the loop presenting itself minus that of the loop beside a presentation thread; it **exceeds the spread** when it is larger than the width of that interval. Each segment also records how busy the machine was (its processor time that was not idle), since these figures are judged on a machine at rest.

**Threads** are read from the system's list of the process's threads at each step: the library started, a window opened, the device and the swapchain created, 120 frames presented, each released. A thread belongs to the step at which it was first seen. After a step, the measuring thread sleeps one second and the processor time each set of threads took meanwhile is read: per-thread cycle counts converted to time on Windows, whose own thread times advance by scheduler ticks; the scheduler's run time on Linux; the thread's user and system time on macOS. The measuring thread itself is left out.

**Injected events.** A key that no keyboard carries (the twenty-fourth function key) is injected two ways: posted to the window's thread as a key message, which needs no focus, and as input of the system, which is delivered to the window that has the keyboard focus and is therefore sent only when that window is the spike's. The host pumps once per period. Each injection is matched with the key event the library delivers; the time the library gives the event is placed on the spike's clock (SDL's clock is read against it at start), and a hook beneath each library records the platform's own message with its time.

**The system's loop and the host's loop.** A window is moved, then resized, for about 0.9 seconds each, while the host's loop records each of its passes: winit's pump and SDL's poll called from a loop the host owns, and SDL's main callbacks, where the library owns the loop. What each library calls meanwhile is counted: winit's application handler, SDL's event watch.

**With no display** (Linux cells), each probe is run with no display variable, under a virtual X11 display, under a Wayland compositor that needs none, and with a Wayland display that does not exist beside the virtual X11 display. A probe that fails its first request makes a second one for X11 in the same process; if both fail, a new process that names X11 from its start is run.

**The synchronized mode, three more ways.** The runs that time each mode use winit. The synchronized mode is also run with SDL3, and with the presenter inside the platform thread's loop, on the first device of each path. On the Linux cells, under the Wayland compositor, it is run four more times with no gesture and with the Wayland protocol the process speaks logged (`WAYLAND_DEBUG`), the presenter on each of its two places with each library; the last lines of that log are kept with the run.

The research CI builds and runs the binary on the five cells after `setup.sh`; locally it runs the same way (`ci/run-spike.sh render-windowed-presentation <cell>`). A machine with a real display adds the pace and the long synchronized runs:

```bash
cargo run --release -p render-windowed-presentation -- <cell> --with-display > out/<cell>/facts.json
```

`--only <section>` runs one section of the plan (`probes`, `threads`, `modal`, `events`, `soak`, `fallback`, `modes`, `long`, `pace`), and a single measurement runs as a child, for example with the gestures left to a person for thirty seconds:

```bash
cargo run --release -p render-windowed-presentation -- --child soak library=sdl3 path=direct3d12 device=0 mode=synchronized frames=3000 gestures=hand
```

The self-checks (`cargo test -p render-windowed-presentation`) verify the report's JSON, the percentile, the reading of an executable's imports, and that a thread started after the ledger belongs to the next step and waits while parked.

## Where the spike departs from its specification

- **On a machine with a display the gestures are injected, as on the cells, and not made by a person.** The specification has a person move the window there. Injection through the system's own loop is repeatable and needs nobody; a run with `gestures=hand` leaves the window to a person.
- **Ten thousand frames are presented in the fastest mode each surface offers** for every library, path, device and window form, and in the synchronized mode for one device per path, windowed, on the machine with a display; the other modes are timed on runs of 1 200 and 3 000 frames. In step with a display of 60 hertz ten thousand frames last 167 seconds, which the whole matrix would multiply by forty, half of it with a window covering the display of a machine in use.
- **The offscreen stream is a second target of the same device**, drawn by the same presenter between two frames of the window, and not a second world.
- **The pace is measured natively on Direct3D 12 and on Vulkan**, with winit, and not through wgpu's surface nor with SDL3: the hypothesis is about the thread that presents.
- **On Linux and on macOS no input is injected**: the injected events and the system's move and resize loop are measured on Windows only.
- **A real gamepad is not read.** SDL's gamepad subsystem is started and its threads counted; the device layer's reading of a gamepad's own event time needs a person at a gamepad.
- **The spike does not stand on the device baseline spike's crate**, whose native modules are private to it: it carries its own declarations for the swapchains and copies of that spike's loader and of its reader of an executable's imports.

## Environment

- The five cells of the research CI: no hardware graphics device, no real display, no person. The Windows and macOS runners are expected to hold a desktop session; the Linux cells are given a virtual X11 display (`Xvfb`) and a Wayland compositor that needs no display (`weston` with its headless backend), both started by the spike. Ubuntu 24.04's packages give them Mesa 25.2.8 for the software Vulkan device, weston 13.0.0 and Xvfb 21.1.12.
- One machine with a display, a hardware device and nobody at the window: a Windows 11 machine with an NVIDIA GeForce RTX 5080 that drives the display, an AMD Radeon integrated device and the Windows software adapter. No such machine is named for macOS nor for Linux under a real compositor: what the hypotheses ask there is listed as not measured.
- Rust 1.99.1, release profile. winit 0.30.13 (Apache 2.0), with its default features. SDL 3.4.20 (zlib license) through the `sdl3-sys` crate 0.7.3 (zlib license), which builds SDL from its sources and links it statically, with its video, joystick, HIDAPI and audio subsystems. wgpu 30.0.1 (MIT or Apache 2.0) at its safe level. Each license was read before the first run.

## Result

Measured on 2026-11-12.

- **The five cells of the research CI**: [the last run](https://github.com/ludenea/ludenea/actions/runs/43324327521), whose summary holds every table, after six runs that found and narrowed what follows ([the first on five cells](https://github.com/ludenea/ludenea/actions/runs/43306968723), whose macOS cell did not build, and [the macOS cell alone](https://github.com/ludenea/ludenea/actions/runs/43307870351); then the Linux cells run again, two or three at a time: [second](https://github.com/ludenea/ludenea/actions/runs/43309608136), [third](https://github.com/ludenea/ludenea/actions/runs/43311456731), [fourth](https://github.com/ludenea/ludenea/actions/runs/43313374124), [fifth](https://github.com/ludenea/ludenea/actions/runs/43315337853)). Fourteen runs of a Linux cell in all.
- **The machine with a display**, twice in full. The first run overlapped another spike's measurement on the same machine for its first sixteen minutes of forty-seven (the pace, measured last, began after it). The second ran with the machine at rest: no other measurement and no compiler in 102 samples taken every thirty seconds, the machine 3.1 to 4.4 percent busy during the pace. The figures below are the second run's, and the first run's are given beside them where a threshold is judged. Four direct runs, at rest, read the presentation times again after the reading of the system's statistics was corrected. The display runs at 59.95 hertz (a refresh period of 16 680.9 microseconds, as the compositor reports it).

A hypothesis is said to hold, to be refuted, or to be recorded where it has no threshold. What could not be measured is listed at the end.

### A presentation thread through the gestures

**Holds on Windows, on macOS and on Linux under X11. Under the Wayland compositor it holds in the modes that do not wait for the display and is refuted in the synchronized mode**, where the presentation call does not return.

- **Windows, with a display**: 40 runs of 40 complete with no deadlock and no error (two libraries; Direct3D 12 natively and through wgpu on the NVIDIA device, the AMD device and the software adapter, Vulkan natively and through wgpu on the two hardware devices; windowed and borderless), each of ten thousand frames or more in 1.7 to 12.4 seconds through a move, a resize, a minimization and an occlusion. The 40 moves and resizes of the windowed runs all entered the system's own loop and left it at the return key. The swapchain was resized 1 to 13 times per run from the presentation thread while the platform thread was held inside that loop. In the synchronized mode, four runs of ten thousand frames (163 to 172 seconds, ten cycles of gestures each) complete as well, one per path on the NVIDIA device.
- **Windows cells** (the software adapter, a display mode of 1 024 by 768 at 64 hertz): the same runs complete on both cells, and the injected move and resize enter the system's loop on a hosted runner, which the method had as a hypothesis.
- **macOS cell** (the runner's virtual device, a desktop session): 8 runs of 8 complete through Metal natively and through wgpu, windowed and borderless, ten thousand frames in 17 to 28 seconds over its two runs, through a resize and a minimization made by calls. The layer is given to the view, and wgpu's surface is made, on the main thread; the device, the queue and every frame are the presentation thread's.
- **Linux cells under the virtual X11 display**: every run completes on both cells, in every mode. The display has no refresh to wait for: the synchronized mode presents five to six thousand frames a second there.
- **Linux cells under the Wayland compositor with no display**: every run of ten thousand frames completes in the unsynchronized mode (3 seconds windowed, 13 seconds borderless). **In the synchronized mode the presenter stops inside the presentation call.** The protocol the process spoke shows why: after the first frame is committed with its frame callback, the compositor never answers that callback, and the first in first out mode of the Vulkan surface (Mesa 25.2.8) waits for it before it presents again. The compositor (weston 13) lists neither of the two protocols that let a client present without that wait. The platform thread goes on pumping, so this is a stall and no deadlock by the spike's definition. Over the fourteen runs of a Linux cell, counted per run of that mode: with a presentation thread and the window resized by calls during the run, natively with winit 28 runs of 28 complete, natively with SDL3 2 of 9, and through wgpu 2 of 23 (2 of 14 with winit, none of 9 with SDL3); with no gesture at all none of 6 completes, natively with either library. In the unsynchronized and latest modes 140 runs of 140 complete there. Where a run completes, its frames come 25 milliseconds apart.
- **A fact of the spike's own window, on Windows**: DXGI refuses a swapchain (`DXGI_ERROR_INVALID_CALL`) for a winit window that is both borderless full screen and asked to stay above the others, on the three devices and on both cells; the AMD device's Vulkan driver fails on it too, and the NVIDIA one does not. The same window at the ordinary level takes a swapchain everywhere, as SDL's window does at either level. The spike no longer asks that level of a window that covers its display.

### Where the time of a turn goes, and what a window that is not shown costs an offscreen stream

**Recorded.** In the synchronized mode on the machine with a display, medians in microseconds over the turns in which the window is shown:

| Path | Device | Acquisition | Submission | Wait for the device | Presentation call | Offscreen frame | Offscreen stream's period |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Direct3D 12 | NVIDIA | 0.5 | 113 | 0.4 | 216 | 16 314 | 16 680 |
| Direct3D 12 | AMD | 0.4 | 53 | 0.3 | 88 | 16 529 | 16 692 |
| Direct3D 12 | software adapter | 0.5 | 58 | 0.3 | 61 | 16 553 | 16 682 |
| Vulkan | NVIDIA | 2.9 | 22 | 16 210 | 55 | 349 | 16 682 |
| Vulkan | AMD | 39 | 17 | 16 332 | 102 | 156 | 16 680 |
| Vulkan, the acquisition allowed to wait | AMD | 15 333 | 53 | 985 | 173 | 135 | 16 683 |
| wgpu over Direct3D 12 | NVIDIA | 16 167 | 103 | none | 171 | 141 | 16 681 |
| wgpu over Vulkan | NVIDIA | 15 924 | 89 | none | 97 | 314 | 16 679 |

- **Which call waits for the display differs on every path.** Natively on Direct3D 12 no call of the frame waits: the presentation call returns in 0.06 to 0.2 millisecond and the queue itself is held, so that the next work submitted to it, here the offscreen stream's frame, completes one refresh later. Natively on Vulkan neither the acquisition nor the presentation call waits on the NVIDIA driver, whatever limit the acquisition is given: the wait is on the semaphore the acquisition signals, which the spike sees as the wait for the frame's commands; on the AMD driver an acquisition allowed to wait takes the wait itself. Through wgpu, on both interfaces, the acquisition waits. On the Windows cells the same holds on the software adapter: natively the offscreen frame takes the refresh (15.8 milliseconds on the x86_64 cell, 29.8 on the ARM64 cell, where a frame is shown every other refresh), and through wgpu the acquisition does.
- **A presenter that serves a synchronized window serves its offscreen stream at the display's pace**, one frame per refresh (16.68 milliseconds), on every path. The longest period of that stream in a run is 17 to 66 milliseconds while the window is shown: one to four refreshes.
- **Covering the window changes nothing on Windows**: a window wholly covered by another is still presented to, with the same times, on every path and device. No presentation call returned the status that says a window is occluded.
- **Minimizing it changes nothing on Direct3D 12**, natively or through wgpu: frames are still presented at the display's pace. **On Vulkan a minimized window has no extent and gives no image**, and the offscreen stream then runs free (a period of 48 to 69 microseconds); through wgpu over Vulkan the acquisition answers that the surface is outdated a few times and presentation goes on.
- **The latest and unsynchronized modes wait for nothing**: a turn takes 0.1 to 0.3 millisecond on the NVIDIA device. The AMD device's Vulkan driver does not offer the latest mode.
- **macOS cell**: in the synchronized mode the wait is in the acquisition of the layer's drawable (about 15 milliseconds at the median, natively and through wgpu). With the window minimized Metal natively still gives drawables and presents (a median acquisition of 11 to 13 milliseconds, the longest period of the offscreen stream 32 to 73 milliseconds over the two runs); wgpu answers that the window is occluded at once and presents nothing. The wait of up to one second that the layer may impose was not seen on the runner's virtual device.
- **Linux cells**: under X11 nothing waits. Under the Wayland compositor with no display the wait of the synchronized mode has no bound, as above.
- **What each library says of the window's state**: on Windows SDL3 delivers a minimized and a restored event in every run and winit delivers neither (it has a call that asks); neither library reports a window covered by another as occluded. On the macOS cell both report the minimized window as occluded, and SDL3 as minimized too.

### Presentation times

**Recorded where a platform gives them, which only Direct3D 12 does in what the spike reads.**

- **DXGI's frame statistics**, read after each synchronized presentation, give the refresh at which the last frame was shown and the performance counter at a refresh. Over ten thousand frames on the NVIDIA device, 9 979 readings of 10 000 are given (21 refused as disjoint), 8 607 pairs of readings give a period, and its median is 16 680.9 microseconds (16 679.6 to 16 684.2): the compositor's refresh period to the tenth of a microsecond. The same holds on the AMD device and on the software adapter over 1 200 frames. From one presentation to the next the shown frame advances by one refresh in 73 percent of the readings, by none in 14 percent and by two in 13 percent: a reading taken right after the call may still describe the frame before. The first readings of a run are zero, and the count breaks once at its start.
- **Vulkan**: the NVIDIA driver lists the presentation timing extension and the AMD driver none; the spike calls neither. **Metal** gives a drawable's presented time to a handler block, which the spike does not register. **wgpu's surface** gives no time at which a frame was shown.

### Through wgpu's surface

**Holds as natively, with five differences.**

- Presentation from a thread that is not the platform thread completes wherever the native path does, and stalls where the native path does (the synchronized mode under the Wayland compositor).
- The acquisition is where wgpu waits for the display, on every interface.
- A minimized window is answered differently per interface: frames go on over Direct3D 12, "outdated" over Vulkan, "occluded" over Metal.
- A surface wgpu could not configure is reported as an error of the device, and the next acquisition panics if nobody read it.
- On macOS the surface was made on the main thread; making it on the presentation thread was not tried.

### The presenter inside the platform thread's loop

**Holds where the presentation thread holds.** On the machine with a display 8 runs of 8 present ten thousand frames with no deadlock, and the synchronized mode completes on the four paths; the cells agree. During a move or a resize the loop is held and no frame is presented, for 0.5 second here. **Under the Wayland compositor in the synchronized mode it is a deadlock at the first frame**, in 18 runs of 18, natively and through wgpu, with a gesture or with none: the same wait as above, with nothing left to pump the window.

### Threads

**Recorded; one set does not wait.** On the machine with a display, both runs agreeing:

| What starts | Threads that appear | Processor time over an idle second | Waits (under 1 000 microseconds) | Left after it is released |
| --- | --- | --- | --- | --- |
| winit | 0, then 3 when a window opens | 0 | yes | 3 |
| SDL3's video subsystem | 1, then 3 when a window opens | 0 | yes | 3 |
| SDL3's gamepad subsystem | 4 | 0 | yes | 3 |
| SDL3's audio subsystem | 1 | 0 | yes | 0 |
| Direct3D 12 device and swapchain, NVIDIA | 10, then 2 when frames are presented | 392 to 496 microseconds | yes | 1 |
| Direct3D 12 device and swapchain, AMD | 42, then 2 | 426 to 487 microseconds | yes | 1 |
| Direct3D 12 device and swapchain, software adapter | 0, then 3 | 0 | yes | 3 |
| Vulkan device and swapchain, NVIDIA | 23, then 2 | 1 055 to 1 440 microseconds | **no** | 6, and 1 more that appears as the device is released |
| Vulkan device and swapchain, AMD | 47, then 2 | 408 to 521 microseconds | yes | 6 |

- Through wgpu the counts are the interface's own: 10 and 42 over Direct3D 12, 23 and 47 over Vulkan.
- **The NVIDIA Vulkan driver's 23 threads take a millisecond or more of every idle second** with a swapchain and nothing to present (eight readings in two runs, all above the threshold), so by the spike's own figure that set does not wait.
- The threads of a window, of the gamepad subsystem and of a device stay after their owner is released, as counted above.
- **Cells** (their last run): Mesa's software device creates 12 threads under X11 and 10 under Wayland and releases them all; winit and SDL3 create none under X11 and one under Wayland; the Windows software adapter creates 1 to 3 when frames are presented; on the macOS cell winit and SDL3 create 2 or 3 threads when they start and 1 or 2 when a window opens, and Metal up to 3 when frames are presented. Every set of the cells waits in the last run; in the macOS cell's first run one set, SDL3's own after frames were presented, took 1 140 microseconds of an idle second.

### The pace of a loop beside a presentation thread

**Holds in part, and both runs are given because they disagree on where.** The 99th percentile of the lateness of the ticks, in microseconds, median of three rounds of twenty seconds; the control's rounds give its interval.

| Path | Window | Control's interval | Beside a presentation thread | Presenting itself, from an empty queue | Presenting itself, the queue full |
| --- | --- | --- | --- | --- | --- |
| Direct3D 12 | shown | 534 to 566 | 552, within | 558, within | 12 658, beyond |
| Direct3D 12 | covered | 544 to 562 | 537, within | 546, within | 12 581, beyond |
| Direct3D 12 | minimized | 547 to 569 | 547, within | 559, within | 12 571, beyond |
| Vulkan | shown | 540 to 564 | 548, within | 548, within | 12 956, beyond |
| Vulkan | covered | 538 to 549 | 562, beyond by 13 | 547, within | 12 732, beyond |
| Vulkan | minimized | 526 to 542 | 578, beyond by 35 | 557, beyond by 15 | 558, beyond by 16 |

- **Beside a presentation thread the loop keeps the control's lateness in four states of six in each run, and not the same four.** In the first run the two states beyond the control were Direct3D 12 minimized (by 14 microseconds) and Vulkan shown (by 8); in the second, Vulkan covered and minimized (by 13 and 35). The control's own interval is 9 to 44 microseconds wide. By the definition stated before the run the hypothesis is refuted in those states; the distances are of the size of the control's own spread, on a 99th percentile of about 550 microseconds.
- **A loop that presents itself is not late for twenty seconds from an empty queue**: the presentation call does not wait while the swapchain's queue has room, and the hypothesis that such a loop does not keep its pace is refuted for that span.
- **With the queue full it is late by 12.6 to 13.0 milliseconds at the 99th percentile, 2.6 to 5.0 at the median**, in every state in which a frame is presented, and the gain of a presentation thread (12.0 to 12.4 milliseconds) exceeds the control's spread several hundred times over. A loop at 60 ticks a second in front of a display at 59.95 hertz fills a queue of three frames in about a minute. With the window minimized on Vulkan there is nothing to present and nothing to wait for.
- The clause that would make the reason for a presentation thread fall (a gain not larger than the control's spread) applies to the first twenty seconds from an empty queue and not to the steady state.

### With no display

**Holds, with one difference between the libraries** (Linux cells).

- With no display variable, winit and SDL3 each return an error (winit: no display variable is set; SDL3: no available video device) and the process goes on. No binary imports a display library on Linux: the spike, the probe that links winit and the probe that links SDL3 import none, each library opening them at run time.
- Under the Wayland compositor with no display each library opens its window on Wayland, and under the virtual X11 display on X11.
- With a Wayland display that does not exist beside the running X11 server, **SDL3 opens its window on X11 at its first request. winit fails, and a second request in the same process is refused** ("EventLoop can't be recreated"), whatever it names: the host's second request needs a new process that names X11 from its start, which then opens the window.
- On Windows and on macOS the binaries import the system's windowing libraries (on Windows `user32.dll` and `gdi32.dll` for winit, and `winmm.dll` too for SDL3; on macOS AppKit, CoreGraphics and CoreVideo for winit, and Cocoa, CoreVideo, CoreAudio, AudioToolbox and GameController among others for SDL3).

### The time a library gives an injected event

**Recorded** (Windows).

- **winit gives an event no time.** The host receives a posted key 0.6, 2.2 and 11.9 milliseconds after its injection at the median, for pump periods of 1, 4 and 16 milliseconds (99th percentile 1.2, 4.2 and 16.2): the period itself.
- **SDL3 gives the time of the Windows message**, converted to its own clock, and not the time of the pump: it does not grow with the pump period. That time advances in steps of the system's tick count, 15 milliseconds on this machine and on both cells, so the time SDL3 gives is 15.5 milliseconds before the injection to 1.1 after it at a pump period of 1 millisecond.
- **Both libraries let the host reach the Windows message before it is translated**, winit through a hook its event loop's builder takes and SDL3 through a hook set by a call; the message's own time is the same tick count.
- Input of the system (a key injected as the keyboard's) was delivered and measured in the first run, 50 keys of 50 per library, with the same figures as a posted key; in the second run and on the cells the window did not have the keyboard focus and nothing was injected.

### The host's loop while Windows moves or resizes the window

**Recorded.** The system's loop held the platform thread 0.9 second per gesture.

| Library | Host's loop | Passes of the host's loop meanwhile | Longest time without one | What the library ran meanwhile |
| --- | --- | --- | --- | --- |
| winit | the host pumps | 0 | the whole gesture | its application handler: a moved event, and on a resize a resized and a redraw event per step |
| SDL3 | the host polls | 0 | the whole gesture | its event watch: an exposed event about every 16 milliseconds (57 in 0.9 second), and the moved and resized events |
| SDL3 | the library calls back | 57 | 31 milliseconds | the host's own iteration, about every 16 milliseconds |

Neither library returns to a loop the host owns. winit and SDL3's polling each offer a callback that runs inside the system's loop; only SDL3's main callbacks keep the host's iteration running, at the rate of a system timer.

### Not measured

- **macOS on a machine with a display and a hardware device, and Linux under a real compositor**: no such machine is named. The hypotheses on the gestures, on the pace, on the presentation times against a display's refresh and on a driver's threads are measured on Windows alone; what the macOS and Linux cells give is given above for a virtual device, a virtual display and a compositor with no display.
- **Whether a real Wayland compositor answers the frame callback of a window it shows**, which decides whether the stall of the synchronized mode is the headless compositor's or any compositor's for a window it does not show.
- **Gestures made by a person**: every gesture was injected or made by a call. On the Linux cells a move, a minimization and an occlusion were not run, and on the macOS cell a move and an occlusion.
- **Injected input and the time of an event on Linux and on macOS.**
- **A real gamepad**: none was attached (SDL3 lists none) and nobody was at one.
- **Ten thousand frames in the synchronized mode for every library, device and window form**: run for one device per path, windowed.
- **The pace through wgpu's surface and with SDL3**, and on the AMD device and the software adapter.
- **The presentation times Vulkan and Metal give**, and wgpu's surface made on the presentation thread on macOS.
- **An exclusive full screen**, which the spike never asks for.

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

- The five cells of the research CI: no hardware graphics device, no real display, no person. The Windows and macOS runners are expected to hold a desktop session; the Linux cells are given a virtual X11 display (`Xvfb`) and a Wayland compositor that needs no display (`weston` with its headless backend), both started by the spike.
- One machine with a display, a hardware device and nobody at the window: a Windows 11 machine with an NVIDIA GeForce RTX 5080 that drives the display, an AMD Radeon integrated device and the Windows software adapter. No such machine is named for macOS nor for Linux under a real compositor: what the hypotheses ask there is listed as not measured.
- Rust 1.99.1, release profile. winit 0.30.13 (Apache 2.0), with its default features. SDL 3.4.20 (zlib license) through the `sdl3-sys` crate 0.7.3 (zlib license), which builds SDL from its sources and links it statically, with its video, joystick, HIDAPI and audio subsystems. wgpu 30.0.1 (MIT or Apache 2.0) at its safe level. Each license was read before the first run.

## Result

Not measured yet.

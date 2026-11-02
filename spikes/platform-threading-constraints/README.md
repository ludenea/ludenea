# Platform threading constraints

Throwaway spike of the cross-platform compatibility research, requested by the multithreading research as the input its thread ownership rules wait for. It answers one question per platform of the engine's first compatibility boundary: which windowing, event loop and graphics device operations the platform binds to the main thread, which it allows from any thread, and which are unavailable on a machine without a display, as the research CI runners are.

## Hypotheses

1. On macOS, AppKit binds the application object, window creation and event polling to the main thread: a worker thread's attempt fails, is refused or crashes the process, while a Core Foundation run loop and the creation of a Metal device work on any thread.
2. On Windows, a window and its message queue belong to the thread that created the window: any thread, a worker included, may create windows and may post messages to another thread's window, but only the creating thread reads that queue and destroys the window; Direct3D 11 device creation and release are not bound to any thread.
3. On Linux, X11 imposes no main-thread rule: one connection serves several threads once `XInitThreads` has been called, and separate connections per thread need nothing; Wayland imposes none either. The hosted runners have no display server unless a virtual one (`Xvfb`) is started by the spike, and may have no Vulkan loader or device.
4. The hosted runners differ in their sessions: Linux has no display, Windows runs the job in an interactive session with a screen, macOS runs it in a logged-in window server session.

## Method

Every probe runs in a child process started by the spike itself with `--probe <name>`, so that an assertion, an abort or a hang inside a platform library is recorded as a fact (`crashed`, `panicked`, `timed out`) instead of ending the measurement; the child prints one JSON line with the outcome of the operation under test (`succeeded`, `failed`, `unavailable`, `unsupported`) and the facts it observed, and the parent records the outcome, the exit status, the duration and the tail of the child's standard error stream, where the platform libraries print their warnings.

The platform libraries are reached without any third-party crate: the Win32 and Direct3D 11 functions through their import libraries, AppKit, Metal, Core Graphics and Core Foundation through the Objective-C runtime and the frameworks, Xlib, the Wayland client library and the Vulkan loader through `dlopen`, so that their absence is recorded rather than fatal. On Linux, when no display is set and `Xvfb` is on the path, the spike starts a virtual X server on display `:99` for the probes that need one and stops it afterwards; the fact sheet records whether it was found and started.

The probes, per platform:

- Windows: screen metrics and remote session; an ordinary and a message-only window created on a worker thread; a window destroyed from a thread other than its creator; a message posted from another thread and the queue read from both threads; a message loop on a worker thread stopped by the main thread; a Direct3D 11 WARP device and a hardware device created on a worker thread and released from the main thread.
- macOS: the active displays and the window server session; the shared application created on the main thread and on a worker thread; a window created on the main thread and on a worker thread; the application's next event polled on the main thread and on a worker thread; a Core Foundation run loop on a worker thread; the default Metal device created on a worker thread and released on the main thread.
- Linux: the X11 and Wayland client libraries and servers; an X11 window created and mapped on a worker thread with its map event read there; one connection opened by a worker thread and read, then closed, by the main thread with `XInitThreads`; two threads with their own connections and windows without `XInitThreads`; a Wayland connection; a Vulkan instance and its physical devices created on a worker thread and destroyed from the main thread.

`cargo test` runs the self-check: the runner must classify a clean report, an abort, a hang and a panic correctly on the platform it runs on.

## Environment

The five cells of the research CI (Windows Server 2025 and Windows 11 ARM64, Ubuntu 24.04 on x86_64 and ARM64, macOS 15 on Apple silicon), Rust 1.99.1, release profile, default target features; the founder's machine (macOS, Apple silicon) for a local run with a display.

## Result

Not yet measured. The fact sheet of each cell is the `facts.json` artifact of the run; the summary table lists one row per probe and cell.

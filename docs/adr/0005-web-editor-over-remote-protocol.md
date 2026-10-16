# 0005. Build the editor as a TypeScript web application over LRP

- Status: Accepted
- Date: 2026-10-15

## Context

The editor must run on the desktop and in a browser. Native Rust user interface toolkits would need a second front end
for the browser, and the lack of an editor is the most cited weakness of
Rust engines.

Incumbent engines compile editor code into the engine, which couples their
release cycles and makes the editor impossible to replace. Prior art such as
the Defold editor API, the Bevy Remote Protocol and the Chrome DevTools
Protocol shows that a protocol between the engine and its tools works in
production.

## Decision

The editor is a web application written in TypeScript. One code base runs on
the desktop inside a WebView shell and in the browser.

The editor talks to the engine only through the Ludenea Remote Protocol
(LRP), using a TypeScript client generated from the protocol schemas. It has
no other access to engine internals. The viewport renders locally with
WebGPU.

## Consequences

- The editor is one LRP client among others: the command line, tests and AI
  agents get the same capabilities, so anything done in the editor can be
  scripted, tested and recorded.
- The editor is never compiled into the runtime, and a game never ships
  editor code.
- LRP must be complete and stable enough to build a full editor on it, which
  makes the protocol a first-class product with its own versioning rules.
- The project maintains two language ecosystems, Rust and TypeScript.

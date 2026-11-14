# Prototypes

Throwaway builds of the Ludenea research program, larger than a spike. A spike answers one engineering question with one measurement. A prototype is built when what must be checked is how several mechanisms of a design meet: it takes several claims of that design at once, each with its measurement and with what refutes it.

**A prototype is throwaway. It is never product code, and never a first version of the engine.** It is written to find out whether a design, built from its text alone, gives the states the text says. Once it has answered, it is left as it is or deleted. Nothing moves from a prototype into the engine without going through the architecture and implementation planning held in the project's steering documentation, and what the engine is built from is specified again for that purpose.

## Layout

- `<slug>/`: one prototype, named after what it builds. It is a Cargo workspace of its own, with its own lock file, outside the workspace of the spikes. Its README states in plain words what it measures, its hypothesis, its method, its environment and its result.

A prototype shares three things with the spikes and nothing else: the pinned toolchain ([`spikes/rust-toolchain.toml`](../spikes/rust-toolchain.toml)), the reference workload ([`spikes/reference-workload/`](../spikes/reference-workload/)) and the research CI. It may stand on the code of earlier spikes: it copies what it takes, says so in its README, and the spikes stay as measured.

## Building and running

A prototype is built and run like a spike, from the `spikes/` directory, so that the pinned toolchain applies to it:

```bash
cd spikes
ci/run-spike.sh asset-table-and-loader my-machine
```

The script builds the prototype in release mode (or in the Cargo profile named by `SPIKE_PROFILE`), runs it, and writes its facts and the facts about the machine under `out/my-machine/`. The prototype prints a short summary and writes its full log beside its facts. Its self-checks run with `cargo test --manifest-path ../prototypes/<slug>/Cargo.toml --workspace`, from `spikes/` as well.

The research CI ([`research-ci.yml`](../.github/workflows/research-ci.yml)) runs a prototype on demand on the five cells, by its name, exactly as it runs a spike:

```bash
gh workflow run research-ci.yml -f spike=asset-table-and-loader
gh workflow run research-ci.yml -f spike=asset-table-and-loader -f profile=dev
```

## Rules

- A prototype states its hypothesis and what refutes it before it runs.
- It uses the standard library and the reference workload crate. A third-party library enters only where it is the subject of a measurement; the README then names its version and its license. Copyleft code is never vendored.
- Prototypes are licensed like the rest of this repository: MIT or Apache 2.0, at your option.
- Measurements are published as CI artifacts. A prototype's README states its result; the interpretation of that result is not written in this repository.
- Prototypes, directories and READMEs name things in plain words; they carry no identifier of the steering documentation.

## Index

| Prototype | Research subject | What it builds and checks | Status |
| --- | --- | --- | --- |
| [`asset-table-and-loader`](asset-table-and-loader/) | Assets and content pipelines | A world's asset table with its wants, their owners and its loading gate, and a loading capability that resolves each world in the manifest it stands under, built from the design text alone and run against oracles written by hand from the same text: does the order at a tick's input fixed point give the stated states, does a want's owner order the table, does a named content set hold under a moved manifest, and is a saved world restored under the content at hand as the text says? | First stage measured on the five cells and on one machine on 2026-11-14; see its README |

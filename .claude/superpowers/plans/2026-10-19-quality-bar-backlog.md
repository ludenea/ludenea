# Ludenea quality-bar audit: execution backlog

Audited against the AGENTS.md quality bar and the project's ambition. 469 verified findings, deduplicated into 249 work items across 21 workstreams (20 P0, 53 P1, 103 P2, 73 P3); 4 findings left out with reasons.

## Verdict

Overall grade C. The deterministic core is genuinely strong (grade B): canonical hashing, thread-count independence, snapshot validation and replay decoding are proven on three operating systems, WebAssembly and Chromium, and survived the auditors' fuzzing. The product around it does not yet meet the quality bar. 20 P0 items remain: finite physics inputs and malformed undo histories crash the host, any oversized thread count panics every tool, game code panics kill the native game process, a failed Save silently loses scene edits, editor prefab operations write simulated values into the tick-0 scene, one throwing plugin unmounts the whole editor, the Explorer freezes at 100,000 entities, multi-entity edits apply only partly after playing, and the desktop app does not build on Windows. The editor (grade D) also breaks two of its own anti-goals: built-in plugins rely on private channels project plugins cannot reach, and command failures reach only the browser devtools. Execute the six guard-rail items first (gates, versioning policy, protocol fingerprint, public editor log, pinned hash shapes, CI dumps), then every P0, then the test harness that would have caught them (seeded fuzzing and random session sequences), then P1 by subsystem.

## Subsystem grades

Grades measure each subsystem against the quality bar today: A meets it, B has gaps that do not break it in ordinary use, C has quality-bar violations in main workflows, D has crashes, data loss or anti-goal breaches in main workflows, E is unusable.

| Workstream | Subsystem | Grade | Items | P0 | P1 |
| --- | --- | --- | --- | --- | --- |
| FND: Tooling and CI gates | cross-cutting: CI, lints, documentation gates, coverage | C | 14 | 0 | 2 |
| VER: Versioning and compatibility | cross-cutting: file formats, protocols, public APIs | C | 12 | 1 | 4 |
| ERR: Error architecture | cross-cutting: error types, protocol errors, editor error channel, command line errors | C | 6 | 0 | 1 |
| DET: Determinism enforcement and proof | state hash, type-level guards, golden traces and replays | B | 6 | 0 | 1 |
| QA: Test infrastructure | cross-cutting: property and fuzz tests, end-to-end suite, test doubles | C | 9 | 0 | 2 |
| PHY: Math and physics | ludenea-math, ludenea-physics, physics editor plugin | C | 17 | 2 | 4 |
| ECS: ECS, schemas, snapshots and hashing | ludenea-ecs, ludenea-schema, ludenea-macros, App registration | B | 20 | 2 | 3 |
| SCN: Scenes, prefabs and data assets | ludenea-app scene, prefab, assets and meta modules; prefab and data-asset session operations | C | 15 | 3 | 4 |
| AST: Project assets and changes made outside the editor | metas, asset listing, file watching, open documents | C | 10 | 1 | 1 |
| RUN: Simulation loop, inputs and game-code isolation | ludenea-app simulation, inputs, parallel runtime; game panics across host, command line tool and worker | C | 13 | 2 | 2 |
| REP: Replays, traces and the command line tool | ludenea-app replay and trace formats, ludenea-cli run, verify and diff | C | 8 | 0 | 3 |
| CTL: Control protocol, host and transports | ludenea-control host and transports, control-client, game process supervision | C | 11 | 1 | 4 |
| SES: Editing sessions, undo and redo, timeline | ludenea-control session, undo history, checkpoints, timeline | C | 14 | 2 | 6 |
| BRN: Branches, ghost and divergence tools | branch tree, compare simulation (ghost), causal divergence report, trace and snapshot viewer | C | 11 | 0 | 0 |
| PRJ: Projects, builds, serving and the template | ludenea-project, ludenea-serve, the project template | C | 17 | 1 | 2 |
| PLG: Editor plugin system | editor-shell plugin API, loader and registry; plugin bundling | D | 14 | 1 | 2 |
| SHL: Editor shell and panels | editor-shell layout, palette, shortcuts and stores; built-in panels (console, inspector, scene tree, viewport, timeline) | D | 19 | 1 | 2 |
| AUT: Scene authoring in the editor | the level-building workflow across scene tree, inspector, viewport and documents | D | 9 | 1 | 3 |
| INT: Explorer and introspection | Explorer panel, introspection protocol and subscriptions | D | 8 | 1 | 2 |
| VPR: Viewport renderer and debug-draw API | ludenea-debug-draw (mesh and wgpu renderer), viewport panel, game-facing debug-draw API | C | 8 | 0 | 2 |
| DSK: Desktop application | editor/apps/desktop (Tauri backend and entry) | D | 8 | 1 | 3 |

## How to execute this backlog

- One work item at a time, test first: write the item's first failing test, make it pass, refactor, then check the acceptance criteria. Run the crate's own suite and, for any engine change, the golden tests (`cargo test --locked -p boxes --test golden`).
- Every input finding was confirmed or partially confirmed by a second verifier; none was disputed or unverified. Partially confirmed findings are kept with the scope their verifier established (for example sessions-1 is not reachable from the editor's own flows but is reachable through protocol messages, so it stays P0 as part of SES-1; physics-3 continuous collision is a documented deferral, kept at P3).
- Golden policy: an item whose golden_impact is certain re-records the goldens with the README commands (FND-9) and explains why in its commit; once VER-8 lands, it also bumps SIMULATION_REVISION. An item marked possible must run the golden verification and treat a moved hash as a deliberate decision to justify, never as noise.
- Protocol policy (VER-1, VER-3): the first item that changes the control or project wire shape bumps the version (control to 4, project to 3) and records it in the CHANGELOG; later items in the same release update the pinned fingerprint and the same changelog entry instead of bumping again.
- Several findings appear in one item because they share one root cause; the finding index at the end of the report maps every finding id to its item.
- Priorities: P0 crashes or panics on bad input, data loss, determinism, broken headline features; P1 quality-bar violations in main workflows and UX or DX blockers; P2 the rest of the quality bar; P3 polish. Effort: S under an hour, M a few hours, L a day or two, XL more.

## Execution order

### Phase 0: guard rails

Cheap gates and policies every later item leans on: formatting, docs and wasm lints; dumps that reach CI artifacts; the versioning policy and the protocol fingerprint that every protocol change must update; the public editor log that error and isolation fixes report to; the pinned hash shapes that guard every hashing change.

1. **FND-1** [P1, M] Enforce rustfmt, rustdoc and WebAssembly clippy in CI and in a local cargo xtask check
2. **FND-4** [P1, S] Divergence dumps reach a CI artifact on every platform and target
3. **VER-1** [P1, M] Versioning policy and a CHANGELOG with a formats table, checked by a test
4. **VER-3** [P1, M] Pin the protocol wire shape and generate the versions and the reply map from Rust (after VER-1)
5. **ERR-1** [P1, M] Public editor log; every command dispatcher reports failures there
6. **DET-1** [P1, S] Pin the canonical hash of every serde shape

### Phase 1: P0, crashes, data loss and broken headline features

Panics reachable from ordinary use or malformed input, silent data loss, editor-wide failures and the Windows build. Each starts with its reproduction as a failing test.

7. **RUN-1** [P0, S] Thread counts validated and pool creation fallible end to end
8. **CTL-3** [P0, S] Game and cargo output read as bytes
9. **PHY-1** [P0, M] A Pose rotation is always unit length, and checkpoint restore never panics
10. **PHY-2** [P0, M] Every physics input validated so the post-step assert is a true invariant
11. **SES-1** [P0, M] Undo history and branch restore keep the tree's invariants
12. **SES-2** [P0, S] Redo rebuilds only the branch's own segment
13. **ECS-1** [P0, S] Recursive types refused cleanly instead of aborting the process
14. **ECS-3** [P0, M] Restore and asset insertion validate prefab assets
15. **SCN-3** [P0, M] Local identifier exhaustion and edge inputs return errors, never panic
16. **SCN-1** [P0, M] Create prefab and Apply to prefab work from the tick-0 state; Apply is one undoable operation
17. **SCN-2** [P0, M] Prefab documents are cycle-checked against themselves (after VER-3)
18. **SHL-1** [P0, M] Save scene is two-phase: the scene stays unsaved until the write succeeds (after VER-3, ERR-1)
19. **VER-5** [P0, M] Stored sessions: versioned, decoded defensively, never overwritten when refused (after VER-3)
20. **PRJ-3** [P0, M] Project WebSocket limits deliberate; oversized messages refused precisely
21. **AST-2** [P0, M] The open scene follows renames and removals on disk
22. **PLG-1** [P0, M] Error boundaries around every plugin-rendered slot (after ERR-1)
23. **INT-1** [P0, M] Explorer lists virtualized in the real dock
24. **AUT-1** [P0, L] Multi-entity edits are one batch (after VER-3)
25. **DSK-1** [P0, S] The desktop app builds on Windows: full icon set
26. **RUN-2** [P0, L] One panic boundary around game code (after RUN-1)

### Phase 2: harness on top of the P0 fixes

Seeded fuzzing and random session sequences land green once the P0 crashes are fixed, then guard every later refactor; the versioned-decode foundation and listener isolation come here because many P1 items build on them.

27. **QA-1** [P1, M] Seeded fuzz tests for every decoder and both protocols (after RUN-1, PHY-1, SES-1, ECS-3, SCN-3)
28. **VER-2** [P1, M] One versioned-decode helper and one VersionError for every format (after VER-1)
29. **VER-4** [P1, M] Strict text formats: meta version checked, unknown fields refused (after VER-2)
30. **CTL-4** [P1, M] Listener isolation in the clients and the shell (after ERR-1)
31. **SES-4** [P1, M] Redo keeps play recorded since the undo (after SES-1)
32. **QA-2** [P1, M] Seeded random command sequences check the session invariants (after SES-1, SES-2, SES-4)

### Phase 3: P1, main-workflow quality and UX or DX blockers

Grouped so each item's foundations come first: ECS rewriter before scene work, PLG-2 and PLG-3 before the authoring and shell items that use the public API, VER-6 and VER-7 before undo and edit-log format changes, DSK-2 before the native game supervision.

33. **SES-3** [P1, M] Restores, edit logs and steps bounded
34. **SES-14** [P1, S] Divergence messages name their cause
35. **SES-5** [P1, S] One reachable end on every timeline view
36. **SES-8** [P1, M] Seeking reuses the current state and caches near recent seeks
37. **RUN-3** [P1, S] Live input idleness judged on the game's mapping
38. **PHY-3** [P1, M] Shapes built only through validating constructors; degenerate capsules handled
39. **PHY-4** [P1, S] A collider without a Body takes part as static ground
40. **ECS-5** [P1, S] The Schema derive follows serde from and into mirrors by construction
41. **ECS-6** [P1, L] One fallible schema-guided rewriter for entity and handle references (after ECS-5)
42. **ECS-9** [P2, M] A trybuild UI suite for the derive macros; name overrides validated
43. **ECS-4** [P1, M] Component types can evolve: #[serde(default)] accepted, moved types keep their name (after ECS-9)
44. **SCN-4** [P1, M] Create prefab from selection refuses references into the selection (after ECS-6)
45. **SCN-5** [P1, M] Remove and unpack prefab instances (after VER-3)
46. **SCN-8** [P1, M] Engine-owned resources stay out of scene files (after VER-4)
47. **AST-1** [P1, M] One asset-set policy for the command line tool and the editor; duplicated assets get a fresh identifier
48. **REP-1** [P1, S] Verify a scene trace recorded without an input log
49. **REP-2** [P1, M] Trace format 2: canonical scene identity and per-identity mismatch messages (after VER-2)
50. **ERR-5** [P2, S] Game command line tool: stderr, printed warnings, documented arguments and exit codes
51. **REP-3** [P1, L] Desync reports localize the divergence (after REP-2, FND-4, ERR-5)
52. **PRJ-1** [P1, S] The Vite dev proxy stops forging Origin
53. **PRJ-2** [P1, M] Opening another project ends the previous game
54. **PLG-2** [P1, M] Every registration kind is observable; hot reload replaces inspectors and overlays
55. **PLG-3** [P1, L] No privileged channels: built-ins use only the public editor API (after ERR-1, PLG-2, VER-3)
56. **AUT-2** [P1, M] The selection is keyed by load address (after PLG-3)
57. **AUT-3** [P1, M] A Create entity command (after ERR-1)
58. **AUT-4** [P1, M] Add component inserts the type's default value (after VER-3)
59. **PHY-5** [P1, L] Polygons authored from vertices; physics inspectors for shapes and rotations; enum switches seeded (after VER-1, AUT-4)
60. **PHY-6** [P1, M] The physics panel reads in bulk once per shown frame; contacts are clickable (after VER-3)
61. **SHL-2** [P1, S] The command palette ranks matches by quality
62. **SHL-14** [P1, M] Inspector inputs never collapse; override marks sit in the component header
63. **SCN-14** [P2, L] Split scene.rs and prefab.rs by concern (after ECS-6)
64. **SCN-9** [P1, M] Data asset edits keep the file's comments and layout (after SCN-14)
65. **VER-6** [P2, M] Branch tree format versioned and decoded exactly (after VER-2)
66. **VER-7** [P2, M] Frozen fixtures for every encoded format (after VER-6)
67. **SES-6** [P1, L] The undo history stores each scene version once and is bounded by bytes (after VER-7)
68. **RUN-4** [P1, L] Edits on runtime-spawned entities keep their target (after VER-2, VER-7)
69. **CTL-5** [P1, M] A lost connection to ludenea serve is shown and recovered (after ERR-1)
70. **CTL-2** [P1, M] Request deadlines and a not-responding watchdog (after ERR-1)
71. **DSK-2** [P1, M] The desktop backend is a tested library (after DSK-1)
72. **CTL-1** [P1, L] A native game that exits is reported, its channel closed, and it restarts from its last export (after DSK-2)
73. **DSK-6** [P1, M] Closing the window stores the session first (after DSK-2)
74. **FND-6** [P2, M] Project builds and the game-process lifecycle tested on macOS, Linux and Windows (after FND-1)
75. **DSK-3** [P1, M] Windows: games run from a private copy, without console windows, with plain paths (after FND-6)
76. **VPR-1** [P1, M] The renderer tested on WebGL2 and WebGPU with pixel checks (after FND-1)
77. **VPR-2** [P1, M] GPU errors captured, device limits respected, failures shown with a retry (after VPR-1)
78. **INT-2** [P1, M] Events and Commands answered for any simulated tick (after RUN-2)
79. **INT-4** [P1, M] Introspection states tested at every level (after INT-2)

### Phase 4: P2, the rest of the quality bar

Remaining quality-bar gaps, deep refactors guarded by the Phase 2 harness, and the golden corpus.

80. **FND-2** [P2, M] Lint and format the editor: type-checked ESLint, React hooks rules, import boundaries, Prettier (after FND-1)
81. **FND-3** [P2, M] Complete the determinism lints and make every opt-out justified (after FND-1)
82. **FND-5** [P2, S] Run the engine test suites under wasm32-wasip1 in CI (after FND-1)
83. **FND-9** [P2, S] Document and check in CI every golden and fixture regeneration
84. **FND-13** [P2, L] Public API documentation: a compiled crate guide, determinism rules for game code, missing_docs gates (after FND-1)
85. **ERR-2** [P2, L] Structured protocol errors: specific codes and a typed detail (after VER-3)
86. **ERR-3** [P2, M] Scene, prefab and override value errors located in their file (after ERR-2, SCN-14)
87. **ERR-4** [P2, M] Binary decode errors separate a damaged file from a value mismatch (after VER-2)
88. **ERR-6** [P2, S] Malformed handshakes answered with the decode reason
89. **DET-2** [P2, S] The assets hash leaf includes each asset's type name
90. **ECS-8** [P2, S] Compiler diagnostics for misused systems, parameters, components and schemas (after ECS-9)
91. **DET-3** [P2, S] usize leaves the Schema; Polygon::count becomes u32 (after ECS-8)
92. **DET-4** [P2, M] &World is read-only outside the ECS
93. **DET-5** [P2, M] A stress golden exercises the parallel colored solve (after FND-4)
94. **DET-6** [P2, L] A golden corpus covering every feature, verified in Chromium, Firefox and WebKit (after DET-5)
95. **PHY-7** [P2, M] Math edge cases defined; the speed clamp is overflow-safe; units and limits documented
96. **PHY-12** [P2, L] Deep stacks and large mass ratios hold (after DET-5)
97. **ECS-2** [P2, L] Recursive schemas supported through TypeSchema::Ref (after ECS-1, ECS-6)
98. **ECS-7** [P2, S] Reading events before every writer is a build error
99. **ECS-10** [P2, M] Prefab overrides: precise messages, a fallible form, validation before spawning (after RUN-2)
100. **ECS-11** [P2, M] Type-erased World APIs return an error on a wrong type
101. **ECS-13** [P2, S] Adding a plugin twice is refused
102. **ECS-14** [P2, S] Generated TypeScript never shadows global names
103. **ECS-15** [P2, M] One field path convention on the wire (after VER-3, ERR-2)
104. **ECS-19** [P2, M] World::hash streams; the host hashes each tick once (after DET-1)
105. **SCN-6** [P2, M] The foreign-reference check uses schemas, not field names (after ECS-6)
106. **SCN-7** [P2, S] Version 1 scenes upgrade before prefab operations
107. **SCN-10** [P2, M] Data assets the open scene does not load are viewable and editable (after ERR-2)
108. **SCN-11** [P2, S] The host encodes new data asset values; the TypeScript RON writer is deleted (after VER-3)
109. **SCN-13** [P2, M] Added and removed instance components are visible and revertible (after VER-3)
110. **AST-4** [P2, M] Metas deleted by hand restored with their identifier and kind
111. **AST-3** [P2, L] Identity kept across moves made without the meta (after AST-4)
112. **AST-5** [P2, S] Meta-only changes emit watcher events
113. **AST-6** [P2, S] Hidden and temporary files ignored by every listing (after AST-1)
114. **AST-7** [P2, M] Saving over a change made on disk asks first (after AST-2)
115. **AST-9** [P2, M] Deleting an asset in use names the documents that reference it (after SCN-5)
116. **RUN-5** [P2, M] Player count declared by the game and carried by the protocol (after VER-3)
117. **RUN-6** [P2, S] Runtime APIs have fallible forms
118. **RUN-7** [P2, L] A Run type owns the logs and the tick contract (after RUN-2, QA-2)
119. **RUN-8** [P2, M] Playing exports incrementally; storing a long session stays fast (after VER-5)
120. **RUN-9** [P2, M] The template game uses its input and verifies its replays
121. **RUN-13** [P2, M] A logging API for game code that reaches the Console on every target (after ERR-1)
122. **REP-4** [P2, M] Record a replay again with this build (after ERR-5)
123. **REP-6** [P2, S] The editor says whether an opened replay matches
124. **REP-7** [P2, S] Trace comparison checks both traces describe the same run (after REP-2)
125. **VER-8** [P2, M] Engine identity recorded and reported: workspace version, simulation revision, manifest engine (after VER-2, REP-2)
126. **VER-9** [P2, S] Editor plugin API version checked at load (after VER-1)
127. **CTL-8** [P2, L] Host::execute split by area; topic state owned by Subscriptions (after RUN-2, SCN-1)
128. **SES-9** [P2, L] Session split by concern (after QA-2, RUN-7)
129. **SES-7** [P2, S] Numbers beyond the f32 range refused precisely
130. **SES-10** [P2, M] The host stops rebuilding the Timeline per frame; per-tick progress travels in Ticked (after VER-3)
131. **SES-11** [P2, L] Resume on the same build skips resimulation; resimulation bounded by time (after SES-9, VER-7)
132. **SES-12** [P2, M] Checkpoint budget in bytes
133. **SES-13** [P2, S] Undo labels say what they undo (after VER-7)
134. **BRN-3** [P2, M] The ghost and the report load the scene once per scene version
135. **BRN-4** [P2, M] Ghost and report work bounded by time; the report start resumable (after BRN-3)
136. **BRN-1** [P2, L] Causes found through entity references in values (after ECS-6, ECS-15)
137. **BRN-2** [P2, M] Resource and asset divergence reported (after VER-3)
138. **BRN-5** [P2, M] A restore before a fork forks a child instead of being lost (after SES-1)
139. **BRN-8** [P2, L] The causal report is incremental and nested (after BRN-1)
140. **BRN-9** [P2, M] Root causes name the field; values labelled by branch
141. **PRJ-4** [P2, M] Builds cancellable and scoped to their project
142. **PRJ-5** [P2, S] ludenea.toml versioned, strict, located and reloaded (after VER-4)
143. **PRJ-6** [P2, S] ludenea serve refusals and misconfigurations explained (after PRJ-5)
144. **PRJ-7** [P2, S] The embedded editor tracked by cargo; static serving tested
145. **PRJ-8** [P2, M] New projects inherit the engine's enforcement and are portable (after FND-3)
146. **PRJ-9** [P2, M] Game types and plugin declarations written without a running game (after ECS-14)
147. **PRJ-10** [P2, S] The project protocol handshake is tested (after ERR-6)
148. **PRJ-11** [P2, S] A directory that cannot be watched does not block opening
149. **PLG-4** [P2, M] Custom inspectors keep decorations and receive readOnly and the schema (after PLG-3)
150. **PLG-6** [P2, M] Plugin errors point at the source file and line (after ERR-1)
151. **PLG-7** [P2, M] Registrations owned by their plugin whatever the time of the call (after PLG-2)
152. **PLG-9** [P2, M] A cargo metadata failure does not take the project plugin down
153. **PLG-10** [P2, M] Settings: a panel, observability and validated values (after PLG-2)
154. **SHL-3** [P2, M] Dock layout persisted per project; primary panels active; Reset layout (after PLG-2)
155. **SHL-4** [P2, M] Build-aware empty states (after PLG-3)
156. **SHL-5** [P2, M] The Console is virtualized, follows its tail, and can be cleared and filtered (after ERR-1)
157. **SHL-6** [P2, M] Shortcuts work from text fields and on every keyboard (after PLG-2)
158. **SHL-7** [P2, L] Entity lists are keyboard listboxes that reveal the selection (after AUT-2)
159. **SHL-9** [P2, S] EntityList keeps only the pages in view (after INT-1)
160. **SHL-11** [P2, S] Scene tree list errors and action errors kept apart (after ERR-1)
161. **SHL-15** [P2, S] Inspector number and text fields behave predictably
162. **SHL-16** [P2, M] Maps and entity references editable in the inspector (after SHL-15)
163. **SHL-17** [P2, S] The selection survives scrubbing before an entity exists and notices a despawn (after AUT-2)
164. **AUT-5** [P2, M] New scene and Save scene as (after SHL-1)
165. **QA-6** [P2, M] Reliable end-to-end runs
166. **AUT-9** [P2, M] End-to-end: a level built from nothing survives save and reopen (after AUT-3, AUT-4, AUT-5, QA-6)
167. **QA-3** [P2, M] Shared all-shapes fixture for diff, TypeScript declarations, JSON wire and inspector fields (after DET-1)
168. **QA-5** [P2, L] End-to-end coverage of every promised workflow (after SCN-1, SCN-9, QA-6)
169. **INT-3** [P2, M] Events rendered with their fields and clickable entities (after INT-2, PLG-3)
170. **INT-5** [P2, M] Entity watches skip unchanged frames (after ECS-19)
171. **VPR-3** [P2, M] Non-finite primitives dropped with a located warning (after ERR-1)
172. **VPR-4** [P2, L] Retained geometry; the camera is a uniform (after VPR-2)
173. **VPR-5** [P2, S] Concave fills triangulated correctly
174. **VPR-6** [P2, M] Typed debug layers; frames filtered to the requested layers
175. **DSK-4** [P2, L] The production desktop build packaged in CI; cargo found outside PATH (after DSK-1)
176. **DSK-5** [P2, S] Startup failures visible (after DSK-2)

### Phase 5: P3, polish

Hygiene, documentation, consistency and deferred physics features.

177. **FND-7** [P3, M] Measure coverage in CI and stop the test server gracefully (after FND-1)
178. **FND-8** [P3, S] Generate bindings through cargo xtask bindings with a check mode (after VER-3)
179. **FND-10** [P3, S] Deterministic slow tests block CI; timing budgets keep generous CI ceilings
180. **FND-11** [P3, M] CI hygiene and supply-chain checks
181. **FND-12** [P3, S] Vendored patches: upstream links, an in-effect guard and a behavioural test each
182. **FND-14** [P3, M] Narrow the public Rust API and snapshot it (after FND-13)
183. **QA-4** [P3, M] Typed fake peers for editor tests (after VER-3)
184. **QA-7** [P3, M] Watcher tests synchronise on sentinel files
185. **QA-8** [P3, S] Error tests assert the variant and the message (after ERR-4)
186. **QA-9** [P3, M] The worker host and the wasm panic hook are tested (after RUN-2)
187. **VER-10** [P3, M] Binary formats carry a kind tag (after VER-2)
188. **VER-11** [P3, S] Format types keep their version field private (after VER-2)
189. **VER-12** [P3, L] A host-owned session file the command line tool can read (after VER-5, VER-6, VER-7)
190. **PHY-8** [P3, S] Untested public math functions get recorded-bits and property tests
191. **PHY-9** [P3, S] Shape and component validation errors name the field, the value and the edge (after ERR-3)
192. **PHY-10** [P3, S] The debug draw shows the rounding radius of polygons
193. **PHY-11** [P3, S] Coincident circles separate
194. **PHY-13** [P3, M] The broad phase caches static proxies; raycast and overlap are culled and allocation-free
195. **RUN-12** [P3, S] Compare and divergence simulations use the session's thread count (after RUN-1)
196. **PHY-14** [P3, M] Parallel stages gated by work; the host's default thread count is bounded (after RUN-1, RUN-12)
197. **PHY-15** [P3, M] One transform component owned by the core; the viewport learns it from the game description (after PLG-3)
198. **PHY-16** [P3, L] Fast bodies do not tunnel through static walls (after PHY-12)
199. **PHY-17** [P3, XL] Physics essentials for real games: collision filters, sensors, rotation lock, richer contact events (after PHY-16, VER-4)
200. **ECS-12** [P3, S] Registration errors phrased in App terms
201. **ECS-16** [P3, M] Commands name the entity they spawn
202. **ECS-17** [P3, S] System labels derived from types; duplicate names refused
203. **ECS-18** [P3, S] Names state intent, not history; diff_snapshots split
204. **ECS-20** [P3, M] ECS costs follow each storage's own size (after ECS-19)
205. **SCN-12** [P3, M] Data asset files read without a synthetic wrapper (after SCN-14)
206. **SCN-15** [P3, M] Prefab authoring friction removed (after AUT-2)
207. **AST-8** [P3, S] Asset browser errors cleared after a successful reload (after ERR-1)
208. **AST-10** [P3, S] Case-only renames work; write errors name the destination
209. **RUN-10** [P3, S] Rng::range_f32 stays below its upper bound
210. **RUN-11** [P3, S] Input capture releases keys after Cmd shortcuts and shows when it is active
211. **REP-5** [P3, M] Verify a folder of replays; document a project's CI (after REP-4)
212. **REP-8** [P3, S] Command line tool and replay hygiene (after REP-3)
213. **CTL-6** [P3, S] A second Hello starts a fresh connection
214. **CTL-7** [P3, S] Only real protocol frames leave a game's output; the client warns on unknown frames (after CTL-3)
215. **CTL-9** [P3, S] The session export is one wire struct (after VER-3)
216. **CTL-10** [P3, M] A documented control surface for third-party tools (after VER-3)
217. **CTL-11** [P3, S] Transport hygiene (after CTL-4)
218. **BRN-6** [P3, S] Branch names validated
219. **DSK-7** [P3, M] Desktop conveniences: folder picker, remembered window, project title (after DSK-2)
220. **BRN-7** [P3, M] Divergence files opened from anywhere; snapshot ticks checked (after DSK-7)
221. **BRN-10** [P3, S] The host names the root branch after the document (after VER-3)
222. **BRN-11** [P3, M] Undo on a stale branch completes after resimulation; deleting a branch keeps unrelated redo (after SES-1)
223. **PRJ-12** [P3, S] ludenea new friction removed
224. **PRJ-13** [P3, M] One editor-connection handle shared by serve and desktop (after DSK-2)
225. **PRJ-14** [P3, M] Editor composition in a library package (after CTL-1)
226. **PRJ-15** [P3, S] One bindgen configuration
227. **PRJ-16** [P3, M] Build output parsing unit-tested
228. **PRJ-17** [P3, M] A second editor on the same project is detected
229. **PLG-5** [P3, S] Plugin loading never waits on a hung bundle (after ERR-1)
230. **PLG-8** [P3, L] Typed writes in the plugin API (after PLG-3, PRJ-9)
231. **PLG-11** [P3, M] A plugin author guide and a typed template plugin (after PLG-3, PRJ-9, VER-9)
232. **PLG-12** [P3, S] The shared modules list has one source
233. **PLG-13** [P3, S] JSON imports and the vendored rolldown patch tested
234. **PLG-14** [P3, M] ListPlugins answered from the watch, outside the service lock (after PLG-9)
235. **SHL-8** [P3, M] Accessibility basics (after FND-2)
236. **SHL-10** [P3, L] The document store as an explicit state machine; large components split (after VER-5, AST-2, AST-7)
237. **SHL-12** [P3, S] Viewport and list multi-selection behave consistently (after AUT-1)
238. **SHL-13** [P3, S] Shared helpers replace duplicated ones (after PLG-3)
239. **SHL-18** [P3, S] Read-only resources reported by the game (after VER-3)
240. **SHL-19** [P3, M] Small editor polish: reveal, rebuild hint, consistent titles, palette parity (after PLG-3)
241. **AUT-6** [P3, M] The default scene can be set and follows renames (after PRJ-5)
242. **AUT-7** [P3, L] Entities can be named (after AUT-2, SHL-7)
243. **AUT-8** [P3, L] Copy, paste and duplicate entities (after AUT-1, SCN-3)
244. **INT-6** [P3, M] Entity query errors positioned
245. **INT-7** [P3, M] Schedule timings measured on every target and labelled honestly
246. **INT-8** [P3, M] One tick recorder shared by introspection and the divergence report (after INT-2, BRN-1)
247. **VPR-7** [P3, M] The ghost and the drag preview drawn by the GPU renderer (after VPR-4)
248. **VPR-8** [P3, S] Labels follow the highlight and avoid clumping; Frame all
249. **DSK-8** [P3, S] The plugin origin comes from convertFileSrc (after DSK-2)

## Workstreams

### FND: Tooling and CI gates (grade C)

Subsystem: cross-cutting: CI, lints, documentation gates, coverage.

**Problem.** CI proves determinism well but enforces only native clippy and tests. There is no rustfmt or rustdoc gate (the docs build fails today), WebAssembly targets are never linted and are not lint-clean, the editor has no linter, formatter or coverage, divergence dumps never reach the CI artifact in the realistic failure path, project and game-process tests run on Linux or unix only, the determinism lints miss several sources and some opt-outs carry no reason, and the public Rust API is barely documented.

**Target state.** One local entry point (cargo xtask check) and CI run rustfmt, rustdoc with -D warnings, clippy on native and WebAssembly targets, ESLint and Prettier, coverage, and the cross-platform project tests. Every lint opt-out is an #[expect] with a reason. Dumps from any failing platform reach an artifact. The facade has a compiled getting-started guide and missing_docs is enforced on game-facing crates.

#### FND-1: Enforce rustfmt, rustdoc and WebAssembly clippy in CI and in a local cargo xtask check

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: delivery-12, delivery-7.
- Depends on: nothing.
- Locations and files: `.github/workflows/determinism.yml`, `xtask/src/main.rs`, `crates/ludenea-control/src/protocol.rs:108`, `crates/ludenea-control/src/in_process.rs:76-91`, `crates/ludenea-serve/src/main.rs:41`, `crates/ludenea-serve/Cargo.toml:6-8`.
- Acceptance: CI runs cargo fmt --all --check (engine and src-tauri), RUSTDOCFLAGS=-D warnings cargo doc --locked --no-deps --workspace, and clippy -D warnings for wasm32-unknown-unknown (ludenea, ludenea-control, boxes lib; ludenea-debug-draw with render) and wasm32-wasip1 (boxes bins); all green. cargo xtask check runs the same locally. protocol.rs:108 uses a pointer-width-aware expect, answer and malformed in in_process.rs compile only where used, serve main.rs:41 writes [::1] as code, and the ludenea-serve binary sets doc = false.
- First failing test: The new CI steps: today cargo clippy --locked --target wasm32-wasip1 -p boxes --bins -- -D warnings and the rustdoc step fail; both must pass.

#### FND-2: Lint and format the editor: type-checked ESLint, React hooks rules, import boundaries, Prettier

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: ts-quality-12.
- Depends on: FND-1.
- Locations and files: `editor/eslint.config.js (new)`, `editor/.prettierrc (new)`, `editor/package.json:9-15`, `.github/workflows/determinism.yml:77-81`, `editor/packages/editor-builtins/src/explorer/Introspection.tsx:42-55`.
- Acceptance: pnpm -C editor lint (typescript-eslint strictTypeChecked with no-floating-promises, no-misused-promises and switch-exhaustiveness-check; eslint-plugin-react-hooks; a boundary rule forbidding imports across editor-builtins plugin folders and between applications, with the known cross-plugin imports listed as exceptions that PLG-3 removes) and pnpm -C editor format:check pass and run in the editor CI job. Existing violations are fixed or carry a reasoned disable.
- First failing test: pnpm -C editor lint on a fixture with an unawaited promise and a missing hook dependency must fail.

#### FND-3: Complete the determinism lints and make every opt-out justified

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: determinism-1, delivery-19, determinism-11, runtime-15, rust-quality-15, determinism-9.
- Depends on: FND-1.
- Locations and files: `clippy.toml`, `crates/ludenea-control/src/pacer.rs:35-39`, `crates/ludenea-control/src/stdio.rs:82-93`, `crates/ludenea-app/src/meta.rs:63-69`, `crates/ludenea-app/src/edit.rs:173`, `crates/ludenea-schema/src/lib.rs:292`, `crates/ludenea-physics/src/manifold.rs:354`, `Cargo.toml`, `xtask/src/main.rs`.
- Acceptance: clippy.toml also bans sort_unstable*, select_nth_unstable*, std::env::var/var_os/vars/args, std::thread::spawn, the rayon entry points, Instant::elapsed, SystemTime::elapsed, rayon::current_thread_index, and the hashbrown and ahash maps, each with an actionable reason naming a replacement that exists (no reason points to a ludenea_math function that is missing). Pacer::set_speed uses ludenea_math::clamp. Every remaining opt-out is #[expect(..., reason = ...)] and the workspace enables clippy::allow_attributes_without_reason. cargo xtask lint-fixtures proves each ban fires, and a CI step checks with nm that the game binary imports no platform libm transcendental.
- First failing test: xtask/fixtures/nondeterminism/src/lib.rs with one use per banned item; cargo xtask lint-fixtures fails unless clippy reports every one.

#### FND-4: Divergence dumps reach a CI artifact on every platform and target

- Priority P1, kind fix, effort S, golden impact none.
- Findings: delivery-1.
- Depends on: nothing.
- Locations and files: `.github/workflows/determinism.yml:23-54`, `examples/boxes/tests/golden.rs:20,127`.
- Acceptance: examples/boxes/tests/golden.rs reads LUDENEA_DUMP_DIR (falling back to the temporary directory); the workflow sets it to the workspace dump folder; the verify steps run with if: success() || failure(); the wasm job uploads dump/ on failure; artifacts are named per operating system, target and thread count and include the failing run's trace.
- First failing test: examples/boxes/tests/golden.rs dump_dir_follows_ludenea_dump_dir; then a throwaway branch perturbing one golden hash shows tick-N.snap and tick-(N-1).snap in every artifact.

#### FND-5: Run the engine test suites under wasm32-wasip1 in CI

- Priority P2, kind put-in-place, effort S, golden impact none.
- Findings: determinism-10.
- Depends on: FND-1.
- Locations and files: `.github/workflows/determinism.yml:44-53`, `crates/ludenea-ecs/src/component.rs:176`.
- Acceptance: The wasm job runs cargo test --locked --target wasm32-wasip1 with wasmtime as runner for ludenea-math, ludenea-ecs, ludenea-app, ludenea-physics and boxes. Tests needing unwinding, threads or the filesystem are ignored on wasm with a stated reason.
- First failing test: The new CI step (ludenea-math and ludenea-physics already pass under wasmtime).

#### FND-6: Project builds and the game-process lifecycle tested on macOS, Linux and Windows

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: delivery-9, test-strategy-7.
- Depends on: FND-1.
- Locations and files: `crates/ludenea-project/src/service.rs:616-640`, `crates/ludenea-project/tests/build.rs`, `crates/ludenea-project/tests/service.rs`, `.github/workflows/determinism.yml:94`.
- Acceptance: The service lifecycle tests drop cfg(unix) and drive a self-re-executing Rust fake game (as crates/ludenea-cli/tests/host.rs does). A projects CI job runs cargo test --locked -p ludenea-project -- --ignored on the three operating systems. Each #[ignore] carries a reason.
- First failing test: crates/ludenea-project/src/service.rs an_exit_code_is_announced with the Rust fake game, run on windows-2025.

#### FND-7: Measure coverage in CI and stop the test server gracefully

- Priority P3, kind put-in-place, effort M, golden impact none.
- Findings: delivery-14, test-strategy-16.
- Depends on: FND-1.
- Locations and files: `.github/workflows/determinism.yml`, `editor/vitest.config.ts`, `editor/package.json`, `crates/ludenea-serve/src/serve.rs:89-91`, `crates/ludenea-serve/tests/serve.rs:31-35`.
- Acceptance: A Linux job runs cargo llvm-cov --workspace (profile files propagated to spawned binaries) and vitest with @vitest/coverage-v8, publishes both in the job summary, and fails below a floor set from today's numbers (74 percent of Rust lines, about 90 percent of editor lines). ludenea serve shuts down gracefully on SIGTERM or stdin close, and the serve tests stop it that way.
- First failing test: crates/ludenea-serve/tests/serve.rs serve_exits_cleanly_on_sigterm (exit code 0, port released).

#### FND-8: Generate bindings through cargo xtask bindings with a check mode

- Priority P3, kind put-in-place, effort S, golden impact none.
- Findings: test-strategy-17.
- Depends on: VER-3.
- Locations and files: `crates/ludenea-control/tests/typescript.rs`, `crates/ludenea-project/tests/typescript.rs:10-32`, `.cargo/config.toml`, `xtask/src/main.rs`, `.github/workflows/determinism.yml:83-93`.
- Acceptance: Tests no longer write into editor/packages/control-client/src/generated. cargo xtask bindings writes; --check compares in memory and lists stale files; CI uses the check and keeps the git diff and porcelain guard.
- First failing test: Running cargo test --workspace leaves git status --porcelain empty.

#### FND-9: Document and check in CI every golden and fixture regeneration

- Priority P2, kind put-in-place, effort S, golden impact none.
- Findings: replays-13, delivery-18.
- Depends on: nothing.
- Locations and files: `README.md:25-76,221-233`, `AGENTS.md:87-89`, `.github/workflows/determinism.yml`, `editor/package.json:6-8`.
- Acceptance: The README gives the exact commands that record golden/boxes.trace, golden/spawner.replay, the generated bench scene and the editor divergence fixtures. The layout table lists every crate, xtask and vendor; the Node requirement matches package.json; the README says timing budgets are reported, not enforced. CI regenerates golden/spawner.replay and editor/e2e/fixtures/divergence and runs git diff --exit-code.
- First failing test: n/a (documentation and CI): the regeneration step must leave a clean tree.

#### FND-10: Deterministic slow tests block CI; timing budgets keep generous CI ceilings

- Priority P3, kind fix, effort S, golden impact none.
- Findings: test-strategy-6, delivery-10.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/tests/branches.rs:598-624`, `crates/ludenea-control/tests/divergence.rs:467`, `.github/workflows/determinism.yml:111-123`, `editor/e2e/performance.spec.ts`.
- Acceptance: A blocking release job runs the deterministic ignored tests (checkpoint_budget_with_20_branches_of_10000_ticks and the correctness halves of the timing tests). The non-blocking timing job keeps only elapsed-time assertions, with order-of-magnitude ceilings on CI and the strict numbers behind PERF_STRICT. Ignore reasons say why a test is ignored.
- First failing test: n/a (CI): the blocking job fails when the 20-branch test fails.

#### FND-11: CI hygiene and supply-chain checks

- Priority P3, kind put-in-place, effort M, golden impact none.
- Findings: delivery-20, delivery-13.
- Depends on: nothing.
- Locations and files: `.github/workflows/determinism.yml`, `deny.toml (new)`, `.github/dependabot.yml (new)`.
- Acceptance: A concurrency group with cancel-in-progress; rust-cache on the native matrix; a nightly schedule; cargo-deny (advisories, licence allowlist, sources admitting the vendored paths) on both manifests; pnpm audit --prod; actions pinned by commit SHA with a version comment; the wasmtime version pinned; Dependabot for actions, cargo and npm.
- First failing test: n/a (CI).

#### FND-12: Vendored patches: upstream links, an in-effect guard and a behavioural test each

- Priority P3, kind put-in-place, effort S, golden impact none.
- Findings: delivery-17, scenes-22, projects-11.
- Depends on: nothing.
- Locations and files: `Cargo.toml:68-82`, `crates/ludenea-project/template/Cargo.toml:18-20`, `editor/apps/desktop/src-tauri/Cargo.toml:18-23`, `vendor/ron/src/parse.rs:1040`.
- Acceptance: vendor/ron/LUDENEA.md and vendor/rolldown_common/LUDENEA.md record the diff, the reason and the upstream issue. A CI check asserts cargo metadata resolves ron and rolldown_common to the vendored path for the engine, the desktop manifest and a generated template. An ignored release test bounds scene load time as entity counts grow (close to linear), and a ludenea-project test decodes an internally tagged enum with a float. The template drops its [patch] once the engine depends on the fixed parser directly.
- First failing test: crates/ludenea-app/tests/scene.rs #[ignore] scene_load_time_grows_linearly_with_entities.

#### FND-13: Public API documentation: a compiled crate guide, determinism rules for game code, missing_docs gates

- Priority P2, kind put-in-place, effort L, golden impact none.
- Findings: ecs-17, dx-live-18, rust-quality-10.
- Depends on: FND-1.
- Locations and files: `crates/ludenea/src/lib.rs:1-44`, `Cargo.toml`, `crates/ludenea-ecs/src/query.rs:493-529`, `crates/ludenea-ecs/src/event.rs:61-72`, `crates/ludenea-app/src/app.rs:41-152`, `crates/ludenea-math/src/lib.rs:1`, `crates/ludenea-physics/src/lib.rs:1`.
- Acceptance: crates/ludenea/src/lib.rs has a doctested guide (a component, an input-driven system, a collision handler ordered after physics::Step, spawn_prefab, a plugin, one tick) and a Determinism rules for game code section. The prelude is documented and exports physics::Step. missing_docs, clippy::missing_errors_doc and clippy::missing_panics_doc warn (and therefore fail CI) for ludenea, ludenea-ecs, ludenea-schema, ludenea-app, ludenea-math and ludenea-physics, with the gaps filled.
- First failing test: n/a: the doctest in crates/ludenea/src/lib.rs compiles and runs.

#### FND-14: Narrow the public Rust API and snapshot it

- Priority P3, kind fix, effort M, golden impact none.
- Findings: rust-quality-11, physics-16.
- Depends on: FND-13.
- Locations and files: `crates/ludenea-control/src/lib.rs:4-21`, `crates/ludenea-physics/src/lib.rs:1-24`, `crates/ludenea-math/src/aabb.rs:12-46`, `crates/ludenea-physics/tests/threads.rs`.
- Acceptance: ludenea-control's checkpoints, pacer, undo, json and in_process modules and ludenea-physics' coloring, contacts and broad-phase internals are pub(crate), their tests moved to unit tests or behind a testing feature. One public path per item. Unused Aabb helpers are tested or removed. A cargo public-api snapshot of the facade is committed and checked in CI.
- First failing test: n/a (API shape): the public-api check fails on an unrecorded change.

### VER: Versioning and compatibility (grade C)

Subsystem: cross-cutting: file formats, protocols, public APIs.

**Problem.** Binary formats mostly read their version first and two real migrations exist, but meta versions are never checked, traces and input logs decode in full before checking, text formats silently drop unknown or misspelled fields, the branch tree and the stored session have no version (and a stored session that does not decode blocks its document while a refused one gets overwritten), the protocol wire shape changed several times under the same number, version errors give wrong advice, the engine identity is a constant nobody reads, the plugin API has no version, and no policy or changelog exists.

**Target state.** A written policy with a CHANGELOG formats table checked against the constants; one versioned-decode helper and one VersionError; every format, editor state included, versioned, strict and pinned by frozen fixtures; the protocol shape pinned by a fingerprint; the engine identity and the plugin API version recorded and reported.

#### VER-1: Versioning policy and a CHANGELOG with a formats table, checked by a test

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: versioning-9.
- Depends on: nothing.
- Locations and files: `CHANGELOG.md (new)`, `AGENTS.md`, `README.md`, `crates/ludenea/tests/formats_documented.rs (new)`.
- Acceptance: CHANGELOG.md (Keep a Changelog) holds a Formats and protocols table (current and oldest readable version of each format and protocol) and sections for the Rust API, control protocol, project protocol and plugin API. AGENTS.md and the README state the rules: the version is read first; any change to an encoded type bumps it; user files stay readable at N-1; editor state may drop to N-1 but a refused file is set aside, never overwritten; protocol changes accumulate under the next version, which is bumped once per release. A test parses the table and compares it with every version constant.
- First failing test: crates/ludenea/tests/formats_documented.rs every_format_version_is_in_the_changelog.

#### VER-2: One versioned-decode helper and one VersionError for every format

- Priority P1, kind fix, effort M, golden impact none.
- Findings: versioning-5, versioning-4, determinism-8, replays-5, untrusted-input-6, scenes-21.
- Depends on: VER-1.
- Locations and files: `crates/ludenea-ecs/src/component.rs:154-161`, `crates/ludenea-ecs/src/snapshot.rs:65-80`, `crates/ludenea-app/src/trace.rs:28-41`, `crates/ludenea-app/src/input_log.rs:37-49`, `crates/ludenea-app/src/replay.rs:87-104`, `crates/ludenea-app/src/edit.rs:97-116`, `crates/ludenea-app/src/scene.rs:37-40`, `crates/ludenea-app/src/prefab.rs:366-373,636-643,716-723`, `crates/ludenea-control/src/host.rs:1438-1444`.
- Acceptance: ludenea-ecs exposes decode_versioned (reads the varint, then decodes exactly) and VersionError { format, found, oldest, newest } whose message says written by a newer Ludenea: update it, or older than this build reads. Snapshot, Replay, EditLog, InputLog and Trace use it, so InputLog and Trace read their version first. Trace::decode returns a TraceError the host maps to a dedicated code. Scene and prefab version errors use VersionError; one check_prefab_version replaces the three copies; SCENE_VERSION replaces literals; ron's recursion error reads values nest deeper than 128 levels.
- First failing test: crates/ludenea-app/tests/input_log.rs a_newer_input_log_with_another_layout_is_refused_by_version (bytes starting 2, 5 give a version error, not a hint about the input type), plus trace.rs a_trace_of_a_newer_version_is_refused_as_such.

#### VER-3: Pin the protocol wire shape and generate the versions and the reply map from Rust

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: protocol-8, versioning-10, protocol-12, ts-quality-8.
- Depends on: VER-1.
- Locations and files: `crates/ludenea-control/src/protocol.rs:21`, `crates/ludenea-project/src/protocol.rs:13`, `crates/ludenea-control/tests/typescript.rs`, `crates/ludenea-project/tests/typescript.rs`, `crates/ludenea-control/src/in_process.rs:51-66`, `editor/packages/control-client/src/client.ts:22-187`, `editor/packages/control-client/src/project.ts:12-33`.
- Acceptance: A fingerprint of the generated ClientMessage and ServerMessage declarations (and the project protocol's) is pinned per protocol version by a test, so a shape change without a deliberate bump and changelog entry fails CI. The fingerprint travels in Hello and Welcome; a mismatch is refused with rebuild the editor (pnpm -C editor build). PROTOCOL_VERSION, PROJECT_PROTOCOL_VERSION and an exhaustive Command::reply_type map are generated into the bindings; client.ts and project.ts import them; handshake() refuses a welcome of another version with version_mismatch.
- First failing test: crates/ludenea-control/tests/typescript.rs the_wire_shape_is_pinned_to_protocol_version_3, plus control-client connect_rejects_a_welcome_of_another_protocol_version.

#### VER-4: Strict text formats: meta version checked, unknown fields refused

- Priority P1, kind fix, effort M, golden impact none.
- Findings: scenes-3, scenes-4, errors-6, versioning-2.
- Depends on: VER-2.
- Locations and files: `crates/ludenea-app/src/meta.rs:16,48-53,84-97`, `crates/ludenea-app/src/scene.rs:119-176`, `crates/ludenea-app/src/prefab.rs:79-127`.
- Acceptance: Meta::parse reads the version first and refuses another one with a VersionError naming the file. #[serde(deny_unknown_fields)] is set on SceneV1, SceneV2, ResolvedV2, SceneEntity, AddressedEntity, Instance, Override, Added, Removed, PrefabFile and Meta; the partial probe structs stay lenient. A misspelled field fails with ron's unknown field message and its position. Every committed asset, fixture and golden still loads.
- First failing test: crates/ludenea-app/tests/prefab.rs misspelled_instance_field_is_refused_with_its_position, and meta.rs a_meta_of_another_version_is_refused_naming_both_versions.

#### VER-5: Stored sessions: versioned, decoded defensively, never overwritten when refused

- Priority P0, kind fix, effort M, golden impact none.
- Findings: versioning-1, sessions-12, shell-12, untrusted-input-5, ts-quality-2, readiness-13.
- Depends on: VER-3.
- Locations and files: `editor/packages/editor-shell/src/stores/document.ts:70-78,154-160,236-249,300-306`, `crates/ludenea-project/src/protocol.rs:118-125`, `crates/ludenea-project/src/files.rs:251-267`, `crates/ludenea-project/src/service.rs:245-251`.
- Acceptance: The stored session is { version: 1, saved } read through a codec returning a result. An unreadable, other-version or game-refused session never blocks opening: the document opens from its file, a warning names .ludenea/sessions/<id>.session and the reason, and a new project request sets the file aside as <id>.<n>.rejected.session before any later store of that document (project protocol bump and changelog entry). Stored bytes are encoded without per-byte string concatenation.
- First failing test: editor/packages/editor-shell/test/document.test.ts a_stored_session_that_does_not_decode_opens_the_document_from_its_file_and_is_set_aside (cases: not base64, not JSON, older shape, newer version, refused by the game).

#### VER-6: Branch tree format versioned and decoded exactly

- Priority P2, kind fix, effort M, golden impact none.
- Findings: branches-6, rust-quality-8.
- Depends on: VER-2.
- Locations and files: `crates/ludenea-control/src/branches.rs:428-451,524-589`, `crates/ludenea-control/src/undo.rs:237-242`.
- Acceptance: BranchTree and StoredBranch encodings start with BRANCHES_VERSION and decode through decode_versioned (trailing bytes refused). Today's unversioned form reads as version 1. Newer versions fail with the branch tree has version N; this editor reads up to M.
- First failing test: branches.rs unit tests tree_bytes_with_trailing_data_are_refused and a_newer_tree_version_names_it.

#### VER-7: Frozen fixtures for every encoded format

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: versioning-6, sessions-20.
- Depends on: VER-6.
- Locations and files: `crates/ludenea-control/tests/formats.rs (new)`, `crates/ludenea-control/tests/fixtures/formats/ (new)`, `crates/ludenea-control/src/undo.rs:10-11,50-56,211-234`.
- Acceptance: crates/ludenea-control/tests/formats.rs with committed fixtures/formats/<format>-v<N>.bin for snapshot, input log, edit log, undo history, branch tree, replay, trace and session export, written once per version and never regenerated. Every committed version decodes or is refused by version, and encoding a fixed canonical value of the current version equals its fixture byte for byte. UNDO_VERSION is bumped for the Asset entry appended after version 3 shipped (version 3 still decodes), and a corrupt version-3 history reports a version-3 error.
- First failing test: crates/ludenea-control/tests/formats.rs current_encodings_match_their_frozen_fixtures.

#### VER-8: Engine identity recorded and reported: workspace version, simulation revision, manifest engine

- Priority P2, kind put-in-place, effort M, golden impact certain.
- Findings: delivery-3, versioning-3, replays-6, readiness-15.
- Depends on: VER-2, REP-2.
- Locations and files: `Cargo.toml`, `crates/ludenea-app/src/replay.rs:18-24,142,180-184`, `crates/ludenea-app/src/trace.rs:12-21`, `crates/ludenea-cli/src/commands.rs:104-116`, `crates/ludenea-control/src/in_process.rs:56`, `crates/ludenea-project/src/manifest.rs:7-8`, `crates/ludenea-project/src/template.rs:71`.
- Acceptance: One workspace version (version.workspace = true) exposed once as ENGINE_VERSION. A SIMULATION_REVISION, bumped whenever goldens are re-recorded on purpose, is recorded in replays and traces (new format versions; older files read as revision unknown). verify and the editor's replay banner print recorded by engine X revision N, this is engine Y revision M before a mismatch. A test fails when the golden files do not carry the current revision. Opening a project whose ludenea.toml engine differs warns with both versions.
- First failing test: crates/ludenea-cli/tests/cli.rs verify_names_the_simulation_revision_of_a_replay_from_another_revision.

#### VER-9: Editor plugin API version checked at load

- Priority P2, kind put-in-place, effort S, golden impact none.
- Findings: plugins-7, versioning-8, ts-quality-19.
- Depends on: VER-1.
- Locations and files: `editor/packages/editor-shell/src/plugin.ts:10-24`, `editor/packages/editor-shell/src/loader.ts:7-13`, `crates/ludenea-project/types/editor-shell.d.ts`, `crates/ludenea-project/src/types.rs:8-18`.
- Acceptance: @ludenea/editor-shell exports EDITOR_API_VERSION (major.minor); definePlugin records it in the definition; the loader refuses another major before activate with plugin X was written for editor API 1; this editor provides 2 (see CHANGELOG, Plugin API); the bundled declarations carry the version in their header; a test pinning the exported member list forces a deliberate bump.
- First failing test: editor/packages/editor-shell/test/loader.test.ts a_plugin_for_another_api_major_is_refused_with_both_versions.

#### VER-10: Binary formats carry a kind tag

- Priority P3, kind put-in-place, effort M, golden impact possible.
- Findings: dx-live-15.
- Depends on: VER-2.
- Locations and files: `crates/ludenea-ecs/src/snapshot.rs:65-80`, `crates/ludenea-app/src/trace.rs`, `crates/ludenea-app/src/replay.rs:82-104`, `crates/ludenea-app/src/input_log.rs:33-45`, `crates/ludenea-app/src/edit.rs`, `crates/ludenea-cli/src/commands.rs:34-36`.
- Acceptance: Trace, replay, snapshot, input log and edit log start with a 4-byte magic checked before the version; untagged files of the current versions still read for one release; a cross-fed file fails with x.trace is a trace, not a snapshot; run on a .replay points to verify.
- First failing test: crates/ludenea-cli/tests/cli.rs diff_of_a_trace_says_it_is_a_trace.

#### VER-11: Format types keep their version field private

- Priority P3, kind fix, effort S, golden impact none.
- Findings: versioning-12.
- Depends on: VER-2.
- Locations and files: `crates/ludenea-app/src/edit.rs:71-76,134-136`, `crates/ludenea-app/src/input_log.rs:10-16`, `crates/ludenea-app/src/trace.rs:12-21`, `crates/ludenea-app/src/replay.rs:18-36`, `crates/ludenea-ecs/src/snapshot.rs:20-32`, `crates/ludenea-cli/src/commands.rs:228-233`.
- Acceptance: version is private on EditLog, InputLog, Trace, Replay and Snapshot with a version() accessor; Trace::new exists; the legacy edit types sit in a doc(hidden) migrate module; a compile_fail doctest shows EditLog { version: 1, .. } no longer compiles.
- First failing test: compile_fail doctest in crates/ludenea-app/src/edit.rs.

#### VER-12: A host-owned session file the command line tool can read

- Priority P3, kind put-in-place, effort L, golden impact none.
- Findings: ambition-9.
- Depends on: VER-5, VER-6, VER-7.
- Locations and files: `crates/ludenea-control/src/session.rs:141-162,1301-1323`, `crates/ludenea-control/src/protocol.rs:283-305,496-512`, `editor/packages/editor-shell/src/stores/document.ts:69-88`, `crates/ludenea-cli/src/commands.rs`.
- Acceptance: ExportSession returns versioned postcard bytes of a SessionFile (scene, saved scene, logs, undo history, branches, unsaved assets, tick) and LoadSession takes them; the editor's wrapper stores those bytes only; run --session <file> --branch <name> --replay out.replay exports a branch from the command line.
- First failing test: crates/ludenea-control/tests/session.rs a_session_file_of_another_version_is_refused_with_its_version.

### ERR: Error architecture (grade C)

Subsystem: cross-cutting: error types, protocol errors, editor error channel, command line errors.

**Problem.** Rust errors are typed and mostly actionable, but the editor has no public error channel (palette, shortcut, viewport and divergence failures reach only the devtools console), protocol errors flatten into a code plus text that the editor parses with regular expressions, value errors give positions inside the value instead of the file, binary decode errors blame game types for truncated files, the command line tool prints errors on stdout and drops scene warnings, and malformed handshakes are told to send Hello first.

**Target state.** One public editor.console used by every dispatcher; ProtocolError carries specific codes and a typed detail; every file error names the file with its line and column; every decode error separates a damaged container from a value mismatch; the command line tool writes errors and warnings to stderr with documented exit codes.

#### ERR-1: Public editor log; every command dispatcher reports failures there

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: shell-2, errors-3, sessions-8, plugins-13, ts-quality-5, ux-live-2, ambition-4, branches-11.
- Depends on: nothing.
- Locations and files: `editor/packages/editor-shell/src/editor.ts:23-125`, `editor/packages/editor-shell/src/palette/CommandPalette.tsx:58`, `editor/packages/editor-shell/src/shortcuts.ts:107`, `editor/packages/editor-builtins/src/console/index.tsx:41-51`, `editor/packages/editor-builtins/src/shared/index.ts:95-99`, `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:39`, `editor/packages/editor-builtins/src/viewport/overlay.ts:57`, `editor/packages/editor-builtins/src/divergence/CausalReport.tsx:16,26`, `editor/packages/editor-builtins/src/divergence/TraceViewer.tsx:44`, `editor/packages/editor-builtins/src/divergence/index.tsx:45,63,238`, `editor/packages/editor-shell/src/stores/timeline.ts:40-42`.
- Acceptance: Editor exposes console: { log(level, text, location?), report(error, context) } backed by a shell store buffered before the Console plugin activates; it replaces the createEditor log option and logToConsole's private WeakMap. The palette, shortcuts, showPanel, viewport gestures, overlays, divergence actions and timeline refresh report <command title> failed: <message> there; the palette also shows a transient role=alert line. console.error remains only before an editor exists.
- First failing test: editor/packages/editor-shell/test/palette.test.tsx a_failing_command_run_from_the_palette_is_logged_in_the_editor_console, and the same for a shortcut.

#### ERR-2: Structured protocol errors: specific codes and a typed detail

- Priority P2, kind deep-refactor, effort L, golden impact none.
- Findings: errors-1, scenes-11, protocol-9, ts-quality-7, ambition-10.
- Depends on: VER-3.
- Locations and files: `crates/ludenea-control/src/protocol.rs:720-766`, `crates/ludenea-control/src/host.rs:271,319,654-659,1351-1444`, `crates/ludenea-project/src/protocol.rs:250-253`, `editor/packages/editor-shell/src/assets.ts:35-79`, `crates/ludenea-ecs/src/error.rs:26`.
- Acceptance: ProtocolError and ProjectFailure gain detail: Option<ErrorDetail> (Location { file, asset, line, column, end }, Value { entity, address, component, resource, path }, Tick, Branch, AssetType). InvalidCommand is split into no_such_branch, root_branch, no_such_asset, unknown_asset_type, no_default_value, invalid_trace, invalid_edit_log and invalid_undo_history, mapped from the typed errors, and every_error_code is extended. The regular expressions and message substring checks in assets.ts are deleted. The NoDefault message names App::asset_with_default.
- First failing test: crates/ludenea-control/tests/protocol_v3.rs new_asset_value_without_default_has_code_no_default_value, plus a_broken_data_asset_error_carries_its_location.

#### ERR-3: Scene, prefab and override value errors located in their file

- Priority P2, kind fix, effort M, golden impact none.
- Findings: scenes-10, errors-2.
- Depends on: ERR-2, SCN-14.
- Locations and files: `crates/ludenea-app/src/scene.rs:178-203,352-362,1152-1158`, `crates/ludenea-app/src/prefab.rs:213-219,282-324`, `crates/ludenea-app/src/assets.rs:78-93`.
- Acceptance: Values are deserialized as borrowed RawValue so their byte offset in the file is known. SceneError::InvalidValue and UnknownComponent carry the source (scene path, or prefab id and path) and a file location. Overrides read override of ludenea::Pose at position.x in instance 6. The serde error names the stable component name, not the TryFrom mirror.
- First failing test: crates/ludenea-app/tests/scene.rs a_malformed_component_value_names_its_line_and_column_in_the_scene_file, plus a prefab variant asserting the prefab path.

#### ERR-4: Binary decode errors separate a damaged file from a value mismatch

- Priority P2, kind fix, effort M, golden impact none.
- Findings: replays-4, untrusted-input-7, errors-8, runtime-7.
- Depends on: VER-2.
- Locations and files: `crates/ludenea-app/src/replay.rs:39-55,87-104,236-244,301-309`, `crates/ludenea-app/src/input_log.rs:37-49`, `crates/ludenea-app/src/simulation.rs:26-28`, `crates/ludenea-cli/src/commands.rs:258-286`, `crates/ludenea-ecs/src/schedule.rs:85-94`.
- Acceptance: ReplayError::Malformed (the file is truncated or not a replay) is distinct from per-value Decode; the other game types hint appears only when fingerprints differ; SimError::InputLogDecode is distinct from the per-tick InputDecode; hints are never doubled. The command line tool prefixes decode errors with the file path, names the bad file in diff, and says cannot run the scene when no replay was asked. Golden mismatches say whether the scene hash or the inputs hash differs. A missing App::input names that call.
- First failing test: crates/ludenea-app/tests/replay.rs damaged_replay_is_not_blamed_on_game_types and crates/ludenea-cli/tests/cli.rs verify_of_a_truncated_replay_names_the_file_and_says_it_is_truncated.

#### ERR-5: Game command line tool: stderr, printed warnings, documented arguments and exit codes

- Priority P2, kind fix, effort S, golden impact none.
- Findings: errors-7, replays-11, scenes-13, dx-live-16.
- Depends on: nothing.
- Locations and files: `crates/ludenea-cli/src/lib.rs:36-63`, `crates/ludenea-cli/src/commands.rs:37-40,168-224`.
- Acceptance: run(app, args, out, err) writes errors and warning: <scene>: <warning> lines to err (stderr in main). Every argument has help text (the VerifyArgs scene doc moves to the field). about and after_help document exit codes 0, 1 and 2. run --snapshot <file> writes the final state. Replays are recognised by content, or the error says expected a .replay file.
- First failing test: crates/ludenea-cli/tests/cli.rs errors_go_to_the_error_stream and run_prints_stale_override_warnings_to_stderr.

#### ERR-6: Malformed handshakes answered with the decode reason

- Priority P2, kind fix, effort S, golden impact none.
- Findings: protocol-10, errors-10, projects-19.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/in_process.rs:72-118`, `crates/ludenea-control/src/host.rs:1408-1422`, `crates/ludenea-project/src/service.rs:171-189`.
- Acceptance: Before connection, a message that does not decode is refused with invalid_request: cannot read this message: <serde error with its path> and the expected hello shape; NotConnected stays only for a well-formed request sent before Hello. Request decode errors carry the JSON path (serde_path_to_error), in both the control host and the project service.
- First failing test: crates/ludenea-control/tests/stdio.rs a_malformed_hello_names_what_is_wrong, and a service.rs test a_hello_without_version_is_refused_naming_protocol_version.

### DET: Determinism enforcement and proof (grade B)

Subsystem: state hash, type-level guards, golden traces and replays.

**Problem.** The guarantee is well enforced and proven, but some holes remain: the hash of several serde shapes is not pinned (mutants survive), the assets hash leaf ignores each asset's type, usize can enter the state, a shared &World can mutate the simulation from an observer or drawer, the goldens never reach the parallel colored solve, and most features (kinematic bodies, rounded polygons, five of seven edit kinds, nested prefabs, several players) have no golden.

**Target state.** Every serde shape pinned; the state hash covers types; the type system forbids pointer-width types and mutation outside systems; a stress golden and a feature corpus verified on three operating systems, WebAssembly and three browser engines.

#### DET-1: Pin the canonical hash of every serde shape

- Priority P1, kind fix, effort S, golden impact none.
- Findings: test-strategy-1.
- Depends on: nothing.
- Locations and files: `crates/ludenea-ecs/tests/hash.rs (new)`, `crates/ludenea-ecs/src/hash.rs:208-528`, `.github/workflows/determinism.yml`.
- Acceptance: crates/ludenea-ecs/tests/hash.rs holds an all-shapes fixture with a pinned hash_value and a sensitivity test that changes each field in turn. cargo mutants on hash.rs leaves no surviving non-equivalent mutant, and CI runs it when hash.rs changes.
- First failing test: every_field_of_every_shape_changes_the_hash (fails today under the serialize_i32 returning Ok(()) mutant).

#### DET-2: The assets hash leaf includes each asset's type name

- Priority P2, kind fix, effort S, golden impact certain.
- Findings: ecs-15.
- Depends on: nothing.
- Locations and files: `crates/ludenea-ecs/src/assets.rs:141-150`, `crates/ludenea-ecs/src/hash.rs:101-106`, `golden/spawner.replay`.
- Acceptance: assets_hash hashes the length-prefixed type name before the bytes; golden/spawner.replay is re-recorded with the commit explaining why.
- First failing test: crates/ludenea-ecs/tests/assets.rs same_bytes_under_two_asset_types_hash_differently.

#### DET-3: usize leaves the Schema; Polygon::count becomes u32

- Priority P2, kind fix, effort S, golden impact certain.
- Findings: ecs-16.
- Depends on: ECS-8.
- Locations and files: `crates/ludenea-schema/src/lib.rs:120-124`, `crates/ludenea-physics/src/shape.rs:53,63`, `golden/`.
- Acceptance: usize has no Schema implementation and the on_unimplemented note says use u32 or u64: usize is 32 bits on wasm32; Polygon::count is u32; goldens re-recorded and explained.
- First failing test: compile_fail doctest: a component with a usize field fails to compile.

#### DET-4: &World is read-only outside the ECS

- Priority P2, kind fix, effort M, golden impact none.
- Findings: ecs-1.
- Depends on: nothing.
- Locations and files: `crates/ludenea-ecs/src/query.rs:471`, `crates/ludenea-ecs/src/system.rs:29-39,260-262`, `crates/ludenea-ecs/src/commands.rs:173-185`, `crates/ludenea-ecs/src/observer.rs:1-2`.
- Acceptance: Query::new is bounded by ReadOnlyQueryData (a crate-private constructor serves systems); SystemParam::fetch requires a capability only the schedule creates, so Commands, ResMut and EventWriter cannot be obtained from a debug drawer, picker or observer; the observer.rs module documents the enforced guarantee.
- First failing test: compile_fail doctests for Query::<&mut Pose>::new(&world), <ResMut<Score> as SystemParam>::fetch(&world) and <Commands as SystemParam>::fetch(&world), plus a passing test that Query::<(Entity, &Pose)>::new(&world) still works.

#### DET-5: A stress golden exercises the parallel colored solve

- Priority P2, kind put-in-place, effort M, golden impact certain.
- Findings: determinism-3.
- Depends on: FND-4.
- Locations and files: `golden/stress.replay (new)`, `examples/boxes/src/bin/record-replay.rs`, `examples/boxes/tests/golden.rs`, `crates/ludenea-physics/tests/threads.rs:166-177`, `.github/workflows/determinism.yml:24-54`, `editor/e2e/golden.spec.ts`, `README.md:13-15`.
- Acceptance: golden/stress.replay (over 512 dynamic bodies, a kinematic body, one color over twice SOLVER_CHUNK) is verified at 1, 2, 3, 8 and 16 threads on the three operating systems, under wasm32-wasip1 and in the browser golden test. A test asserts from the recorded scene that it still crosses SOLVER_CHUNK. README:13-15 says which golden exercises the parallel path.
- First failing test: examples/boxes/tests/golden.rs stress_replay_crosses_the_solver_chunk.

#### DET-6: A golden corpus covering every feature, verified in Chromium, Firefox and WebKit

- Priority P2, kind put-in-place, effort L, golden impact certain.
- Findings: determinism-5, delivery-8.
- Depends on: DET-5.
- Locations and files: `examples/boxes/src/bin/record-replay.rs`, `golden/features/ (new)`, `editor/e2e/playwright.config.ts:13-19`, `editor/e2e/golden.spec.ts`, `.github/workflows/determinism.yml`.
- Acceptance: One record-goldens binary writes golden/features/*.replay (kinematic, rounded and 8-vertex polygons, damping, nested prefab, all seven edit kinds, two players, a game event read across systems, raycast and overlap queries, an editor-recorded session with a branch). CI verifies them natively at 1, 2 and 8 threads, under wasm32-wasip1, and in Playwright projects chromium, firefox and webkit restricted to the golden spec.
- First failing test: examples/boxes/tests/golden.rs golden_corpus_covers_every_feature.

### QA: Test infrastructure (grade C)

Subsystem: cross-cutting: property and fuzz tests, end-to-end suite, test doubles.

**Problem.** Example-based suites and goldens are strong, but no decoder or protocol has a property or fuzz test (the auditors' scratch fuzzers found the panics), no randomized test checks session invariants (two real undo and redo bugs slipped through), the editor's fake peers are untyped and stale, the end-to-end suite misses promised workflows and depends on wall-clock windows, fixed ports and sleeps.

**Target state.** Bounded, seeded fuzz and sequence tests run in cargo test; typed fakes built from the generated types; the end-to-end suite covers every README workflow and synchronises on state, not time.

#### QA-1: Seeded fuzz tests for every decoder and both protocols

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: test-strategy-5, untrusted-input-10.
- Depends on: RUN-1, PHY-1, SES-1, ECS-3, SCN-3.
- Locations and files: `crates/ludenea-control/tests/fuzz_protocol.rs (new)`, `crates/ludenea-app/tests/fuzz_formats.rs (new)`, `crates/ludenea-ecs/tests/fuzz_snapshot.rs (new)`, `crates/ludenea-project/tests/fuzz_project.rs (new)`.
- Acceptance: Tests named fuzz_* run in cargo test with a fixed seed and a bounded budget (about 2,000 mutations per target, under 10 seconds each): Trace, InputLog, EditLog, Replay through play_replay, Snapshot through restore and diff, UndoHistory, BranchTree through Session::resume, scene and prefab RON, control lines through serve_stdio, ProjectService::answer. Each input yields Ok or a typed error, never a panic, and never schedules more than a bounded number of ticks.
- First failing test: crates/ludenea-control/tests/fuzz_protocol.rs arbitrary_client_lines_never_panic_the_host, seeded from golden/ and the auditors' corpus (SetThreads included).

#### QA-2: Seeded random command sequences check the session invariants

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: sessions-17.
- Depends on: SES-1, SES-2, SES-4.
- Locations and files: `crates/ludenea-control/tests/session_sequences.rs (new)`.
- Acceptance: crates/ludenea-control/tests/session_sequences.rs runs a fixed seed set on every build (seeds 1 and 72 of the audit probe included) and an ignored long variant. Operations: play, seek, edit, tick-0 scene edits, prefab overrides, asset edits, fork, switch, delete, undo, redo, export and resume, restore. After each step a fresh replay of export() matches every recorded hash and the timeline invariants hold.
- First failing test: random_sequences_keep_the_session_replayable (fails today on seeds 1 and 72).

#### QA-3: Shared all-shapes fixture for diff, TypeScript declarations, JSON wire and inspector fields

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: test-strategy-13.
- Depends on: DET-1.
- Locations and files: `crates/ludenea-ecs/src/diff.rs:191-386,507-576`, `crates/ludenea-schema/src/typescript.rs`, `crates/ludenea-control/src/json.rs`, `editor/packages/editor-builtins/test/fields.test.tsx`.
- Acceptance: One test-support component with every serde shape and a NaN value is used by a diff test per shape (path and left and right text asserted), a typescript_declarations comparison, a to_wire and from_wire round trip, and a game-info fixture for editor-builtins field tests (Option set and clear, tuple items).
- First failing test: crates/ludenea-ecs/tests/snapshot.rs diff_walks_every_shape.

#### QA-4: Typed fake peers for editor tests

- Priority P3, kind put-in-place, effort M, golden impact none.
- Findings: test-strategy-9, ts-quality-11.
- Depends on: VER-3.
- Locations and files: `editor/packages/editor-shell/test/support.ts:14-38`, `editor/packages/editor-builtins/test/support.tsx:7-42`, `editor/apps/web/test/app.test.ts`, `editor/apps/web/test/games.test.ts`, `editor/packages/control-client/test/client.test.ts:24,78`.
- Acceptance: @ludenea/control-client/testing exports FakeGame and FakeProject typed from the generated Command, Reply, Event and ErrorCode, with welcomes built from the generated versions. The five hand-built fakes are replaced, invented error codes are gone and typecheck rejects new ones, and test-double branches leave production code.
- First failing test: editor/packages/control-client/test/testing.test.ts fake_game_answers_each_command_with_a_reply_of_its_type.

#### QA-5: End-to-end coverage of every promised workflow

- Priority P2, kind put-in-place, effort L, golden impact none.
- Findings: scenes-19, delivery-16, test-strategy-12.
- Depends on: SCN-1, SCN-9, QA-6.
- Locations and files: `editor/e2e/prefabs.spec.ts`, `editor/e2e/data-assets.spec.ts (new)`, `editor/e2e/rebuild.spec.ts (new)`, `editor/e2e/branches.spec.ts`, `crates/ludenea-serve/tests/serve.rs`, `.github/workflows/determinism.yml:98-99`.
- Acceptance: End-to-end specs cover Apply to prefab after playing, Revert and Create prefab from selection with undo (prefabs.spec.ts); a data asset edit at a past tick, save, file content and a replay verified in the command line tool (data-assets.spec.ts); branch rename and delete (branches.spec.ts); a game code change, rebuild, kept session and reported first divergent tick (rebuild.spec.ts). A ludenea-serve test under --features embed-editor serves / and a hashed asset in CI.
- First failing test: editor/e2e/prefabs.spec.ts apply_to_prefab_after_playing_writes_the_override_value.

#### QA-6: Reliable end-to-end runs

- Priority P2, kind fix, effort M, golden impact none.
- Findings: delivery-11, test-strategy-18, delivery-21.
- Depends on: nothing.
- Locations and files: `editor/e2e/project.ts:22-99`, `editor/e2e/session.spec.ts:48-81`, `editor/e2e/smoke.spec.ts:44-47`, `editor/e2e/divergence.spec.ts:7-10`, `editor/e2e/playwright.config.ts`.
- Acceptance: serveProject passes --port 0 and reads the address; stop() awaits the exit before removing the folder; every play uses the tick-targeted form; the smoke spec's 2 second budget is gated behind PERF_STRICT; traces and screenshots are kept on failure; one openAsset helper (openScene renamed), plus showTab and editField; timeline() throws naming the shown text; a nightly job runs --repeat-each 3.
- First failing test: n/a (test infrastructure): pnpm -C editor e2e --repeat-each 3 is green.

#### QA-7: Watcher tests synchronise on sentinel files

- Priority P3, kind fix, effort M, golden impact none.
- Findings: test-strategy-10.
- Depends on: nothing.
- Locations and files: `crates/ludenea-project/tests/project.rs:190-370`, `crates/ludenea-project/tests/plugins.rs:419-500`.
- Acceptance: A sync(&changes) helper writes a unique sentinel in a watched directory and waits for its event; the project.rs and plugins.rs watcher tests use it instead of fixed sleeps and quiet windows, including the assertions that nothing happened.
- First failing test: crates/ludenea-project/tests/project.rs watch_reports_scene_change_on_disk rewritten with the barrier and run 50 times with --test-threads 16.

#### QA-8: Error tests assert the variant and the message

- Priority P3, kind fix, effort S, golden impact none.
- Findings: test-strategy-19.
- Depends on: ERR-4.
- Locations and files: `crates/ludenea-app/tests/edit.rs:147,249`, `crates/ludenea-app/tests/prefab_editing.rs:158,174`.
- Acceptance: The bare is_err assertions on user-facing error paths assert the variant and the key words of the message.
- First failing test: crates/ludenea-app/tests/edit.rs:249 rewritten as unordered_edit_log_names_the_ordering_problem.

#### QA-9: The worker host and the wasm panic hook are tested

- Priority P3, kind put-in-place, effort M, golden impact none.
- Findings: test-strategy-11.
- Depends on: RUN-2.
- Locations and files: `editor/packages/control-client/src/worker/host-worker.ts:30-75`, `crates/ludenea-control/src/worker.rs:8-63`.
- Acceptance: host-worker.ts unit tests with a stubbed self: a glue that fails to load posts crashed; strings sent before init are handled in order; after a crash nothing else is advanced or handled. An end-to-end fixture game that panics on an edit shows the crash with its tick and source location, and the session survives.
- First failing test: editor/packages/control-client/test/host-worker.test.ts a_glue_that_fails_to_load_posts_crashed_with_the_reason.

### PHY: Math and physics (grade C)

Subsystem: ludenea-math, ludenea-physics, physics editor plugin.

**Problem.** The narrow phase, the solver's thread independence and the coverage are excellent, but finite inputs still crash the editor host (a zero rotation the snapshot decoder rejects; NaN settings; huge densities and substep counts), public constructors build degenerate shapes, a collider without a Body is drawn and raycast as ground but ignored by the solver, polygons and rotations cannot be authored or edited, the physics panel floods the host with one request per body per tick, and deep stacks and large mass ratios fail.

**Target state.** Finite in, finite out, by validation at every boundary; one definition of what takes part in physics; polygons authored from their vertices with physics inspectors; the panel reads in bulk; stacking scenarios covered by regression tests.

#### PHY-1: A Pose rotation is always unit length, and checkpoint restore never panics

- Priority P0, kind fix, effort M, golden impact possible.
- Findings: physics-1.
- Depends on: nothing.
- Locations and files: `crates/ludenea-math/src/rot.rs:62-80`, `crates/ludenea-math/src/scalar.rs:51-73`, `crates/ludenea-physics/src/solver.rs:262-289`, `crates/ludenea-physics/src/step.rs:217-237`, `crates/ludenea-physics/src/components.rs:35-52`, `crates/ludenea-control/src/session.rs:1692`.
- Acceptance: Every moving body's per-substep rotation is clamped, kinematic bodies included. Rot::normalize and sin_cos return IDENTITY for a zero or non-finite magnitude (documented). The post-step check reuses the Pose validation and names the entity and field. The session turns the checkpoint expect at session.rs:1692 into a SessionError that keeps the decode reason.
- First failing test: crates/ludenea-physics/tests/pipeline.rs kinematic_spin_keeps_a_unit_rotation_and_restores (angular velocity 1e30, one step, unit rotation, the snapshot restores into a fresh simulation), plus a ludenea-control session test that seeks back afterwards without a panic.

#### PHY-2: Every physics input validated so the post-step assert is a true invariant

- Priority P0, kind fix, effort M, golden impact none.
- Findings: physics-2.
- Depends on: nothing.
- Locations and files: `crates/ludenea-physics/src/components.rs:221-243`, `crates/ludenea-physics/src/step.rs:224-237`, `crates/ludenea-physics/src/solver.rs:105-152`, `crates/ludenea-app/src/time.rs`.
- Acceptance: PhysicsSettings has a try_from mirror (finite gravity; substeps in a documented range such as 1 to 64; finite, non-negative hertz, damping, push velocity and thresholds). Time.dt is validated finite and positive. Kinematic linear speed is clamped like dynamic speed. Density and gravity times gravity_scale are capped with precise errors. The post-step message names the entity, the field and the step inputs. set_field of substeps 400000000 is refused instead of hanging the host.
- First failing test: crates/ludenea-physics/tests/pipeline.rs scene_with_nan_gravity_is_rejected, scene_with_huge_substeps_is_rejected and kinematic_body_at_huge_finite_speed_stays_finite.

#### PHY-3: Shapes built only through validating constructors; degenerate capsules handled

- Priority P1, kind fix, effort M, golden impact none.
- Findings: physics-7.
- Depends on: nothing.
- Locations and files: `crates/ludenea-physics/src/shape.rs:154-164,345-377`, `crates/ludenea-physics/src/components.rs:168-176`, `crates/ludenea-physics/src/manifold.rs:213-220`.
- Acceptance: Polygon::make_box, Circle::new and Capsule::new return Result<_, ShapeError>; Collider::new validates (or a documented try_new is the checked path); Shape::validate runs the polygon checks; collide_capsule_circle and collide_capsules treat a zero-length segment as a circle.
- First failing test: shape.rs make_box_rejects_zero_extent and pipeline.rs zero_length_capsule_is_rejected_by_collider_new.

#### PHY-4: A collider without a Body takes part as static ground

- Priority P1, kind fix, effort S, golden impact none.
- Findings: physics-4.
- Depends on: nothing.
- Locations and files: `crates/ludenea-physics/src/step.rs:70-96`, `crates/ludenea-physics/src/components.rs:156-176`.
- Acceptance: physics_step gathers (Entity, &Pose, &Collider) Without<Body> as static proxies (inverse mass 0, never written back), which matches the debug draw and the queries; the rule is documented on Collider.
- First failing test: pipeline.rs collider_without_body_acts_as_static_ground.

#### PHY-5: Polygons authored from vertices; physics inspectors for shapes and rotations; enum switches seeded

- Priority P1, kind deep-refactor, effort L, golden impact possible.
- Findings: physics-5, dx-live-5, ux-live-4, sessions-7.
- Depends on: VER-1, AUT-4.
- Locations and files: `crates/ludenea-physics/src/shape.rs:44-168`, `crates/ludenea-physics/src/components.rs:7-55`, `crates/ludenea-physics/editor/index.tsx:108-114`, `editor/packages/editor-builtins/src/inspector/fields/Field.tsx:176-213`, `editor/packages/editor-builtins/src/inspector/default-value.ts:5-58`, `crates/ludenea-project/template/scenes/main.scene.ron`, `examples/boxes/assets/`.
- Acceptance: Polygon's RON and JSON form is (points, radius), decoded through Polygon::new (bit-identical to make_box); scene and prefab versions are bumped with a migration reading the old form; set_field on points[i].x succeeds. The physics editor plugin registers inspectors through the public API: Pose rotation as an angle in degrees writing cos and sin together; Collider shape presets (circle, capsule, box half extents, editable vertices). EnumField seeds a new variant from same-named fields of the current one and, when the host refuses it, opens an inline form showing the reason.
- First failing test: shape.rs polygon_loads_from_vertices_only_and_round_trips_bit_identically, plus editor-builtins physics.test.tsx typing_30_degrees_sets_cos_and_sin_together and inspector.test.tsx switching_to_a_variant_whose_default_is_rejected_opens_its_form.

#### PHY-6: The physics panel reads in bulk once per shown frame; contacts are clickable

- Priority P1, kind fix, effort M, golden impact none.
- Findings: physics-12, plugins-15, ts-quality-6, ux-live-7, ambition-11, ux-live-23.
- Depends on: VER-3.
- Locations and files: `crates/ludenea-physics/editor/index.tsx:21-114`, `crates/ludenea-control/src/protocol.rs:214-218`, `crates/ludenea-control/src/host.rs`, `editor/packages/control-client/src/generated/EntityRow.ts`, `editor/e2e/performance.spec.ts:66`.
- Acceptance: QueryEntities gains components: Vec<String> and returns those values per row (protocol change, changelog). The panel issues one paged projection per refresh, keeps at most one read in flight coalesced to the latest tick, and refreshes at most a few times per second while playing. Contacts use the scene tree's labels and select both entities on click. A tick that was not recorded shows an empty state, not the raw host message. The default layout runs in the performance spec.
- First failing test: editor-builtins physics.test.tsx the_panel_sends_one_query_per_refresh (no get_entity, at most one read in flight over 10 ticked events), and introspection.rs query_entities_returns_requested_component_values.

#### PHY-7: Math edge cases defined; the speed clamp is overflow-safe; units and limits documented

- Priority P2, kind fix, effort M, golden impact possible.
- Findings: physics-6, physics-11.
- Depends on: nothing.
- Locations and files: `crates/ludenea-math/src/scalar.rs:39-73`, `crates/ludenea-math/src/rot.rs:21-24`, `crates/ludenea-math/src/vec2.rs:54-62`, `crates/ludenea-physics/src/solver.rs:10-11,262-281`, `crates/ludenea-physics/src/components.rs:59-71`, `README.md`.
- Acceptance: unwind_angle reduces with the exact remainder; from_angle of a non-finite angle returns IDENTITY; Vec2::length and normalize scale by the larger component; the maximum-speed clamp clamps (1e20 becomes the maximum, not 0). A committed math golden table (input to output bits over signed zeros, subnormals, 1e-30 to 1e30, infinities and NaN) passes natively and under wasmtime. Units, the speed cap and LINEAR_SLOP are documented on Body, PhysicsSettings and in the README.
- First failing test: scalar.rs from_angle_is_unit_for_any_input, vec2.rs normalize_of_huge_vector_is_unit, pipeline.rs velocity_above_max_speed_is_clamped_not_zeroed.

#### PHY-8: Untested public math functions get recorded-bits and property tests

- Priority P3, kind fix, effort S, golden impact none.
- Findings: test-strategy-14.
- Depends on: nothing.
- Locations and files: `crates/ludenea-math/src/aabb.rs:12-36`, `crates/ludenea-math/src/vec2.rs:57-135`, `crates/ludenea-math/src/scalar.rs:71-81`.
- Acceptance: Aabb::union, contains and perimeter, Vec2::abs, f32 times Vec2, MulAssign, Rot::angle, sin, cos and the zero-magnitude sin_cos branch have recorded-bits tests and properties; cargo mutants -p ludenea-math leaves no non-equivalent survivor.
- First failing test: vec2.rs normalizing_zero_or_a_tiny_vector_gives_zero.

#### PHY-9: Shape and component validation errors name the field, the value and the edge

- Priority P3, kind fix, effort S, golden impact none.
- Findings: physics-14.
- Depends on: ERR-3.
- Locations and files: `crates/ludenea-physics/src/shape.rs:11-23,109-118,352-371`, `crates/ludenea-physics/src/components.rs:39-55`.
- Acceptance: ShapeError::NonFinite { field }, NormalMismatch { edge, stored, expected } and NotConvex { vertex }; circle and capsule errors stop using InvalidPolygon; Pose and Body errors include the value; positions map to file lines through ERR-3.
- First failing test: shape.rs normal_mismatch_error_names_edge_and_expected_normal.

#### PHY-10: The debug draw shows the rounding radius of polygons

- Priority P3, kind fix, effort S, golden impact none.
- Findings: physics-15.
- Depends on: nothing.
- Locations and files: `crates/ludenea-physics/src/editor.rs:125-146`, `crates/ludenea-debug-draw/src/mesh.rs:96-137`.
- Acceptance: Rounded polygons are drawn as their offset outline: arcs at each vertex joined by offset edges, reusing mesh::arc.
- First failing test: crates/ludenea-physics/tests/editor.rs rounded_polygon_outline_encloses_its_radius.

#### PHY-11: Coincident circles separate

- Priority P3, kind fix, effort S, golden impact possible.
- Findings: physics-8.
- Depends on: nothing.
- Locations and files: `crates/ludenea-physics/src/manifold.rs:24-27,163-182`.
- Acceptance: round_contact falls back to a deterministic normal (the rotation of A applied to (0, 1)) when the offset is below EPSILON; the Manifold documentation is updated.
- First failing test: pipeline.rs coincident_circles_separate.

#### PHY-12: Deep stacks and large mass ratios hold

- Priority P2, kind fix, effort L, golden impact certain.
- Findings: physics-9.
- Depends on: DET-5.
- Locations and files: `crates/ludenea-physics/src/solver.rs:133-162,300-345`, `crates/ludenea-physics/tests/pipeline.rs`.
- Acceptance: Regression tests for a 20-box tower and 100:1 and 1000:1 mass ratios pass after aligning the soft step with Box2D v3 (point solve order, relax and restitution stages, substeps and contact hertz relative to the substep rate); goldens re-recorded and explained when hashes move.
- First failing test: pipeline.rs tower_of_twenty_boxes_stands_for_ten_seconds.

#### PHY-13: The broad phase caches static proxies; raycast and overlap are culled and allocation-free

- Priority P3, kind fix, effort M, golden impact none.
- Findings: physics-17, gap-5-7.
- Depends on: nothing.
- Locations and files: `crates/ludenea-physics/src/broad_phase.rs:17-41`, `crates/ludenea-physics/src/step.rs:73-94`, `crates/ludenea-physics/src/queries.rs:29-67,156-209`.
- Acceptance: Static proxies are recomputed only when their Pose or Collider changes; dynamic proxies sweep a sorted static list so static pairs are never visited; raycast rejects by slab test before building an allocation-free core; overlap_aabb reuses the proxies; tie rules unchanged; a static-level case joins the bench.
- First failing test: crates/ludenea-physics/tests/queries.rs random rays match the brute-force reference, plus an ignored release budget of 100 rays over 5,000 colliders under 1 ms.

#### PHY-14: Parallel stages gated by work; the host's default thread count is bounded

- Priority P3, kind fix, effort M, golden impact none.
- Findings: gap-5-5.
- Depends on: RUN-1, RUN-12.
- Locations and files: `crates/ludenea-physics/src/solver.rs:146-160,209-231`, `crates/ludenea-app/src/parallel.rs:98-103`, `crates/ludenea-control/src/stdio.rs:29-32,82-88`.
- Acceptance: Body stages and prepare enter the pool only above a work threshold; serve_stdio defaults to the smaller of available_parallelism and 4; the bench reports 1, 2, 4 and 8 threads at 5,000 and 20,000 contacting bodies.
- First failing test: Ignored timing test in crates/ludenea-physics/tests/threads.rs: 8 threads take at most 1.05 times one thread at 1,000 bodies.

#### PHY-15: One transform component owned by the core; the viewport learns it from the game description

- Priority P3, kind deep-refactor, effort M, golden impact none.
- Findings: physics-13.
- Depends on: PLG-3.
- Locations and files: `crates/ludenea-physics/src/components.rs:10-17`, `editor/packages/editor-builtins/src/viewport/state.ts:20-30`, `crates/ludenea-control/src/protocol.rs:140-158`.
- Acceptance: The core owns Pose (stable name ludenea::Pose kept) wrapping math::Transform; GameInfo names the transform component and its position path; the viewport's literal is deleted.
- First failing test: Viewport test dragging_moves_an_entity_whose_transform_comes_from_game_info.

#### PHY-16: Fast bodies do not tunnel through static walls

- Priority P3, kind put-in-place, effort L, golden impact certain.
- Findings: physics-3.
- Depends on: PHY-12.
- Locations and files: `crates/ludenea-physics/src/step.rs:18,86-93`, `crates/ludenea-physics/src/manifold.rs:126-129`, `crates/ludenea-physics/src/solver.rs`.
- Acceptance: Speculative margins scale with motion (speed times dt) so the existing speculative branch stops the body at the surface; an optional bullet flag covers dynamic pairs; supported speeds are documented.
- First failing test: pipeline.rs fast_ball_does_not_pass_through_thin_static_wall.

#### PHY-17: Physics essentials for real games: collision filters, sensors, rotation lock, richer contact events

- Priority P3, kind put-in-place, effort XL, golden impact possible.
- Findings: physics-10.
- Depends on: PHY-16, VER-4.
- Locations and files: `crates/ludenea-physics/src/broad_phase.rs:14-41`, `crates/ludenea-physics/src/events.rs:17-24`, `crates/ludenea-physics/src/components.rs`, `crates/ludenea-physics/src/queries.rs:156-209`, `README.md`.
- Acceptance: The README lists supported and deferred physics features. Collider gains category and mask bits honored by find_pairs and the queries, and a sensor flag (Begin and End only); Body gains lock_rotation; Begin events carry normal, point and approach speed; overlap_point and a raycast filter are public. Each is a versioned format change with defaults so older files load.
- First failing test: pipeline.rs filtered_pair_does_not_collide.

### ECS: ECS, schemas, snapshots and hashing (grade B)

Subsystem: ludenea-ecs, ludenea-schema, ludenea-macros, App registration.

**Problem.** Snapshot decoding and restore are robust and well covered, but several type-level traps remain: a recursive type aborts every tool with a stack overflow, restore accepts prefab assets the game cannot spawn, #[serde(default)] is refused so adding a field breaks every saved file, serde from and into mirrors let a schema disagree with its data while two separate remapping mechanisms depend on that agreement, misused systems produce generic trait errors, a reader ordered before every writer silently sees no events, and World::hash rebuilds the whole tree twice per tick.

**Target state.** Every registered type has a sound schema by construction; restore validates everything it accepts; component types can evolve; one fallible schema-guided walker rewrites references; compile errors and build errors name the fix; hashing streams once per tick.

#### ECS-1: Recursive types refused cleanly instead of aborting the process

- Priority P0, kind fix, effort S, golden impact none.
- Findings: ecs-8.
- Depends on: nothing.
- Locations and files: `crates/ludenea-schema/src/lib.rs:14-84`, `crates/ludenea-macros/src/lib.rs:90-201`, `crates/ludenea-app/src/simulation.rs:83,236-249`, `crates/ludenea-app/src/app.rs`.
- Acceptance: Derived schema() tracks the types in progress on a thread-local stack and, on re-entry, returns a TypeSchema::Recursive { name } marker; App::build and every registration path return an error naming the type and the advice (store children as Entity references until recursive schemas are supported). A direct self-reference fails at compile time with a spanned error.
- First failing test: compile_fail doctest in ludenea-schema/src/lib.rs for #[derive(Schema)] struct Node { children: Vec<Node> }, plus an app test that an indirectly recursive asset type makes build() return an error instead of overflowing the stack.

#### ECS-2: Recursive schemas supported through TypeSchema::Ref

- Priority P2, kind put-in-place, effort L, golden impact none.
- Findings: dx-live-1.
- Depends on: ECS-1, ECS-6.
- Locations and files: `crates/ludenea-schema/src/lib.rs`, `crates/ludenea-schema/src/typescript.rs`, `crates/ludenea-ecs/src/diff.rs`, `editor/packages/editor-builtins/src/inspector/fields/Field.tsx`.
- Acceptance: Re-entry yields Ref { name }, resolved through a registry of named schemas; the diff, entity and handle walkers, the JSON wire, the TypeScript generator (recursive alias) and the editor's generic form follow it; a dialogue-tree asset registers, saves, loads and edits.
- First failing test: App test: App::new().asset::<Dialogue>().build() succeeds and typescript_declarations contains replies: (Dialogue)[].

#### ECS-3: Restore and asset insertion validate prefab assets

- Priority P0, kind fix, effort M, golden impact certain.
- Findings: ecs-3.
- Depends on: nothing.
- Locations and files: `crates/ludenea-ecs/src/snapshot.rs:125-238`, `crates/ludenea-ecs/src/assets.rs:153-161`, `crates/ludenea-ecs/src/prefab.rs:20-30,88-120`, `crates/ludenea-ecs/src/world.rs`.
- Acceptance: AssetType gains an optional validate hook; the Prefab hook checks that every component name is registered, every value decodes exactly and addresses are unique. insert_asset, insert_asset_ron and restore refuse with SnapshotError::Decode naming the asset and the component, leaving the world unchanged. Whether a type mentions Entity is computed once at register_component and no longer read from the snapshot.
- First failing test: crates/ludenea-ecs/tests/snapshot.rs restoring_a_prefab_asset_with_an_unregistered_component_is_refused (hash_tree unchanged).

#### ECS-4: Component types can evolve: #[serde(default)] accepted, moved types keep their name

- Priority P1, kind fix, effort M, golden impact none.
- Findings: ecs-2, dx-live-4, dx-live-12.
- Depends on: ECS-9.
- Locations and files: `crates/ludenea-macros/src/lib.rs:54-80,203-231`, `crates/ludenea-ecs/src/component.rs:10-17`, `crates/ludenea-app/src/scene.rs:41-47`.
- Acceptance: default (field and container level, with or without a path) leaves SHAPE_ATTRIBUTES while the skip attributes stay refused. #[component(alias = "old::Name")] (and the resource, asset and event equivalents) is accepted when reading scenes, snapshots and replays, never written. An unknown name whose last segment matches a registered type adds pin its name with #[component(name = ...)]. The Component documentation carries the evolution guide.
- First failing test: crates/ludenea-app/tests/scene.rs a_component_with_a_defaulted_new_field_loads_old_scenes (hash equal to writing the default explicitly), plus a trybuild case where skip_serializing_if still fails.

#### ECS-5: The Schema derive follows serde from and into mirrors by construction

- Priority P1, kind fix, effort S, golden impact possible.
- Findings: ecs-7.
- Depends on: nothing.
- Locations and files: `crates/ludenea-macros/src/lib.rs:78-79,135-153,233-240`, `crates/ludenea-physics/src/shape.rs:48-66`.
- Acceptance: A container with into = X gets its schema from X's schema renamed to the outer type; from or try_from must name the same X or the derive emits compile_error; PolygonData derives Schema.
- First failing test: tests/schema.rs a_mirrored_component_has_its_mirrors_schema, and a compile_fail doctest for from = A, into = B.

#### ECS-6: One fallible schema-guided rewriter for entity and handle references

- Priority P1, kind deep-refactor, effort L, golden impact none.
- Findings: ecs-12, rust-quality-1.
- Depends on: ECS-5.
- Locations and files: `crates/ludenea-ecs/src/component.rs:56-151`, `crates/ludenea-ecs/src/entity.rs:15-31`, `crates/ludenea-ecs/src/prefab.rs:115-120`, `crates/ludenea-app/src/scene.rs:394-409,834-852,1162-1431`.
- Acceptance: rewrite_references(schema, bytes, on_entity, on_handle) returning Result lives in ludenea-ecs next to Codec and reports PathSegment paths. prefab::spawn, the scene loader and the scene saver use it. The thread-local remap in Entity::deserialize and scene.rs's Rewriter are deleted. Truncated or mismatched bytes return errors, never panic.
- First failing test: ludenea-ecs unit test: for entities in a list, an option, a map value and an enum variant the walker gives the bytes remap_entities gives today, and truncated bytes return Err; app test: a component with a reordered mirror loads with the right entity target.

#### ECS-7: Reading events before every writer is a build error

- Priority P2, kind fix, effort S, golden impact none.
- Findings: ecs-6, dx-live-3.
- Depends on: nothing.
- Locations and files: `crates/ludenea-ecs/src/schedule.rs:119-163`, `crates/ludenea-ecs/src/event.rs:1-99`.
- Acceptance: ScheduleBuilder::build returns BuildError::EventsNeverSeen { reader, event, writers } with the hint order it .after(<label>) when every writer of an event runs after its reader; Events documents the one-tick lifetime.
- First failing test: tests/systems.rs: a Hit reader registered before its only writer fails to build naming both systems; with .after it builds and sees every event.

#### ECS-8: Compiler diagnostics for misused systems, parameters, components and schemas

- Priority P2, kind fix, effort S, golden impact none.
- Findings: ecs-5, dx-live-8.
- Depends on: ECS-9.
- Locations and files: `crates/ludenea-ecs/src/system.rs:29-183`, `crates/ludenea-ecs/src/schedule.rs:25-64`, `crates/ludenea-schema/src/lib.rs`, `crates/ludenea-macros/src/lib.rs`.
- Acceptance: #[diagnostic::on_unimplemented] on IntoSystem, IntoSystemConfig, SystemParam, Component, Resource, Asset, Event, QueryData and Schema (the Schema note names BTreeMap and BTreeSet for HashMap fields); derive(Query) emits per-field bounds with quote_spanned!.
- First failing test: trybuild UI test tests/ui/system_with_res_inputs.rs whose .stderr names Runtime<Inputs<I>>.

#### ECS-9: A trybuild UI suite for the derive macros; name overrides validated

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: ecs-14, errors-13.
- Depends on: nothing.
- Locations and files: `crates/ludenea-macros/src/lib.rs:41-69,247-312,379-450`, `crates/ludenea-macros/Cargo.toml`, `crates/ludenea-macros/tests/ui/ (new)`.
- Acceptance: crates/ludenea-macros/tests/ui holds a .stderr snapshot for every syn::Error branch and every refused serde attribute. Name overrides are non-empty, free of surrounding whitespace, and not ludenea::Entities or ludenea::Assets. A generic Component, Resource, Asset or Event gets its own message.
- First failing test: tests/ui/query_without_lifetime.rs pinning a query struct takes exactly one lifetime parameter at the generics span.

#### ECS-10: Prefab overrides: precise messages, a fallible form, validation before spawning

- Priority P2, kind fix, effort M, golden impact none.
- Findings: ecs-4, test-strategy-4.
- Depends on: RUN-2.
- Locations and files: `crates/ludenea-ecs/src/prefab.rs:43-141`, `crates/ludenea-ecs/src/commands.rs:103-120`, `crates/ludenea-app/tests/spawn_prefab.rs:242`.
- Acceptance: Messages name the prefab id and path, the local identifier and the component, with the fix. set_if_present skips missing targets. Override targets are checked before any entity spawns, so a failure never leaves a half-spawned instance. The should_panic test is replaced by one asserting the reported error and a simulation that keeps running deterministically.
- First failing test: crates/ludenea-app/tests/spawn_prefab.rs an_override_naming_a_missing_local_identifier_is_reported_not_panicked.

#### ECS-11: Type-erased World APIs return an error on a wrong type

- Priority P2, kind fix, effort M, golden impact none.
- Findings: ecs-10.
- Depends on: nothing.
- Locations and files: `crates/ludenea-ecs/src/world.rs:112-116,314-350`, `crates/ludenea-ecs/src/component.rs:33-63`.
- Acceptance: insert_boxed and insert_resource_boxed return EcsError::WrongType; Codec's fields are private behind fallible methods; the per-format accessors go through one with_component_named.
- First failing test: tests/snapshot.rs: insert_boxed of a u8 under Health returns WrongType and the hash is unchanged.

#### ECS-12: Registration errors phrased in App terms

- Priority P3, kind fix, effort S, golden impact none.
- Findings: ecs-9, dx-live-23.
- Depends on: nothing.
- Locations and files: `crates/ludenea-ecs/src/error.rs:7-27`, `crates/ludenea-ecs/src/query.rs:93-96`, `crates/ludenea-ecs/src/schedule.rs:92-99`, `crates/ludenea-app/src/app.rs:133-147`.
- Acceptance: Hints name App::component, resource, event, asset and input, and PhysicsPlugin for ludenea physics names; DuplicateName covers #[event(name)]; QueryError::DoesNotMatch names the missing or excluding component.
- First failing test: tests/query.rs: get() on Query<(&Pose, &Velocity)> for an entity without Velocity names Velocity.

#### ECS-13: Adding a plugin twice is refused

- Priority P2, kind fix, effort S, golden impact none.
- Findings: dx-live-9.
- Depends on: nothing.
- Locations and files: `crates/ludenea-app/src/app.rs:41-43,98-101`.
- Acceptance: App::plugin records the plugin's TypeId and build returns AppError::DuplicatePlugin { name } on a second add.
- First failing test: App::new().plugin(PhysicsPlugin).plugin(PhysicsPlugin).build() returns DuplicatePlugin naming the plugin.

#### ECS-14: Generated TypeScript never shadows global names

- Priority P2, kind fix, effort S, golden impact none.
- Findings: ecs-13, plugins-10.
- Depends on: nothing.
- Locations and files: `crates/ludenea-schema/src/typescript.rs:30-39,124-186,232-275`, `crates/ludenea-project/src/types.rs:39-51`.
- Acceptance: Lists are emitted as (T)[] and string maps as { [key: string]: V }; ComponentTypes, AssetTypes and every other declared name are reserved; collision names are unique by construction; one unit test per mapping rule; the ignored integration test type-checks game.d.ts with skipLibCheck false.
- First failing test: typescript.rs game_types_named_like_typescript_globals_do_not_shadow_them.

#### ECS-15: One field path convention on the wire

- Priority P2, kind deep-refactor, effort M, golden impact none.
- Findings: ecs-11, rust-quality-9.
- Depends on: VER-3, ERR-2.
- Locations and files: `crates/ludenea-ecs/src/diff.rs:16-24,290-359`, `crates/ludenea-schema/src/entities.rs:7-118`, `crates/ludenea-app/src/path.rs`, `editor/packages/editor-builtins/src/divergence/SnapshotDiff.tsx:6`.
- Acceptance: PathSegment moves to ludenea-schema; Difference.path, Cause::Event.path and EntityRefAt use Vec<PathSegment>; ludenea_app's Path is the single Display, so the command line diff text stays stable; protocol bumped and bindings regenerated.
- First failing test: tests/compare.rs: a DiffSnapshots path inside a tuple-variant field, passed unchanged to SetField, edits that field.

#### ECS-16: Commands name the entity they spawn

- Priority P3, kind put-in-place, effort M, golden impact none.
- Findings: ecs-18, dx-live-10.
- Depends on: nothing.
- Locations and files: `crates/ludenea-ecs/src/commands.rs:56-161`, `crates/ludenea-ecs/src/entity.rs`, `crates/ludenea-ecs/src/world.rs:248`.
- Acceptance: World::reserve_entity on the deterministic allocator; SpawnCommands::id(); spawn_prefab returns the root (or every) entity; reservation follows issue order, so determinism is unchanged.
- First failing test: tests/systems.rs: one system spawns A and B where B holds A; after a tick B names A's live entity, with equal hashes at 1 and 8 threads.

#### ECS-17: System labels derived from types; duplicate names refused

- Priority P3, kind fix, effort S, golden impact none.
- Findings: ecs-19.
- Depends on: nothing.
- Locations and files: `crates/ludenea-ecs/src/schedule.rs:11-14,123-128`, `crates/ludenea-physics/src/plugin.rs:14-21`, `crates/ludenea-macros/src/lib.rs`.
- Acceptance: #[derive(SystemLabel)] names a label after its module path and type; physics::Step uses it; two label types sharing a name fail the build with DuplicateLabel naming both.
- First failing test: tests/schedule.rs two_label_types_with_one_name_fail_the_build.

#### ECS-18: Names state intent, not history; diff_snapshots split

- Priority P3, kind fix, effort S, golden impact none.
- Findings: ecs-20, rust-quality-14.
- Depends on: nothing.
- Locations and files: `crates/ludenea-ecs/src/diff.rs:67-186,313,390,421,457-464`, `crates/ludenea-app/src/app.rs:45`, `crates/ludenea-app/src/trace.rs:70`, `crates/ludenea-control/src/undo.rs`, `crates/ludenea-control/src/session.rs:740,787`.
- Acceptance: milestone_1 becomes canonical_ron_text; milestone wording leaves code, rustdoc and tests (formats are named by version); diff_snapshots is split into entities, resources, components and assets; the uncovered ron_text branches are tested.
- First failing test: tests/snapshot.rs: differences inside a tuple variant and an integer-keyed map render the pinned text.

#### ECS-19: World::hash streams; the host hashes each tick once

- Priority P2, kind fix, effort M, golden impact none.
- Findings: gap-5-1.
- Depends on: DET-1.
- Locations and files: `crates/ludenea-ecs/src/hash.rs:91-138`, `crates/ludenea-control/src/session.rs:1704-1711`, `crates/ludenea-control/src/host.rs:124-196`.
- Acceptance: World::hash walks the entities with one cursor per storage and feeds the root hasher directly (the same bytes, no tree, no per-component String); hash_tree stays for the divergence tools; Session::step returns the recorded hash and the host reuses it, so a played tick is hashed once.
- First failing test: ludenea-control test with a counting Serialize component: one Host::advance tick hashes once; ludenea-ecs test: World::hash() equals hash_tree().root on varied worlds.

#### ECS-20: ECS costs follow each storage's own size

- Priority P3, kind fix, effort M, golden impact none.
- Findings: gap-5-9.
- Depends on: ECS-19.
- Locations and files: `crates/ludenea-ecs/src/hash.rs:97-100`, `crates/ludenea-ecs/src/entity.rs:58-91`, `crates/ludenea-ecs/src/world.rs:32-33`.
- Acceptance: EntitiesState is hashed as little-endian byte slices with the same length prefixes (hash unchanged); component storages drop trailing empty slots on despawn and replace_all. Reusing the lowest free index stays out of scope because it changes entity identifiers.
- First failing test: ludenea-ecs: a fixed EntitiesState hashes to a recorded constant; an ignored release test keeps World::hash after a 100,000-entity burst within 1.2 times the time before it.

### SCN: Scenes, prefabs and data assets (grade C)

Subsystem: ludenea-app scene, prefab, assets and meta modules; prefab and data-asset session operations.

**Problem.** Loading is transactional, identifiers are never reused and override text splicing keeps files byte for byte, but editor prefab operations capture the live world at the current tick and write simulated values into the tick-0 scene and the prefab file, a prefab document can contain itself, identifier exhaustion panics, scenes persist the engine clock (overriding App::tick_rate), data asset saves drop comments, instances cannot be removed, references into a new prefab silently die, and scene.rs and prefab.rs mix many concerns in about 1,400 lines each.

**Target state.** Every prefab and asset operation works from the tick-0 state as one undoable session operation; files keep their text when edited; engine-owned resources stay out of scene files; scene and prefab code is split by concern with a tested RON span parser.

#### SCN-1: Create prefab and Apply to prefab work from the tick-0 state; Apply is one undoable operation

- Priority P0, kind fix, effort M, golden impact none.
- Findings: scenes-1, scenes-9, ux-live-6.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/host.rs:681-683,739-765,951-1006`, `crates/ludenea-control/src/session.rs:1029-1096`, `editor/packages/editor-builtins/src/prefabs/index.tsx:121-130`.
- Acceptance: Session::create_prefab and Session::apply_to_prefab replace the host's live-world code. Create restores checkpoint 0, writes the prefab and replays to the current tick. Apply reads the override's value from the scene text, updates the prefab text in the session's assets (marked unsaved, written by Save), removes the override, all under one undo entry; undo restores both.
- First failing test: crates/ludenea-control/tests/protocol_v3.rs apply_to_prefab_after_playing_writes_the_override_value, plus session_assets.rs create_prefab_after_playing_keeps_the_tick_0_values and apply_to_prefab_moves_the_override_into_the_prefab.

#### SCN-2: Prefab documents are cycle-checked against themselves

- Priority P0, kind fix, effort M, golden impact none.
- Findings: scenes-2.
- Depends on: VER-3.
- Locations and files: `crates/ludenea-control/src/session.rs:89-93,1051-1060,2147-2165`, `crates/ludenea-control/src/host.rs:518-522`, `crates/ludenea-app/src/prefab.rs:664-682`.
- Acceptance: Document::Prefab carries its AssetId (protocol change, changelog entry); a prefab session loads through the overlay so its own text replaces its id; every edit is checked for cycles; SaveScene of a prefab runs save_prefab_checked before marking it saved.
- First failing test: crates/ludenea-control/tests/session_assets.rs placing_a_prefab_inside_itself_is_refused (SceneError::PrefabCycle, text unchanged).

#### SCN-3: Local identifier exhaustion and edge inputs return errors, never panic

- Priority P0, kind fix, effort M, golden impact none.
- Findings: untrusted-input-2, scenes-23.
- Depends on: nothing.
- Locations and files: `crates/ludenea-app/src/scene.rs:228-252,581-640,700-740,1132-1150`, `crates/ludenea-app/src/prefab.rs:422-430`, `crates/ludenea-control/src/session.rs:1988-1997`.
- Acceptance: parse_scene refuses entity ids at or above next_id; after returns Option; save_scene_keeping and write_scene return Result with SceneError::IdentifiersExhausted { next_id } (renumber the scene); place_instance uses checked_add; override_value_ron returns UnknownComponent; a Spawn that would exhaust identifiers is an InvalidEdit.
- First failing test: crates/ludenea-app/tests/scene.rs a_save_that_needs_more_local_identifiers_than_remain_is_refused, and prefab_editing.rs place_instance_at_the_last_identifier_is_refused.

#### SCN-4: Create prefab from selection refuses references into the selection

- Priority P1, kind fix, effort M, golden impact none.
- Findings: scenes-6.
- Depends on: ECS-6.
- Locations and files: `crates/ludenea-control/src/session.rs:1065-1096`, `crates/ludenea-app/src/prefab.rs:433-457`, `crates/ludenea-app/src/scene.rs:757-798`.
- Acceptance: Before removing anything, replace_with_instance walks the remaining scene entities at tick 0 and refuses with SceneError::InboundReference { entity, component, path, target }.
- First failing test: prefab_editing.rs replace_with_instance_refuses_references_into_the_selection (using the existing Link component).

#### SCN-5: Remove and unpack prefab instances

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: scenes-5.
- Depends on: VER-3.
- Locations and files: `crates/ludenea-app/src/prefab.rs:753-761`, `crates/ludenea-app/src/scene.rs:97-101`, `crates/ludenea-control/src/session.rs:2028-2030`, `crates/ludenea-control/src/protocol.rs`, `editor/packages/editor-builtins/src/prefabs/InstanceGroups.tsx:29-55`.
- Acceptance: prefab::remove_instance and an undoable Session::remove_instance; RemoveInstance and UnpackInstance protocol commands; a Remove instance command bound to Delete on an instance group; the InstanceEntity refusal names that command.
- First failing test: prefab_editing.rs remove_instance_takes_its_entry_out and session_assets.rs remove_instance_removes_its_entities_and_undoes.

#### SCN-6: The foreign-reference check uses schemas, not field names

- Priority P2, kind fix, effort M, golden impact none.
- Findings: scenes-12.
- Depends on: ECS-6.
- Locations and files: `crates/ludenea-app/src/prefab.rs:631-661,763-807`, `crates/ludenea-control/src/host.rs:761-763`, `crates/ludenea-control/src/session.rs:1083`.
- Acceptance: save_prefab_checked takes the registry and uses the shared rewriter; the field-name heuristic is deleted; a struct with index and generation fields is not taken for an Entity.
- First failing test: prefab_editing.rs a_component_with_index_and_generation_fields_is_not_an_entity_reference.

#### SCN-7: Version 1 scenes upgrade before prefab operations

- Priority P2, kind fix, effort S, golden impact none.
- Findings: scenes-14.
- Depends on: nothing.
- Locations and files: `crates/ludenea-app/src/prefab.rs:422-457`, `crates/ludenea-control/src/session.rs:1051-1096`.
- Acceptance: place_instance and replace_with_instance upgrade a version 1 scene first (restore tick 0, save_scene_keeping) under the same undo entry; the pure functions return SceneError::NeedsVersion2.
- First failing test: session_assets.rs place_instance_on_a_version_1_scene_upgrades_it.

#### SCN-8: Engine-owned resources stay out of scene files

- Priority P1, kind fix, effort M, golden impact certain.
- Findings: runtime-1, dx-live-2, runtime-16.
- Depends on: VER-4.
- Locations and files: `crates/ludenea-app/src/time.rs:6-11`, `crates/ludenea-app/src/scene.rs:524-529,565-574,718-727,1055-1072`, `crates/ludenea-app/src/rng.rs:6-26`, `crates/ludenea-app/src/app.rs:185-213`, `crates/ludenea-project/template/scenes/main.scene.ron:4`, `examples/boxes/assets/`.
- Acceptance: Time has private fields written only by the engine. save_scene and save_scene_keeping write neither Time nor an empty ContactCache. Loading ignores a stored Time matching the app's clock and warns when it differs (saved at 60 Hz; the game ticks at 30 Hz). Rng accepts a (seed, stream) form. App::tick_rate is tested at the app and host levels. The template and boxes scenes are rewritten.
- First failing test: crates/ludenea-app/tests/simulation.rs scene_clock_never_overrides_the_tick_rate and saved_scene_omits_the_clock, plus scene.rs scene_rng_seed_form_matches_rng_new.

#### SCN-9: Data asset edits keep the file's comments and layout

- Priority P1, kind fix, effort M, golden impact none.
- Findings: scenes-16, ux-live-5.
- Depends on: SCN-14.
- Locations and files: `crates/ludenea-control/src/session.rs:1022-1025,1129-1150,1866-1889`, `crates/ludenea-app/src/prefab.rs:1045`, `crates/ludenea-control/tests/session_assets.rs:388-389`.
- Acceptance: At tick 0, SetAssetField splices the changed field into the file text with the existing span editor; a whole-value replacement falls back to the canonical form after warning that comments will be lost; the test asserting the compact form is replaced.
- First failing test: session_assets.rs asset_field_edit_keeps_the_file_comments.

#### SCN-10: Data assets the open scene does not load are viewable and editable

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: scenes-7.
- Depends on: ERR-2.
- Locations and files: `crates/ludenea-control/src/host.rs:632-641,1089-1127`, `crates/ludenea-control/src/values.rs:106-134`, `crates/ludenea-app/src/edit.rs:231-232`.
- Acceptance: GetAsset decodes an unloaded asset's text with its meta type; at tick 0 SetAssetField rewrites the text as an undoable asset entry; past tick 0 the refusal says not loaded by the open scene; edit it at tick 0 or open a scene that uses it; not in the project and not loaded are separate errors.
- First failing test: protocol_v3.rs get_asset_of_an_asset_the_scene_does_not_reach_returns_its_file_value.

#### SCN-11: The host encodes new data asset values; the TypeScript RON writer is deleted

- Priority P2, kind fix, effort S, golden impact none.
- Findings: scenes-17.
- Depends on: VER-3.
- Locations and files: `crates/ludenea-control/src/host.rs:654-659`, `editor/packages/editor-builtins/src/assets/value.ts:66-121`, `editor/packages/editor-builtins/src/assets/index.tsx:121-128`.
- Acceptance: NewAssetValue accepts an optional JSON value and replies with the codec's RON text; toRon is deleted.
- First failing test: protocol_v3.rs new_asset_value_encodes_a_given_json_value, with a string holding a backspace control character that round-trips through decode_ron.

#### SCN-12: Data asset files read without a synthetic wrapper

- Priority P3, kind fix, effort M, golden impact possible.
- Findings: scenes-15.
- Depends on: SCN-14.
- Locations and files: `crates/ludenea-app/src/assets.rs:62-127`, `crates/ludenea-app/src/meta.rs:161-165`.
- Acceptance: FoundAsset carries the type beside the text; the file text is decoded unchanged with ron::Options taken from its header; a final line comment and an extensions header load; replays recorded before still decode.
- First failing test: scene_assets.rs a_data_asset_ending_in_a_line_comment_loads.

#### SCN-13: Added and removed instance components are visible and revertible

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: scenes-18.
- Depends on: VER-3.
- Locations and files: `crates/ludenea-app/src/prefab.rs:46-71,265-298`, `editor/packages/editor-builtins/src/prefabs/OverrideMark.tsx:18-24`.
- Acceptance: InstanceInfo reports added and removed components per entity; the inspector marks added components with Revert and lists removed ones with Restore from prefab (a new text operation).
- First failing test: prefab.rs instances_report_added_and_removed_components and prefabs.test.tsx a_removed_prefab_component_is_listed_with_restore.

#### SCN-14: Split scene.rs and prefab.rs by concern

- Priority P2, kind deep-refactor, effort L, golden impact none.
- Findings: scenes-20, rust-quality-12.
- Depends on: ECS-6.
- Locations and files: `crates/ludenea-app/src/scene.rs:1-1431`, `crates/ludenea-app/src/prefab.rs:1-1382`, `crates/ludenea-physics/src/step.rs:64-239`.
- Acceptance: scene/{format, load, save}.rs, assets/resolve.rs, prefab/{resolve, edit}.rs and ron_text.rs (the lossless RON span parser with its own tests); physics_step split into gather, narrow phase and write-back. Behaviour unchanged: the app and control suites and both goldens pass with identical hashes.
- First failing test: ron_text unit tests on malformed input (unterminated string, character literal ending in a backslash, unclosed block comment, unclosed raw string) each returning None.

#### SCN-15: Prefab authoring friction removed

- Priority P3, kind fix, effort M, golden impact none.
- Findings: scenes-24, ux-live-19.
- Depends on: AUT-2.
- Locations and files: `editor/packages/editor-builtins/src/prefabs/index.tsx:58-64,114-166`, `editor/packages/editor-builtins/src/prefabs/InstanceGroups.tsx:45-49`, `crates/ludenea-project/src/lib.rs:49-50`.
- Acceptance: Instance groups use the scene tree's labels; Create prefab from selection asks for the prefab name (prefilled, validated); the placed instance is selected and placed at the viewport centre; KindMismatch names the expected extension.
- First failing test: prefabs.test.tsx create_from_selection_with_name_crate_creates_assets_crate_prefab_ron and place_leaves_the_new_instance_selected.

### AST: Project assets and changes made outside the editor (grade C)

Subsystem: metas, asset listing, file watching, open documents.

**Problem.** Writes are atomic and the watcher is robust, but one duplicated asset (file and meta copied together) makes every document unopenable, the command line tool and the editor build the asset set with two different walkers, renaming the open scene outside the editor drops its live session, files moved or metas deleted outside the editor silently get new identities, meta-only changes emit no event, Finder and editor temporary files get metas, and Save overwrites a pending disk change without a decision.

**Target state.** One asset-set policy shared by every tool, where a problem file costs only itself; identities survive moves, copies and lost metas; the open document follows its file; Save never silently overwrites a change made on disk.

#### AST-1: One asset-set policy for the command line tool and the editor; duplicated assets get a fresh identifier

- Priority P1, kind fix, effort M, golden impact none.
- Findings: gap-2-1, rust-quality-6.
- Depends on: nothing.
- Locations and files: `crates/ludenea-app/src/meta.rs:131-173`, `crates/ludenea-cli/src/assets.rs:10-79,183-208`, `crates/ludenea-project/src/files.rs:20,72-93,229-249,559-573`.
- Acceptance: A single project_asset_files walker in ludenea_app::meta is used by the command line tool and Project::asset_texts (one symlink and broken-meta policy). read_assets returns the readable assets plus problems. Two files with one identifier are both left out and named, and a scene referencing the ambiguous identifier says which files share it. Project::files gives the copy a fresh meta of the same kind and warns.
- First failing test: crates/ludenea-cli/src/assets.rs a_duplicated_prefab_leaves_the_other_assets_loadable (replacing duplicate_asset_identifiers_are_refused), and crates/ludenea-project/tests/assets.rs a_copied_asset_gets_a_new_identifier.

#### AST-2: The open scene follows renames and removals on disk

- Priority P0, kind fix, effort M, golden impact none.
- Findings: gap-2-4, readiness-10.
- Depends on: nothing.
- Locations and files: `editor/packages/editor-builtins/src/assets/index.tsx:174-188`, `editor/packages/editor-builtins/src/scene-tree/index.tsx:28-29`, `editor/packages/editor-shell/src/stores/document.ts:292-297,454-460`, `editor/packages/editor-builtins/test/assets.test.tsx:251-262`.
- Acceptance: On files_changed the asset browser refreshes and, when the listing holds the open identifier at a new path, the document follows it. When a created file holds the document's text, it follows that file (reconnecting the orphan meta once AST-3 lands). When the file is really gone, the session is stored first and the document stays open as deleted on disk with Save again and Close. The Save message never advises an action that throws the session away.
- First failing test: editor-builtins assets.test.tsx an_open_scene_renamed_outside_with_its_meta_stays_open_at_its_new_path and a_dirty_scene_removed_on_disk_stays_open_with_a_banner.

#### AST-3: Identity kept across moves made without the meta

- Priority P2, kind put-in-place, effort L, golden impact none.
- Findings: gap-2-2.
- Depends on: AST-4.
- Locations and files: `crates/ludenea-project/src/files.rs:82-126,384-509`, `crates/ludenea-project/src/service.rs:594-597`, `crates/ludenea-project/src/protocol.rs`.
- Acceptance: When one batch removes a file whose meta remains and creates a meta-less file of the same kind and bytes, the orphan meta moves to the new path and the move is reported. Otherwise a structured orphan-meta event names the candidates, and AdoptMeta { meta, file } reconnects it from the editor.
- First failing test: files.rs a_file_moved_without_its_meta_keeps_its_identifier.

#### AST-4: Metas deleted by hand restored with their identifier and kind

- Priority P2, kind fix, effort M, golden impact none.
- Findings: gap-2-3.
- Depends on: nothing.
- Locations and files: `crates/ludenea-project/src/files.rs:82-126`, `crates/ludenea-app/src/meta.rs:102-121`.
- Acceptance: The service remembers the metas it has read (persisted in .ludenea/metas.ron); a known file that lost its meta gets it back when the identifier is free, with a warning; otherwise the warning names the old identifier and kind.
- First failing test: crates/ludenea-project/tests/metas.rs a_meta_deleted_by_hand_is_restored_with_its_identifier_and_kind.

#### AST-5: Meta-only changes emit watcher events

- Priority P2, kind fix, effort S, golden impact none.
- Findings: gap-2-6.
- Depends on: nothing.
- Locations and files: `crates/ludenea-project/src/files.rs:388-411,491-505,586`.
- Acceptance: Tracked.known holds identifier and kind; a change, appearance or disappearance of a meta's identifier reports Changed for its file.
- First failing test: files.rs a_meta_whose_identifier_changes_reports_its_file.

#### AST-6: Hidden and temporary files ignored by every listing

- Priority P2, kind fix, effort S, golden impact none.
- Findings: gap-2-7, untrusted-input-11.
- Depends on: AST-1.
- Locations and files: `crates/ludenea-project/src/files.rs:559-591`, `crates/ludenea-cli/src/assets.rs:57-77`, `crates/ludenea-app/src/meta.rs`.
- Acceptance: One shared predicate (names starting with a dot, ending with a tilde, .swp, .swx or .tmp, or of the form #...#) is applied by the shared walker, list_files and the watcher's relative_path, and documented in the README.
- First failing test: metas.rs hidden_and_temporary_files_get_no_meta.

#### AST-7: Saving over a change made on disk asks first

- Priority P2, kind fix, effort M, golden impact none.
- Findings: gap-2-5.
- Depends on: AST-2.
- Locations and files: `editor/packages/editor-builtins/src/scene-tree/index.tsx:26-35`, `editor/packages/editor-builtins/src/inspector/index.tsx:87-107`, `editor/packages/editor-builtins/src/assets/index.tsx:154-170`.
- Acceptance: With a pending banner, or disk text differing from the text last loaded or saved, Save asks Overwrite, Reload or Cancel; a successful Save clears the banner for its path; Reload re-reads the file when clicked.
- First failing test: assets.test.tsx saving_with_a_pending_disk_change_asks_before_overwriting.

#### AST-8: Asset browser errors cleared after a successful reload

- Priority P3, kind fix, effort S, golden impact none.
- Findings: gap-2-8.
- Depends on: ERR-1.
- Locations and files: `editor/packages/editor-builtins/src/assets/index.tsx:81,165-188`, `editor/packages/editor-shell/src/stores/document.ts:207-212,348-358`.
- Acceptance: The document store owns reload errors; the panel-level report is removed or cleared when a later reload succeeds.
- First failing test: assets.test.tsx a_failed_disk_reload_followed_by_a_good_one_leaves_no_error.

#### AST-9: Deleting an asset in use names the documents that reference it

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: scenes-8.
- Depends on: SCN-5.
- Locations and files: `crates/ludenea-project/src/files.rs:160-165`, `editor/packages/editor-builtins/src/assets/AssetsPanel.tsx:191-199`, `crates/ludenea-app/src/prefab.rs:346-353`, `crates/ludenea-app/src/scene.rs:56-59`.
- Acceptance: The delete confirmation lists the documents mentioning the identifier; a scene refused for a missing prefab offers Remove instance N from its error banner; MissingAsset says restore it or remove instance 5.
- First failing test: assets.test.tsx deleting_a_prefab_used_by_a_scene_names_that_scene_in_the_confirmation.

#### AST-10: Case-only renames work; write errors name the destination

- Priority P3, kind fix, effort S, golden impact none.
- Findings: readiness-14, readiness-16.
- Depends on: nothing.
- Locations and files: `crates/ludenea-project/src/files.rs:137-157,291-321`.
- Acceptance: rename accepts a target that is the source itself, going through a temporary name; place() errors name the destination with the cause and mention the staging file only as detail.
- First failing test: tests/project.rs renaming_to_another_letter_case_keeps_the_identifier and a_failed_write_names_the_destination.

### RUN: Simulation loop, inputs and game-code isolation (grade C)

Subsystem: ludenea-app simulation, inputs, parallel runtime; game panics across host, command line tool and worker.

**Problem.** Input validation, the pacer, the deterministic RNG and Parallel's ordering are solid, but an oversized thread count panics every tool, game code panics are caught call site by call site (an input mapping or a value codec kills the native game process, the command line tool reports a bare panic), live input is judged idle on raw thresholds (unmapped keys fork the future), edits on runtime-spawned entities can retarget another entity, the player count cannot be set, the tick contract is hand-written in nine loops, and the template never reads its input.

**Target state.** Pool creation fallible end to end; one panic boundary around game code with tick and system; the game's mapping defines idleness; runtime entities addressed stably; a Run type owns the logs and the tick contract; a logging API for game code.

#### RUN-1: Thread counts validated and pool creation fallible end to end

- Priority P0, kind fix, effort S, golden impact none.
- Findings: runtime-4, replays-2, protocol-1, untrusted-input-1, determinism-2, test-strategy-2, errors-5, rust-quality-3, dx-live-17.
- Depends on: nothing.
- Locations and files: `crates/ludenea-app/src/parallel.rs:14-30`, `crates/ludenea-app/src/simulation.rs:264-267`, `crates/ludenea-cli/src/commands.rs:84-90`, `crates/ludenea-control/src/host.rs:371-374`, `crates/ludenea-control/src/protocol.rs:180-182`, `crates/ludenea-control/src/stdio.rs:29-32`.
- Acceptance: Parallel::try_new returns ParallelError for 0, for counts above a documented MAX_THREADS (for example 256) and for operating system refusals; Simulation::set_threads returns Result and keeps the previous pool. The command line tool validates --threads with a clap range and prints cannot start N threads: ...; use 1 to 256, exiting 2. SetThreads replies InvalidCommand and the session stays usable. serve_stdio falls back to one thread with a console warning. The panicking Parallel::new is removed. (The other APIs rust-quality-3 lists are covered by RUN-5 and SCN-3.)
- First failing test: crates/ludenea-control/tests/host.rs set_threads_beyond_the_limit_is_refused_and_the_session_goes_on, and crates/ludenea-cli/tests/cli.rs too_many_threads_is_refused.

#### RUN-2: One panic boundary around game code

- Priority P0, kind deep-refactor, effort L, golden impact none.
- Findings: runtime-3, protocol-2, rust-quality-2, replays-10, errors-12, dx-live-11.
- Depends on: RUN-1.
- Locations and files: `crates/ludenea-app/src/simulation.rs:159-168,298-306`, `crates/ludenea-ecs/src/schedule.rs:218-237`, `crates/ludenea-app/src/replay.rs:39-55`, `crates/ludenea-control/src/host.rs:123-138,552-556,654-659,848-860`, `crates/ludenea-control/src/session.rs:393-405,1663-1680,2189-2195`, `crates/ludenea-control/src/compare.rs:75,201`, `crates/ludenea-control/src/divergence.rs:250`, `crates/ludenea-cli/src/commands.rs:71-103`, `crates/ludenea-control/src/worker.rs:44-56`.
- Acceptance: The Simulation methods that run game code (stepping, map_raw_input, debug_draw, pick, value codecs and asset defaults reached through the host) catch unwinding natively and return SimError::Panicked { tick, system, message, location }, the schedule recording the running system. Host::handle and Host::advance have one boundary that replies SystemPanicked and reloads the session to a consistent tick. play_replay and record_replay return ReplayError::Panicked, so the command line tool prints the game panicked at tick 3 in system boom: ... and writes dumps (exit 2). The per-site wrappers in session, compare and divergence are deleted. Prefab panics show the prefab path, not a hexadecimal id.
- First failing test: crates/ludenea-control/tests/running.rs a_panicking_input_mapping_fails_set_live_input_and_keeps_the_host, host.rs a_panicking_try_from_in_set_field_is_reported_and_the_hash_is_unchanged, and cli.rs game_panic_names_the_tick_and_system.

#### RUN-3: Live input idleness judged on the game's mapping

- Priority P1, kind fix, effort S, golden impact none.
- Findings: runtime-2.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/host.rs:552-555`, `crates/ludenea-app/src/raw_input.rs:8-38`, `crates/ludenea-control/src/session.rs:519-535`.
- Acceptance: SetLiveInput compares the mapped bytes with the mapping of idle players (cached per player count); RawInput::is_idle, PRESSED and DEAD_ZONE are deleted.
- First failing test: running.rs unmapped_key_replays_the_recording (holding KeyQ keeps one branch and the recorded head), and an app test small_axis_the_game_maps_takes_over.

#### RUN-4: Edits on runtime-spawned entities keep their target

- Priority P1, kind deep-refactor, effort L, golden impact none.
- Findings: runtime-5.
- Depends on: VER-2, VER-7.
- Locations and files: `crates/ludenea-app/src/edit.rs:16-22,323-340`, `crates/ludenea-control/src/session.rs:1739-1760`.
- Acceptance: Entities spawned after load get a spawn address kept outside the state (the spawning edit's tick and index, or the spawning system's position and command sequence). EntityRef gains Spawned(SpawnAddress) in edit log version 3; versions 1 and 2 still decode as Runtime. At minimum, an edit whose identifier now names an entity of another origin is dropped with EditDropped instead of silently retargeting.
- First failing test: crates/ludenea-control/tests/session.rs edit_on_a_runtime_entity_follows_it_when_a_scene_entity_is_added.

#### RUN-5: Player count declared by the game and carried by the protocol

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: runtime-6, protocol-16, versioning-11.
- Depends on: VER-3.
- Locations and files: `crates/ludenea-app/src/simulation.rs:76-104`, `crates/ludenea-app/src/input.rs:22-85`, `crates/ludenea-control/src/protocol.rs`, `editor/packages/editor-builtins/src/input/capture.ts`, `editor/apps/web/src/session-guard.ts:24-44`.
- Acceptance: App::players(NonZeroU16) appears in GameInfo; set_players rejects 0; SimError::PlayerCount { expected, got }; capture fills player 0 from the keyboard and player k from gamepad k; LoadScene and LoadSession take players when the logs are empty and Reply::Session reports them; the editor's withoutTicks input-log decoder is deleted.
- First failing test: input_log.rs player_count_mismatch_names_the_counts, protocol_v3.rs load_session_with_empty_logs_keeps_the_given_player_count, capture.test.ts capture_sends_one_input_per_player.

#### RUN-6: Runtime APIs have fallible forms

- Priority P2, kind fix, effort S, golden impact none.
- Findings: runtime-11, untrusted-input-9.
- Depends on: nothing.
- Locations and files: `crates/ludenea-app/src/app.rs:185-193`, `crates/ludenea-app/src/rng.rs:35-53`, `crates/ludenea-app/src/input.rs:21-26`, `examples/boxes/src/lib.rs:161-168`.
- Acceptance: tick_rate(0) becomes AppError::InvalidTickRate returned by build; Rng::pick and try_range_u32 exist; Inputs::get and iter exist; each panicking method documents its fallible alternative; boxes uses Rng::pick. (Schema recursion from untrusted-input-9 is ECS-1, set_players is RUN-5, override_value_ron is SCN-3.)
- First failing test: simulation.rs tick_rate_zero_fails_the_build and rng.rs pick_from_an_empty_slice_is_none.

#### RUN-7: A Run type owns the logs and the tick contract

- Priority P2, kind deep-refactor, effort L, golden impact none.
- Findings: runtime-8, rust-quality-7.
- Depends on: RUN-2, QA-2.
- Locations and files: `crates/ludenea-app/src/replay.rs:150-165,314-353`, `crates/ludenea-control/src/session.rs:523-556,1243-1273,1688-1736`, `crates/ludenea-control/src/compare.rs:126-148,210-236`, `crates/ludenea-control/src/divergence.rs:291-311,383-404`.
- Acceptance: A ludenea-app Run owns InputLog and EditLog with private fields (push_edit keeps tick order, record_input checks the player count, truncate_after) and one advance(sim, tick, observer, on_edit_error); session, compare, divergence, subscriptions, replay and snapshot_before call it with explicit edit-failure policies.
- First failing test: edit.rs push_edit_keeps_tick_order, plus a cross-check that record_loaded_replay and Run::advance give the same hashes on the spawner replay.

#### RUN-8: Playing exports incrementally; storing a long session stays fast

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: runtime-9, readiness-6.
- Depends on: VER-5.
- Locations and files: `crates/ludenea-control/src/host.rs:566-580`, `crates/ludenea-control/src/session.rs:1301-1323`, `editor/apps/web/src/session-guard.ts:47-138`, `editor/packages/editor-shell/src/stores/document.ts:154-160`.
- Acceptance: ExportSession { since } returns only the inputs, edits and hashes after since (or the periodic export omits hashes), and the guard appends them; the per-second export size is bounded; stored-session encoding is chunked so a session of 216,000 ticks stores in under 50 ms.
- First failing test: host.rs export_since_returns_only_the_ticks_after_it, and session-guard.test.ts playing_exports_incrementally.

#### RUN-9: The template game uses its input and verifies its replays

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: runtime-10, projects-9, dx-live-19.
- Depends on: nothing.
- Locations and files: `crates/ludenea-project/template/src/lib.rs:1-61`, `crates/ludenea-project/src/template.rs:6-48`, `editor/e2e/session.spec.ts`, `crates/ludenea-project/tests/project.rs:417-451`.
- Acceptance: The template has a steer system reading Inputs<GameInput> before physics::Step, a collision handler ordered after it, a tests/replays.rs running verify over assets/replays/*.replay, and a README. An end-to-end step holds ArrowRight, exports, asserts non-default inputs and verifies the session in the command line tool.
- First failing test: editor/e2e/session.spec.ts held_keys_are_recorded_and_replay_in_the_cli, plus the ignored template test asserting that an ArrowRight log changes the hash.

#### RUN-10: Rng::range_f32 stays below its upper bound

- Priority P3, kind fix, effort S, golden impact possible.
- Findings: runtime-12.
- Depends on: nothing.
- Locations and files: `crates/ludenea-app/src/rng.rs:55-98`.
- Acceptance: The top draw never returns high; the conversion from 24 random bits to the range is a separately tested function.
- First failing test: rng.rs range_f32_top_draw_stays_below_high for [0.5, 1.5) and [1000, 1001).

#### RUN-11: Input capture releases keys after Cmd shortcuts and shows when it is active

- Priority P3, kind fix, effort S, golden impact none.
- Findings: runtime-13.
- Depends on: nothing.
- Locations and files: `editor/packages/editor-builtins/src/input/capture.ts:41-58`, `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:288-298`, `README.md:84-96`.
- Acceptance: A Meta keyup releases the keys pressed while Meta was down; a Keys go to the game badge shows while capture is on; the README explains playing (focus, keys the editor keeps, one player for now).
- First failing test: capture.test.ts releasing_meta_releases_keys_whose_keyup_was_swallowed.

#### RUN-12: Compare and divergence simulations use the session's thread count

- Priority P3, kind fix, effort S, golden impact none.
- Findings: runtime-14.
- Depends on: RUN-1.
- Locations and files: `crates/ludenea-control/src/host.rs:285-299,371-374`, `crates/ludenea-app/src/app.rs:214`.
- Acceptance: The host keeps the requested thread count and applies it to every simulation it builds.
- First failing test: host.rs set_threads_applies_to_the_compare_simulation.

#### RUN-13: A logging API for game code that reaches the Console on every target

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: readiness-8.
- Depends on: ERR-1.
- Locations and files: `crates/ludenea-app/src/`, `crates/ludenea-control/src/host.rs:1258-1260`, `crates/ludenea-control/src/worker.rs`, `crates/ludenea-cli/src/commands.rs`.
- Acceptance: A game-facing log (a Runtime<Console> parameter or ludenea::app::log) outside the state and the hash; the host drains it into Event::Console natively and in the worker; the command line tool prints it to stderr; it is documented as the way to log; the browser editor notes once that std output is not captured.
- First failing test: host.rs: a system logging hello yields Event::Console, and the hash equals that of the same run without the call.

### REP: Replays, traces and the command line tool (grade C)

Subsystem: ludenea-app replay and trace formats, ludenea-cli run, verify and diff.

**Problem.** Replay decoding validates everything before playing and CI verifies goldens everywhere, but the command line desync report stops at the tick and the hash (the anti-goal 'a desync report that only says hash differs'), nothing produces the reference snapshot, a trace recorded without inputs can never be verified, a CRLF checkout fails verification as a different scene, replays cannot be recorded again after a deliberate change, and the editor shows a same-types divergence only as a red mark.

**Target state.** verify localizes a divergence to the first divergent tick, leaf, entity, component and field, and prints the next commands; traces identify scenes canonically; replays can be re-recorded and verified in bulk; the editor says whether an opened replay matches.

#### REP-1: Verify a scene trace recorded without an input log

- Priority P1, kind fix, effort S, golden impact none.
- Findings: replays-3.
- Depends on: nothing.
- Locations and files: `crates/ludenea-cli/src/commands.rs:181-189,272-274`, `crates/ludenea-app/src/trace.rs:89-97`.
- Acceptance: --inputs is optional when verifying a scene, as it is for run; only --golden is required; the scene and inputs hash still guard; the README shows the example.
- First failing test: crates/ludenea-cli/tests/cli.rs run_then_verify_passes_without_inputs (also with --edits and no --inputs).

#### REP-2: Trace format 2: canonical scene identity and per-identity mismatch messages

- Priority P1, kind fix, effort M, golden impact certain.
- Findings: determinism-7, delivery-15, readiness-9.
- Depends on: VER-2.
- Locations and files: `crates/ludenea-app/src/trace.rs:66-97`, `crates/ludenea-cli/src/commands.rs:278-286`, `golden/boxes.trace`.
- Acceptance: scene_hash always hashes the canonical resolved scene text plus the assets, so line endings no longer matter; TRACE_VERSION 2, with version 1 still read; golden/boxes.trace re-recorded; verify reports scene and inputs mismatches separately with both hashes, the path and the run --trace command that records it again. (The template's .gitattributes is PRJ-8.)
- First failing test: cli.rs verify_ignores_the_line_endings_of_the_scene and verify_names_the_mismatched_identity.

#### REP-3: Desync reports localize the divergence

- Priority P1, kind put-in-place, effort L, golden impact certain.
- Findings: replays-1, ambition-2, determinism-4, dx-live-6, replays-7.
- Depends on: REP-2, FND-4, ERR-5.
- Locations and files: `crates/ludenea-cli/src/commands.rs:95-138,182-224,289-327`, `crates/ludenea-app/src/replay.rs:18-36,57-68,173-224,377-393`, `crates/ludenea-app/src/trace.rs:13-21`, `README.md`.
- Acceptance: Traces and replays record per tick one digest per resource and component name (next format versions). verify prints the first divergent tick and leaves (for example ludenea::Body, ludenea::ContactCache). For replays, verify plays to the first recorded checkpoint after the divergence and prints the entity, component, field and both values, and recorded checkpoints are no longer swapped into the live state. run --snapshot-at <tick> produces the reference on the passing platform; verify prints the exact next commands; diff labels its sides with the file names; the README documents the CI desync workflow.
- First failing test: cli.rs verify_replay_names_the_diverging_entity (a drifting app from tick 1,000) and verify_names_the_first_divergent_leaf.

#### REP-4: Record a replay again with this build

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: replays-9.
- Depends on: ERR-5.
- Locations and files: `crates/ludenea-cli/src/commands.rs:181-244`, `crates/ludenea-app/src/replay.rs:42-52`, `examples/boxes/src/bin/record-replay.rs`.
- Acceptance: run x.replay --replay out.replay (or --in-place) re-records over the replay's ticks; messages after a deliberate change name this command; record-replay uses it.
- First failing test: cli.rs run_records_a_replay_again_with_this_build.

#### REP-5: Verify a folder of replays; document a project's CI

- Priority P3, kind put-in-place, effort M, golden impact none.
- Findings: replays-8.
- Depends on: REP-4.
- Locations and files: `crates/ludenea-cli/src/commands.rs:202-216`, `README.md:147-157`, `crates/ludenea-project/template/`.
- Acceptance: verify accepts several paths and directories with one line per replay and exit codes 0, 1 and 2; a README section Verify a project's replays in CI; the template ships the workflow.
- First failing test: cli.rs verify_accepts_a_folder_of_replays.

#### REP-6: The editor says whether an opened replay matches

- Priority P2, kind fix, effort S, golden impact none.
- Findings: replays-12, ux-live-16.
- Depends on: nothing.
- Locations and files: `editor/packages/editor-builtins/src/timeline/TimelinePanel.tsx:36,84-112`, `editor/packages/editor-shell/src/stores/timeline.ts:66`.
- Acceptance: Every replay document shows a status line: verified up to tick N of M, matches all M recorded hashes, or diverges at tick N (expected X, got Y) with Seek and Compare actions; play stops at the recorded end or says ticks past it are new.
- First failing test: assets.test.tsx replay_that_diverges_with_the_same_types_says_so and timeline.test.tsx a_matching_replay_says_so_at_its_end.

#### REP-7: Trace comparison checks both traces describe the same run

- Priority P2, kind fix, effort S, golden impact none.
- Findings: branches-9.
- Depends on: REP-2.
- Locations and files: `crates/ludenea-app/src/trace.rs:96-122`, `crates/ludenea-control/src/host.rs:411-422`, `editor/packages/editor-builtins/src/divergence/TraceViewer.tsx:49-51`.
- Acceptance: TraceComparison carries same_scene and same_inputs (None for replay traces); the trace viewer warns that a divergence is expected when they differ; trace decode errors stop saying golden.
- First failing test: trace.rs comparing_traces_of_different_scenes_says_so, plus a divergence.test.tsx case asserting the warning.

#### REP-8: Command line tool and replay hygiene

- Priority P3, kind fix, effort S, golden impact none.
- Findings: replays-14.
- Depends on: REP-3.
- Locations and files: `crates/ludenea-app/src/trace.rs:43-65`, `crates/ludenea-cli/src/trace.rs:1`, `crates/ludenea-cli/src/lib.rs:6-14`, `crates/ludenea-app/src/replay.rs:57-79`.
- Acceptance: Trace::from_replay is deleted or used; the ludenea-cli Trace re-export goes (one import path); ReplayOutcome::Diverged holds its snapshots; verify is split into replay and golden functions.
- First failing test: n/a (refactor): the command line and replay suites stay green.

### CTL: Control protocol, host and transports (grade C)

Subsystem: ludenea-control host and transports, control-client, game process supervision.

**Problem.** The host resists hostile commands and the wire format is pinned, but a native game that exits or hangs is never reported (requests hang, the queue jams, only a rebuild restarts it), one non-UTF-8 byte on the game's output cuts the control channel, one throwing listener drops other responses, a lost connection to ludenea serve is never shown or recovered, a second Hello inherits stale state, and Host::execute is a 571-line match with topic state in four places.

**Target state.** Every game exit or hang is reported and recovered from the last export; output is read as bytes; listeners are isolated; the project connection reconnects; Host is split by area with one owner for topic state.

#### CTL-1: A native game that exits is reported, its channel closed, and it restarts from its last export

- Priority P1, kind fix, effort L, golden impact none.
- Findings: protocol-3, desktop-2, projects-12, untrusted-input-8.
- Depends on: DSK-2.
- Locations and files: `crates/ludenea-project/src/service.rs:124-139,284-327,546-590`, `crates/ludenea-project/src/game.rs:36-43`, `editor/apps/desktop/src-tauri/src/main.rs:101-111,194-215`, `editor/packages/control-client/src/transports/tauri.ts:9-27`, `editor/apps/web/src/games.ts:85-125`, `editor/packages/editor-builtins/src/console/index.tsx:76`.
- Acceptance: When a game exits, the service clears the process slot (the endpoint answers NoGame, GameReady is no longer replayed, GameExited is). The desktop relay clears to_game and sends a close frame that tauriTransport turns into exactly one close. RestartGame restarts the last good binary when no rebuilt one waits. followNativeGames treats an exit like a worker crash: same crash budget, the guard's latest export, an actionable console line. The game starts with the project root as its working directory.
- First failing test: editor/apps/web/test/games.test.ts a_native_game_that_exits_is_restarted_with_its_latest_session, and service.rs an_exited_game_is_no_longer_announced_and_can_be_restarted.

#### CTL-2: Request deadlines and a not-responding watchdog

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: protocol-4, ts-quality-14, readiness-3.
- Depends on: ERR-1.
- Locations and files: `editor/packages/control-client/src/client.ts:96-187`, `editor/packages/control-client/src/project.ts`, `editor/apps/web/src/games.ts:8-79`, `editor/packages/control-client/src/worker/host-worker.ts:38-57`.
- Acceptance: request(command, { timeoutMs, signal }) on both clients rejects naming the command and the wait; the handshake has a default deadline. A rebuild races the old game's export against a deadline and falls back to the guard's latest export. With a request pending and no frame for N seconds, the editor logs The game is not responding (tick T, waiting on <command> for S s) and offers Restart the game, which terminates the worker or calls restart_game through the crash path.
- First failing test: games.test.ts a_rebuild_replaces_a_game_that_stopped_answering and a_game_that_stops_answering_is_reported_and_restarts_from_its_last_export (fake timers).

#### CTL-3: Game and cargo output read as bytes

- Priority P0, kind fix, effort S, golden impact none.
- Findings: protocol-5, desktop-7, untrusted-input-3, readiness-12.
- Depends on: nothing.
- Locations and files: `crates/ludenea-project/src/game.rs:50-65`, `crates/ludenea-project/src/build.rs:139`, `crates/ludenea-project/src/service.rs:563-584`.
- Acceptance: Both game streams and cargo's stderr are split on newline bytes, a trailing carriage return trimmed, and converted with String::from_utf8_lossy; only an I/O error ends a relay; a non-UTF-8 line is never taken for a protocol message.
- First failing test: game.rs a_line_that_is_not_utf8_is_output_and_protocol_lines_still_arrive (stdout and stderr cases).

#### CTL-4: Listener isolation in the clients and the shell

- Priority P1, kind fix, effort M, golden impact none.
- Findings: protocol-6, plugins-2.
- Depends on: ERR-1.
- Locations and files: `editor/packages/control-client/src/transports/transport.ts:18-20`, `editor/packages/control-client/src/transports/tauri.ts:9-11`, `editor/packages/control-client/src/client.ts:208-212,285-287`, `editor/packages/control-client/src/project.ts:81-83`, `editor/packages/editor-shell/src/editor.ts:188-207`, `editor/packages/editor-shell/src/registry.ts:61-64`, `editor/apps/web/src/app.ts`.
- Acceptance: One notify helper calls each listener in try/catch, reports a failure once to editor.console naming the owning plugin, and continues. It is used by Listeners.emit, ControlClient and ProjectClient dispatch, setControl, Collection.changed, entity watches and the zustand subscribe wrapper exposed to plugins. Responses settle before events; tauri.ts emits each message of a batch independently; app.ts deactivates each plugin in its own try/catch and always clears the loaded list.
- First failing test: client.test.ts a_throwing_listener_neither_stops_other_listeners_nor_loses_responses, and registry.test.ts a_throwing_game_listener_does_not_stop_the_switch_or_the_other_listeners.

#### CTL-5: A lost connection to ludenea serve is shown and recovered

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: protocol-7, projects-5, shell-4, readiness-7.
- Depends on: ERR-1.
- Locations and files: `editor/packages/control-client/src/project.ts:38-84`, `editor/packages/control-client/src/transports/websocket.ts:3-25`, `editor/apps/web/src/main.tsx:6-18`, `editor/apps/web/src/app.ts:29-85`, `editor/apps/web/src/session-guard.ts:76-86`.
- Acceptance: ProjectClient.onClose exists. A persistent role=alert banner reads Lost the connection to ludenea serve (reason). Reconnecting. The application reconnects with backoff, says hello again, reloads info, listing and plugins, stores the guard's latest export, then clears the banner. Saves and stores are refused with a precise error (or queued) while disconnected.
- First failing test: editor/apps/web/test/app.test.ts a_dropped_project_connection_reconnects_and_stores_the_latest_session.

#### CTL-6: A second Hello starts a fresh connection

- Priority P3, kind fix, effort S, golden impact none.
- Findings: protocol-13, desktop-9.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/in_process.rs:51-89`, `crates/ludenea-control/src/host.rs:45-60,869-912`, `editor/apps/desktop/src-tauri/src/main.rs:68-75`.
- Acceptance: Host::reset_connection runs on every Hello and clears subscriptions, ghost and divergence state, introspection and live input, keeping the session and simulation; the Hello documentation says so; an editor.reload command is named in the lag warning.
- First failing test: stdio.rs a_second_hello_starts_the_connection_afresh.

#### CTL-7: Only real protocol frames leave a game's output; the client warns on unknown frames

- Priority P3, kind fix, effort S, golden impact none.
- Findings: protocol-11, projects-20.
- Depends on: CTL-3.
- Locations and files: `crates/ludenea-project/src/game.rs:121-139`, `editor/packages/control-client/src/client.ts:131-137,208-212`.
- Acceptance: split_stdout_line takes a line as protocol only for the welcome, refused, response and event tags; ControlClient warns on unknown frame types and turns a refusal after the handshake into a visible error.
- First failing test: game.rs a_printed_line_shaped_like_json_stays_the_games_output, and client.test.ts a_refusal_after_the_handshake_is_reported.

#### CTL-8: Host::execute split by area; topic state owned by Subscriptions

- Priority P2, kind deep-refactor, effort L, golden impact none.
- Findings: protocol-14, rust-quality-5.
- Depends on: RUN-2, SCN-1.
- Locations and files: `crates/ludenea-control/src/host.rs:35-61,343-1127,1306-1345`, `crates/ludenea-control/src/subscriptions.rs`.
- Acceptance: execute dispatches to handlers in host/{queries, editing, prefabs, branches, replays, topics}.rs; one edited_value helper replaces the three read-modify-write copies with one read-only wording; Subscriptions owns every topic's state behind one subscribe and unsubscribe entry point; Command::is_query is exhaustive without a wildcard.
- First failing test: tests/editing.rs: SetField, ApplyOverride and SetAssetField on a value without an exact JSON form return the same InvalidEdit wording.

#### CTL-9: The session export is one wire struct

- Priority P3, kind fix, effort S, golden impact none.
- Findings: rust-quality-13.
- Depends on: VER-3.
- Locations and files: `crates/ludenea-control/src/session.rs:141-162`, `crates/ludenea-control/src/protocol.rs:283-305,496-512`, `crates/ludenea-control/src/host.rs:566-608`.
- Acceptance: A SessionData struct is embedded in Reply::Session and Command::LoadSession with From conversions; SessionExport drops its unused serde derives.
- First failing test: running.rs export_session_replays_in_fresh_host extended to re-send the reply through one conversion.

#### CTL-10: A documented control surface for third-party tools

- Priority P3, kind put-in-place, effort M, golden impact none.
- Findings: protocol-15.
- Depends on: VER-3.
- Locations and files: `README.md:33,96-200`, `editor/packages/control-client/src/transports/`, `crates/ludenea-cli/src/lib.rs:31-33`.
- Acceptance: A README section Driving a game from your own tool (host --stdio, the handshake, line framing, an example, where the types live); control-client exports a Node stdioTransport; the workspace table lists host.
- First failing test: control-client test/stdio.test.ts stdio_transport_drives_a_real_game_host (skipped when the binary is not built).

#### CTL-11: Transport hygiene

- Priority P3, kind fix, effort S, golden impact none.
- Findings: protocol-17.
- Depends on: CTL-4.
- Locations and files: `crates/ludenea-control/src/in_process.rs:7-17,115-119`, `editor/packages/control-client/src/transports/post-message.ts`, `editor/packages/control-client/src/worker/supervisor.ts:31-53`.
- Acceptance: InProcess::next_id is removed; the single-implementation Transport trait is removed or justified; startWorkerGame is built on postMessageTransport or the latter is deleted; both clients share one event source; allows become expects with reasons; the Document wire casing is aligned at the next protocol bump.
- First failing test: n/a (hygiene): the control-client and control suites stay green.

### SES: Editing sessions, undo and redo, timeline (grade C)

Subsystem: ludenea-control session, undo history, checkpoints, timeline.

**Problem.** Session invariants are documented, coverage is high and panics in game code are isolated, but a malformed undo history panics the host or exhausts memory, branch identifiers can be reused into a cycle, redo can rebuild a branch from a stale parent (false recorded hashes) or silently discard play recorded since the undo, restore and edit logs are unbounded, the undo history stores two full scene texts per tick-0 edit, scrubbing far behind the head replays thousands of ticks per step, the host rebuilds the timeline several times per frame, every tick-0 edit is blamed on the game code, and session.rs is a 2,249-line god object.

**Target state.** The undo history and branch tree validate on resume and keep their invariants; redo never loses or falsifies recorded work; work per command is bounded; storage and seek cost follow the change, not the run; Session is split by concern under a seeded sequence test.

#### SES-1: Undo history and branch restore keep the tree's invariants

- Priority P0, kind fix, effort M, golden impact none.
- Findings: sessions-1, sessions-3, branches-5.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/branches.rs:88-95,201-235,291-305,446-474`, `crates/ludenea-control/src/session.rs:289-334,648-661,1310-1314`, `crates/ludenea-control/src/undo.rs:211-234`.
- Acceptance: put_back raises next above every restored identifier and runs the decode fork checks. resume drops, with a load warning, entries naming an unknown branch or the root as a fork, and removed branches whose fork fails check_forks. The expect at session.rs:654 becomes SessionError::NoSuchBranch, handled by is_dropped. An undecodable history resumes empty with a warning. fork() debug-asserts that the identifier is new.
- First failing test: tests/undo.rs a_fork_entry_naming_the_root_is_dropped_not_a_panic, a_redo_entry_forking_past_its_parent_is_refused, and a_redone_fork_after_a_resume_keeps_branch_identifiers_unique.

#### SES-2: Redo rebuilds only the branch's own segment

- Priority P0, kind fix, effort S, golden impact none.
- Findings: sessions-2.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/session.rs:720-739,1556-1587,2089-2095`, `crates/ludenea-control/src/branches.rs:342-349`.
- Acceptance: Redo of a Run entry splices the branch's own segment onto the current run (or resimulates from the first stale tick), so recorded hashes stay true and a resume reports no divergence.
- First failing test: undo.rs redo_after_a_scene_edit_on_a_fork_keeps_recorded_hashes_true (a fresh replay of export() matches every hash).

#### SES-3: Restores, edit logs and steps bounded

- Priority P1, kind fix, effort M, golden impact none.
- Findings: sessions-4, untrusted-input-4.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/session.rs:289-330,512-518,1157-1198,1228-1300,1644-1658`, `crates/ludenea-control/src/host.rs:392-398`.
- Acceptance: Restore of a snapshot past simulated().max(head) is InvalidTick naming both ticks; LoadEdits and resume refuse edits past the recorded inputs and hashes with InvalidEdits naming the tick; Step above a documented maximum is refused or runs through the resimulation budget with progress events.
- First failing test: session.rs restoring_a_snapshot_past_the_recorded_end_is_invalid_tick and an_edit_log_past_the_recorded_run_is_refused.

#### SES-4: Redo keeps play recorded since the undo

- Priority P1, kind fix, effort M, golden impact none.
- Findings: sessions-5.
- Depends on: SES-1.
- Locations and files: `crates/ludenea-control/src/session.rs:535-548,610-626,720-805`.
- Acceptance: Redo of a Run entry on a branch with recorded ticks past the entry first forks them into a new branch, named in the redo label (Redo edit at tick 100 (keeps Branch 3)).
- First failing test: undo.rs redo_after_new_play_keeps_that_play_as_a_branch.

#### SES-5: One reachable end on every timeline view

- Priority P1, kind fix, effort S, golden impact none.
- Findings: sessions-6, ux-live-12, branches-17.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/session.rs:196-199,966-972,1326-1365,1655-1658`, `editor/packages/editor-builtins/src/timeline/TimelinePanel.tsx:29-122`, `editor/packages/editor-builtins/src/timeline/BranchLanes.tsx:26-43`, `crates/ludenea-control/src/compare.rs:164-168`.
- Acceptance: Timeline reports end = max(head, simulated); the label, slider maximum, marks, progress bar, ResimulatingNotice and branch lanes use it; the lanes share the scrubber's scale and draw the current tick and the first divergent tick; a game without an input type can scrub; GhostDraw reports when the compare branch ends.
- First failing test: timeline.test.tsx scrubber_covers_simulated_ticks_past_the_head (Tick 275 of 275), branches.test.tsx lanes_and_scrubber_share_one_scale, and a Rust test that an input-less session reports end 100.

#### SES-6: The undo history stores each scene version once and is bounded by bytes

- Priority P1, kind fix, effort L, golden impact none.
- Findings: sessions-10.
- Depends on: VER-7.
- Locations and files: `crates/ludenea-control/src/undo.rs:26-30,101-104`, `crates/ludenea-control/src/session.rs:1933-1985,1301-1322`.
- Acceptance: Scene versions are pooled (an entry's after is the next one's before) or stored as reversible patches; a byte cap drops the oldest entries with a console notice; the undo format version is bumped with a migration.
- First failing test: undo.rs undo_history_of_tick_zero_edits_grows_with_the_change_not_the_scene.

#### SES-7: Numbers beyond the f32 range refused precisely

- Priority P2, kind fix, effort S, golden impact none.
- Findings: sessions-11.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/json.rs:29-43`, `crates/ludenea-control/src/host.rs:1410-1424`, `editor/packages/editor-builtins/src/inspector/fields/inputs.tsx:72-78`.
- Acceptance: from_wire or encode_value refuses finite JSON numbers above f32::MAX at F32 positions (1e39 does not fit in an f32, at most 3.4e38); NumberField mirrors the check for f32 and integer kinds; a value shown only as RON says why.
- First failing test: editing.rs set_field_with_a_number_beyond_f32_is_invalid_edit.

#### SES-8: Seeking reuses the current state and caches near recent seeks

- Priority P1, kind fix, effort M, golden impact none.
- Findings: sessions-9, gap-5-8.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/session.rs:962-983,1683-1712`, `crates/ludenea-control/src/checkpoints.rs:9-59`.
- Acceptance: replay_before_edits continues from the world when current is at or before the target and past the nearest checkpoint; the checkpoint budget is spent (per-level counts derived from CHECKPOINT_BUDGET) and evictable checkpoints every CHECKPOINT_INTERVAL near a seek are kept in a small most-recently-used set evicted first.
- First failing test: session.rs with a restore counter: after a seek to 9,000, a seek to 9,015 restores nothing and the hash matches; seeking_near_a_recent_seek_far_behind_the_head_replays_at_most_an_interval.

#### SES-9: Session split by concern

- Priority P2, kind deep-refactor, effort L, golden impact none.
- Findings: sessions-18, branches-14, rust-quality-4.
- Depends on: QA-2, RUN-7.
- Locations and files: `crates/ludenea-control/src/session.rs:164-2128`, `crates/ludenea-control/src/branches.rs:47-117`.
- Acceptance: A Run type for the run's data (head, end, simulated, discard_after, splice) used by Session, BranchRun and Segment; a ForkPoint type with own_start; impl blocks under session/{lifecycle, scene_edits, history, branching, resimulation}.rs; the branch switching state machine in its own struct; introspection moved to Host. The public API is unchanged and the control suites stay green.
- First failing test: run.rs unit tests pinning head and end for an empty run, inputs longer than edits, edits past the inputs and hashes past both, written first against today's formulas.

#### SES-10: The host stops rebuilding the Timeline per frame; per-tick progress travels in Ticked

- Priority P2, kind fix, effort M, golden impact none.
- Findings: sessions-19, readiness-4.
- Depends on: VER-3.
- Locations and files: `crates/ludenea-control/src/host.rs:143,193,1018,1197,1264-1274`, `crates/ludenea-control/src/session.rs:543-548,1326-1365`, `editor/packages/editor-builtins/src/timeline/TimelinePanel.tsx`.
- Acceptance: Session::current and is_resimulating are used where only those are read; per-tick head and simulated travel in Ticked (or a TimelineProgressed event) and TimelineChanged is emitted only on structural change; the timeline panel draws marks without one element per edit.
- First failing test: host.rs: with 200 edits recorded, 60 played ticks emit no TimelineChanged and Ticked carries the new head.

#### SES-11: Resume on the same build skips resimulation; resimulation bounded by time

- Priority P2, kind deep-refactor, effort L, golden impact none.
- Findings: readiness-2.
- Depends on: SES-9, VER-7.
- Locations and files: `crates/ludenea-control/src/session.rs:143-170,289-370`, `crates/ludenea-control/src/host.rs:31,1184-1228`, `crates/ludenea-control/src/stdio.rs:45-79`.
- Acceptance: Sparse checkpoints are stored with the session (or inactive sessions kept in a host cache) and restored when the schema fingerprint matches, so a resume on the same build can seek at once; resimulation slices are bounded by a deadline the transport passes.
- First failing test: session.rs: play 5,000 ticks, export, resume on the same build, and a seek to 4,990 succeeds at once; a host test keeps one advance during resimulation under its deadline.

#### SES-12: Checkpoint budget in bytes

- Priority P2, kind fix, effort M, golden impact none.
- Findings: readiness-5, branches-13.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/checkpoints.rs:9-15`, `crates/ludenea-control/src/session.rs:1614-1637`, `crates/ludenea-ecs/src/snapshot.rs:14-31`.
- Acceptance: Each checkpoint's size is recorded; eviction keeps the total under a configurable byte budget (512 MB native, 256 MB in the worker) with today's eviction order; the figure is reported in Timeline.
- First failing test: Session test eviction_respects_a_byte_budget (20,000 Pose entities, 30 branches of 600 ticks; seeks still match the recorded hashes).

#### SES-13: Undo labels say what they undo

- Priority P2, kind fix, effort S, golden impact none.
- Findings: gap-1-12.
- Depends on: VER-7.
- Locations and files: `crates/ludenea-control/src/undo.rs:26-79`, `crates/ludenea-control/src/session.rs:864-890`.
- Acceptance: Scene and Asset undo entries carry a label (Spawn entity #6, Set ludenea::Body.gravity_scale on #4, 3 edits) that defaults when absent; older exports keep loading; the change is in the changelog.
- First failing test: undo.rs the_undo_label_names_a_tick_zero_spawn.

#### SES-14: Divergence messages name their cause

- Priority P1, kind fix, effort S, golden impact none.
- Findings: gap-1-11, ux-live-8, gap-2-9.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/host.rs:1201-1215`, `crates/ludenea-control/src/session.rs:119-138,1029-1044,2089-2109`.
- Acceptance: Resimulation::Diverged carries its cause (tick-0 edit, asset edit, scene or assets reloaded from disk, rebuild or loaded session); the user's own edits log at info level naming the edit; only rebuilds and loaded sessions mention the game code.
- First failing test: editing.rs a_divergence_after_a_tick_zero_edit_names_the_edit_not_the_game_code, and host.rs a_scene_replaced_from_disk_does_not_blame_the_game_code.

### BRN: Branches, ghost and divergence tools (grade C)

Subsystem: branch tree, compare simulation (ghost), causal divergence report, trace and snapshot viewer.

**Problem.** Branch storage, Arc-shared checkpoints, panic isolation and the incremental report are well designed, but the report finds causes for almost no divergent entity in the reference scenario, never reports resources or assets, is a flat list resent in full, describes a field change as an insert, the ghost reparses the whole scene on every restore, work is bounded in ticks not time, a restore before a fork is lost on switch, branch names are unchecked, and the root branch is named by matching a magic string.

**Target state.** The report explains propagation generically through references in values, covers resources and assets, streams incrementally as a tree, and names fields; ghost and report work is bounded in time without reparsing; restores fork like edits.

#### BRN-1: Causes found through entity references in values

- Priority P2, kind put-in-place, effort L, golden impact none.
- Findings: branches-1.
- Depends on: ECS-6, ECS-15.
- Locations and files: `crates/ludenea-control/src/divergence.rs:89-111,524-581`, `crates/ludenea-physics/src/contacts.rs:11-15`.
- Acceptance: At tick-1 and tick on either branch, resource and component values referencing an already divergent entity (walked through their schemas) yield Cause::Reference { tick, owner, path, other }; ContactCache pairs explain contact propagation with no physics special case; the work is capped to changed values.
- First failing test: tests/divergence.rs reference_scene_divergences_have_causes (at least 90 percent of the entities diverging after the fork have a cause chain leading to the push).

#### BRN-2: Resource and asset divergence reported

- Priority P2, kind fix, effort M, golden impact none.
- Findings: branches-2.
- Depends on: VER-3.
- Locations and files: `crates/ludenea-control/src/divergence.rs:28-42,415-432,493-506`, `editor/packages/editor-builtins/src/divergence/CausalReport.tsx`.
- Acceptance: DivergenceReport.resources (name, first tick, first difference) and an assets equivalent are filled from the hash tree leaves; they show under the root cause and count in root_cause; bindings regenerated.
- First failing test: divergence.rs resource_divergence_names_the_resource_and_field.

#### BRN-3: The ghost and the report load the scene once per scene version

- Priority P2, kind fix, effort M, golden impact none.
- Findings: branches-3, gap-5-4.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/compare.rs:111-127,209-222`, `crates/ludenea-control/src/divergence.rs:383-389`, `crates/ludenea-control/src/session.rs:434-438`.
- Acceptance: Session keeps a scene generation counter bumped by scene and asset changes; CompareRun and DivergenceRun reload only when it changes and otherwise restore snapshots directly.
- First failing test: compare.rs scrubbing_back_restores_without_reloading_the_scene (4 restores, 1 scene load).

#### BRN-4: Ghost and report work bounded by time; the report start resumable

- Priority P2, kind fix, effort M, golden impact none.
- Findings: branches-4.
- Depends on: BRN-3.
- Locations and files: `crates/ludenea-control/src/compare.rs:16-19`, `crates/ludenea-control/src/host.rs:203-237`, `crates/ludenea-control/src/divergence.rs:281-313`.
- Acceptance: Compare and divergence work per advance stops at a deadline derived from the elapsed time; Progress::start resumes from a cursor over several advances and reports progress; the tick budgets remain upper bounds.
- First failing test: divergence.rs report_start_spreads_over_advances.

#### BRN-5: A restore before a fork forks a child instead of being lost

- Priority P2, kind fix, effort M, golden impact none.
- Findings: branches-7.
- Depends on: SES-1.
- Locations and files: `crates/ludenea-control/src/session.rs:1157-1198`, `crates/ludenea-control/src/branches.rs:57-74`.
- Acceptance: A restore changing the state at a shared or recorded tick forks a child there holding the restored checkpoint and hash, undoable as a fork; the parent's prefix stays intact.
- First failing test: tests/branches.rs restore_before_the_fork_survives_switching_away_and_back.

#### BRN-6: Branch names validated

- Priority P3, kind fix, effort S, golden impact none.
- Findings: branches-8, ux-live-14.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/session.rs:1508-1515`, `editor/packages/editor-builtins/src/timeline/BranchLanes.tsx:79-103`, `editor/packages/editor-builtins/src/timeline/index.tsx:128`.
- Acceptance: rename_branch trims and refuses empty, duplicate and over-64-character names with SessionError::BranchName naming the problem; the rename form shows the error, autofocuses and selects its text.
- First failing test: branches.rs rename_refuses_empty_duplicate_and_oversized_names.

#### BRN-7: Divergence files opened from anywhere; snapshot ticks checked

- Priority P3, kind put-in-place, effort M, golden impact none.
- Findings: branches-10.
- Depends on: DSK-7.
- Locations and files: `editor/packages/editor-builtins/src/divergence/index.tsx:116-158`, `editor/packages/editor-builtins/src/divergence/SnapshotDiff.tsx:39-60`.
- Acceptance: Open a trace or snapshot file uses a file input in the browser and the dialog on desktop, passing bytes straight to compare_traces or diff_snapshots without writing into the project; comparing a snapshot with the session seeks to its tick or warns naming both ticks.
- First failing test: divergence.test.tsx a_snapshot_of_another_tick_is_compared_at_its_tick.

#### BRN-8: The causal report is incremental and nested

- Priority P2, kind put-in-place, effort L, golden impact none.
- Findings: branches-12.
- Depends on: BRN-1.
- Locations and files: `crates/ludenea-control/src/divergence.rs:214-240`, `editor/packages/editor-builtins/src/divergence/CausalReport.tsx:51-88`, `editor/packages/editor-builtins/src/divergence/index.tsx:164-178`.
- Acceptance: GetDivergence { since } or progress events carry only new entries; entities nest under the entity their first cause points to; entities without a cause collapse per tick; the report filters by kind or component; long lists are virtualized.
- First failing test: divergence.rs get_divergence_since_returns_only_new_entries, and divergence.test.tsx an_entity_caused_by_another_is_nested_under_it.

#### BRN-9: Root causes name the field; values labelled by branch

- Priority P2, kind fix, effort M, golden impact none.
- Findings: branches-15, ux-live-13.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/divergence.rs:56-61,617-641`, `editor/packages/editor-builtins/src/divergence/CausalReport.tsx:43-75`.
- Acceptance: An Insert of a component the entity already had reads change <component> <field>; one entity notation (4v0, scene entity at 4); TakeOver names the player and the first differing input field; values are labelled by branch (main: 1.5, Branch 2: 3.0); Extend sits on its own line.
- First failing test: divergence.rs a_set_field_root_cause_names_the_changed_field, and a divergence.test.tsx case for labelled values.

#### BRN-10: The host names the root branch after the document

- Priority P3, kind fix, effort S, golden impact none.
- Findings: branches-16.
- Depends on: VER-3.
- Locations and files: `crates/ludenea-control/src/branches.rs:22-28,144-153,422-426`, `editor/packages/editor-shell/src/stores/document.ts:163-164,253-264`.
- Acceptance: LoadScene, LoadSession and LoadReplay carry the document's name and the root is named there; ROOT_NAME and nameRoot are removed from document.ts.
- First failing test: Host test load_scene_names_the_root_after_the_document, plus a vitest case asserting the document store sends no rename_branch.

#### BRN-11: Undo on a stale branch completes after resimulation; deleting a branch keeps unrelated redo

- Priority P3, kind fix, effort M, golden impact none.
- Findings: branches-20.
- Depends on: SES-1.
- Locations and files: `crates/ludenea-control/src/session.rs:589-596,665-673`, `crates/ludenea-control/src/undo.rs:128-163`.
- Acceptance: A pending undo completes when resimulation reaches its tick and replies with progress instead of an error; forget_branches drops only the redo entries touching the deleted branches.
- First failing test: branches.rs undo_on_a_stale_branch_completes_after_resimulation and deleting_an_unrelated_branch_keeps_redo.

### PRJ: Projects, builds, serving and the template (grade C)

Subsystem: ludenea-project, ludenea-serve, the project template.

**Problem.** Path confinement, atomic writes, the file watcher, build diagnostics and serve's host and origin checks are strong, but the Vite dev proxy forges Origin and voids serve's cross-site protection, opening another project loads its start document into the previous project's game, a project message above 16 MiB silently drops the connection, a closed project's build keeps broadcasting, ludenea.toml is unversioned and unlocated, serve refusals are unexplained, new projects get none of the engine's determinism enforcement, and the game's TypeScript types exist only after an editor connects to a running game.

**Target state.** The development setup keeps the security model; project switches end the previous game and build; message limits are deliberate and explained; the manifest is strict; new projects inherit the engine's lints, line-ending rules, overflow checks and lockfile; types exist from project creation.

#### PRJ-1: The Vite dev proxy stops forging Origin

- Priority P1, kind fix, effort S, golden impact none.
- Findings: projects-1.
- Depends on: nothing.
- Locations and files: `editor/apps/web/vite.config.ts:5-23`, `editor/apps/web/dev-proxy.ts (new)`, `README.md:116-119`.
- Acceptance: The /ws/project proxy has no Origin rewrite and no changeOrigin; a bypass, in a small tested dev-proxy.ts module, refuses upgrades whose Origin is not the Vite origin; the README launches serve with --allow-origin http://localhost:5173.
- First failing test: editor/apps/web vitest allowProjectSocket_refuses_a_foreign_origin (and accepts the Vite origin; the exported configuration uses it as bypass).

#### PRJ-2: Opening another project ends the previous game

- Priority P1, kind fix, effort M, golden impact none.
- Findings: projects-2.
- Depends on: nothing.
- Locations and files: `editor/apps/web/src/games.ts:33-126`, `editor/apps/web/src/app.ts:63-70`, `editor/packages/editor-builtins/src/project/index.tsx:64-73`, `crates/ludenea-project/src/service.rs`.
- Acceptance: On project_opened with another root, followWasmGames and followNativeGames dispose the guard, close the game, call connectGame(null) and reset crash state; the service announces the stop; the start document loads into the new project's game on GameReady.
- First failing test: games.test.ts opening_another_project_disconnects_the_previous_game.

#### PRJ-3: Project WebSocket limits deliberate; oversized messages refused precisely

- Priority P0, kind fix, effort M, golden impact none.
- Findings: projects-4.
- Depends on: nothing.
- Locations and files: `crates/ludenea-serve/src/serve.rs:94-104,188-193`, `editor/packages/editor-shell/src/stores/document.ts:155-159`.
- Acceptance: max_frame_size and max_message_size are set deliberately for the 100,000-entity target and documented; a refused message gets a precise error naming its size and the limit before the close, plus a server log line; the editor shows it (CTL-5 then reconnects).
- First failing test: crates/ludenea-serve/tests/serve.rs a_large_write_is_accepted_and_an_oversized_one_is_refused_precisely.

#### PRJ-4: Builds cancellable and scoped to their project

- Priority P2, kind fix, effort M, golden impact none.
- Findings: projects-3.
- Depends on: nothing.
- Locations and files: `crates/ludenea-project/src/service.rs:71-78,387-418`, `crates/ludenea-project/src/build.rs:92-104`.
- Acceptance: Project::build returns a handle owning the cargo child process (killed on cancel or drop); closing a project kills its build; the relay drops events once the project is closed.
- First failing test: service.rs closing_a_project_stops_its_build_and_its_events.

#### PRJ-5: ludenea.toml versioned, strict, located and reloaded

- Priority P2, kind fix, effort S, golden impact none.
- Findings: projects-6, errors-9.
- Depends on: VER-4.
- Locations and files: `crates/ludenea-project/src/manifest.rs:4-11`, `crates/ludenea-project/src/lib.rs:45-46,88-103`, `crates/ludenea-project/src/service.rs:308-315`.
- Acceptance: version = 1 (the default when absent, higher refused); deny_unknown_fields; errors read ludenea.toml:line:column: message; a missing manifest is NotAProject suggesting ludenea new; engine is compared with the running engine, warning with both versions; the manifest reloads on change and ProjectOpened is announced again.
- First failing test: tests/project.rs open_rejects_unknown_manifest_keys_with_their_name and a_manifest_type_error_names_its_line_and_column.

#### PRJ-6: ludenea serve refusals and misconfigurations explained

- Priority P2, kind fix, effort S, golden impact none.
- Findings: projects-7, dx-live-22.
- Depends on: PRJ-5.
- Locations and files: `crates/ludenea-serve/src/serve.rs:65-116,265-296`, `crates/ludenea-serve/src/main.rs:33-35`.
- Acceptance: 421 and 403 responses carry a body naming the flag that allows them, logged once per kind; the remote warning mentions --allow-host; startup refuses an --editor-dir without index.html (run pnpm -C editor build) and a non-embedded build without --editor-dir; embedded mode answers a plain 404 for missing files.
- First failing test: serve.rs a_refused_host_is_told_which_flag_allows_it and serve_refuses_an_editor_dir_without_index_html.

#### PRJ-7: The embedded editor tracked by cargo; static serving tested

- Priority P2, kind put-in-place, effort S, golden impact none.
- Findings: projects-8.
- Depends on: nothing.
- Locations and files: `crates/ludenea-serve/src/serve.rs:19-21,266-289`, `crates/ludenea-serve/Cargo.toml:10-13`, `crates/ludenea-serve/build.rs (new)`, `crates/ludenea-serve/tests/serve.rs`.
- Acceptance: A build.rs emits rerun-if-changed for editor/apps/web/dist and fails with an actionable message when index.html is missing; tests cover the index, content types, traversal and missing files; one embedded-mode test runs in CI.
- First failing test: serve.rs editor_files_are_served_from_the_editor_dir_and_cannot_escape_it.

#### PRJ-8: New projects inherit the engine's enforcement and are portable

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: determinism-6, delivery-6, versioning-7, dx-live-20, projects-10, delivery-4.
- Depends on: FND-3.
- Locations and files: `crates/ludenea-project/src/template.rs:6-80`, `crates/ludenea-project/template/Cargo.toml`, `crates/ludenea-project/template/rust-toolchain.toml`, `crates/ludenea-project/template/gitignore`, `crates/ludenea-project/src/build.rs:120-130`, `crates/ludenea-project/src/lib.rs:115-117`.
- Acceptance: ludenea new writes clippy.toml (embedded from the engine's), .gitattributes (*.ron and *.meta with LF line endings; binary formats marked binary), [profile.release] overflow-checks = true, toolchain components clippy and wasm32-wasip1, a Cargo.lock seeded from the engine's, and a relative engine path when project and engine share an ancestor. Builds pass --locked once a lock exists. A test compares the template's overflow-checks with the engine's. Publishing the engine stays out of scope.
- First failing test: tests/project.rs a_new_project_carries_the_engine_determinism_lints and create_writes_a_relative_engine_path_and_seeds_the_lockfile.

#### PRJ-9: Game types and plugin declarations written without a running game

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: projects-22, plugins-11, ambition-6, dx-live-13.
- Depends on: ECS-14.
- Locations and files: `crates/ludenea-project/src/types.rs:8-62`, `crates/ludenea-project/src/service.rs:280-283`, `crates/ludenea-project/src/protocol.rs:142-146`, `editor/apps/web/src/app.ts:78-83`, `crates/ludenea-cli/src/lib.rs`, `crates/ludenea-project/template/editor/`.
- Acceptance: The shared declarations and an empty game.d.ts are written when a project opens and by ludenea new; a types subcommand of the game command line tool prints GameInfo and the service writes .ludenea/types after each native build; game.d.ts gains ResourceTypes, EventTypes and Input; engine component declarations ship as a base; the template has a typecheck script with a pinned typescript.
- First failing test: crates/ludenea-project/tests/service.rs opening_a_project_writes_the_shared_plugin_declarations_before_any_build, and cli.rs types_prints_the_game_info.

#### PRJ-10: The project protocol handshake is tested

- Priority P2, kind fix, effort S, golden impact none.
- Findings: test-strategy-8.
- Depends on: ERR-6.
- Locations and files: `crates/ludenea-project/src/service.rs:153-189`, `crates/ludenea-project/src/protocol.rs:32-49,305-320`.
- Acceptance: Unit tests on ProjectService::answer cover a mismatching version (Refused naming both versions), a request before hello, malformed JSON with and without an id, and a Response round trip through the wire serializer.
- First failing test: service.rs a_hello_of_another_version_is_refused_with_both_versions.

#### PRJ-11: A directory that cannot be watched does not block opening

- Priority P2, kind fix, effort S, golden impact none.
- Findings: projects-15, readiness-11.
- Depends on: nothing.
- Locations and files: `crates/ludenea-project/src/service.rs:342-344`, `crates/ludenea-project/src/files.rs:342-372,523-541`, `crates/ludenea-project/src/plugins.rs:154-155`.
- Acceptance: A watch failure skips that directory and announces a warning naming it and the cause (fs.inotify.max_user_watches on Linux); watcher errors are forwarded as warnings; a rescan re-lists.
- First failing test: tests/project.rs (unix): with a top-level directory set to mode 000, open and watch succeed and a warning names the directory.

#### PRJ-12: ludenea new friction removed

- Priority P3, kind fix, effort S, golden impact none.
- Findings: projects-18, dx-live-21.
- Depends on: nothing.
- Locations and files: `crates/ludenea-project/src/template.rs:50-55`, `crates/ludenea-serve/src/main.rs:9-24,87-99`.
- Acceptance: Reserved names (Rust keywords, std, core, alloc, proc_macro, test, ludenea and ludenea_*, serde) are refused with the reason; an existing empty directory is accepted (new .); invalid names get a suggestion; LUDENEA_ENGINE_PATH is read; the next steps are printed; --version works.
- First failing test: crates/ludenea-serve/tests/new.rs new_refuses_reserved_names and new_dot_in_an_empty_directory_creates_the_project_there.

#### PRJ-13: One editor-connection handle shared by serve and desktop

- Priority P3, kind deep-refactor, effort M, golden impact none.
- Findings: projects-16, desktop-11.
- Depends on: DSK-2.
- Locations and files: `crates/ludenea-serve/src/serve.rs:182-243,300-302`, `editor/apps/desktop/src-tauri/src/main.rs:46-245`.
- Acceptance: ProjectService::connect() returns a ProjectConnection that answers text with ordered messages (reply plus replay), streams later events with the lag warning built in, and releases the editor flag on drop; plain_name and plugin bundle serving live in ludenea-project; serve and desktop shrink to framing; desktop channel names are typed.
- First failing test: ludenea-project a_connection_replays_then_streams_every_event_once.

#### PRJ-14: Editor composition in a library package

- Priority P3, kind deep-refactor, effort M, golden impact none.
- Findings: projects-17, ts-quality-10.
- Depends on: CTL-1.
- Locations and files: `editor/apps/web/src/app.ts`, `editor/apps/web/src/games.ts`, `editor/apps/web/src/session-guard.ts`, `editor/apps/desktop/src/main.tsx:2-3`, `editor/packages/control-client/src/index.ts`.
- Acceptance: startEditor, game following and the session guard live in a library package (for example @ludenea/editor-app) with their tests; the applications are thin entries; stylesheets are exported by their packages; fakeTransport moves to a testing entry and client internals stay internal; a test forbids one application importing another.
- First failing test: editor/apps/web/test/imports.test.ts an_application_imports_no_other_application.

#### PRJ-15: One bindgen configuration

- Priority P3, kind fix, effort S, golden impact none.
- Findings: projects-21.
- Depends on: nothing.
- Locations and files: `xtask/src/main.rs:36-46`, `crates/ludenea-project/src/build.rs:262-276`.
- Acceptance: ludenea_project::bind_game is used by Project::build and xtask.
- First failing test: n/a (refactor): the build tests and the browser golden test pass.

#### PRJ-16: Build output parsing unit-tested

- Priority P3, kind fix, effort M, golden impact none.
- Findings: test-strategy-15.
- Depends on: nothing.
- Locations and files: `crates/ludenea-project/src/build.rs:144-206`, `crates/ludenea-project/src/types.rs:26`.
- Acceptance: follow_build(stream, package, target, output) is extracted and tested with recorded cargo JSON (an error with a span, a warning, native and wasm artifacts, another package, a malformed line); write_game_types is tested with a synthetic GameInfo.
- First failing test: build.rs a_compiler_error_becomes_a_diagnostic_with_file_line_and_column.

#### PRJ-17: A second editor on the same project is detected

- Priority P3, kind put-in-place, effort M, golden impact none.
- Findings: readiness-17.
- Depends on: nothing.
- Locations and files: `crates/ludenea-project/src/files.rs:251-256`, `crates/ludenea-serve/src/serve.rs:182-236`.
- Acceptance: The service warns the second editor that says hello that stored sessions are shared; a lock file under .ludenea/ makes a second service warn too.
- First failing test: serve.rs a_second_editor_is_warned.

### PLG: Editor plugin system (grade D)

Subsystem: editor-shell plugin API, loader and registry; plugin bundling.

**Problem.** The loader's rollback, the registry's ownership and the bundler are good, but one plugin component that throws while rendering unmounts the whole editor, built-in plugins coordinate through private WeakMap channels project plugins cannot reach (the 'no privileged API' anti-goal), inspectors and overlays are not observable so hot reload leaves stale ones, registrations made after an await leak, runtime errors carry no source location, settings are half done, and the typed API covers reads only.

**Target state.** Every plugin slot is isolated by an error boundary; built-ins use only the public API (edits, console, viewport, assets, build state, scene tree groups); every registration kind is observable; registrations belong to their plugin whatever the time of the call; errors point at source lines; settings have a panel; writes are typed.

#### PLG-1: Error boundaries around every plugin-rendered slot

- Priority P0, kind fix, effort M, golden impact none.
- Findings: plugins-1, errors-4, ts-quality-1.
- Depends on: ERR-1.
- Locations and files: `editor/packages/editor-shell/src/layout/Shell.tsx:15-27`, `editor/packages/editor-shell/src/layout/mount.tsx:9-12`, `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:257-275`, `editor/packages/editor-builtins/src/inspector/fields/Field.tsx:26-46`, `editor/packages/editor-builtins/src/explorer/ExplorerPanel.tsx:38-44`, `editor/packages/editor-builtins/src/viewport/overlay.ts:53-59`.
- Acceptance: A PluginBoundary in editor-shell wraps every panel, custom inspector (falling back to the schema form), decoration and explorer cell. The fallback names the panel and the plugin, shows the error, offers Reload panel and resets on a new registration. Failures reach editor.console once. mountShell passes onUncaughtError and onCaughtError. A failing overlay is reported once, not on every frame.
- First failing test: shell.test.tsx a_panel_that_throws_while_rendering_leaves_the_other_panels_and_the_palette_working.

#### PLG-2: Every registration kind is observable; hot reload replaces inspectors and overlays

- Priority P1, kind fix, effort M, golden impact none.
- Findings: plugins-5, ts-quality-4, plugins-17.
- Depends on: nothing.
- Locations and files: `editor/packages/editor-shell/src/editor.ts:28-54,139-165`, `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:257`, `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:123-152`, `editor/packages/editor-shell/src/palette/CommandPalette.tsx:54`, `editor/packages/editor-builtins/src/shared/index.ts:95-99`.
- Acceptance: Panels, commands, shortcuts, overlays, inspectors and settings expose list and subscribe like decorations and columns; Section reads inspectors through useSyncExternalStore; the viewport repaints on overlay changes; editor.panels.show(id) replaces the command-id convention; the end-to-end plugin test asserts the reloaded inspector without a click.
- First failing test: inspector.test.tsx a_custom_inspector_registered_or_reloaded_after_render_replaces_the_shown_one, and registry.test.ts every_registration_kind_lists_and_notifies_changes.

#### PLG-3: No privileged channels: built-ins use only the public editor API

- Priority P1, kind deep-refactor, effort L, golden impact none.
- Findings: plugins-4, shell-3, ts-quality-3, ambition-3, dx-live-7, sessions-15, gap-1-10, ambition-8, branches-19.
- Depends on: ERR-1, PLG-2, VER-3.
- Locations and files: `editor/packages/editor-builtins/src/shared/index.ts:15-92`, `editor/packages/editor-builtins/src/timeline/editing.tsx:5-29`, `editor/packages/editor-builtins/src/console/index.tsx:41-51`, `editor/packages/editor-builtins/src/prefabs/index.tsx:183-193`, `editor/packages/editor-builtins/src/inspector/index.tsx:109-115`, `editor/packages/editor-builtins/src/divergence/index.tsx:1-330`, `editor/packages/editor-builtins/src/viewport/index.tsx:17-102`, `editor/packages/editor-shell/src/editor.ts`, `editor/packages/editor-shell/src/index.ts`, `crates/ludenea-control/src/protocol.rs`, `crates/ludenea-control/src/host.rs`.
- Acceptance: The host emits SceneChanged { load_addresses, instances } after every scene edit, undo, redo and replace. Editor gains edits.run(command) (resimulation guard, refresh, one edited notification whoever sent the command) and onEdited, viewport.requestRepaint and observable hidden layers (ViewportOverlay.subscribe), assets.reveal(path), an observable project build state, and a scene tree groups registration point. entities.set and assets.setField route through edits.run. shared(), the console sinks, the timeline editing WeakMap and logToConsole are deleted. Built-ins import only public modules (a public UI module exports EntityList, Field, a store helper and ResimulatingNotice). The divergence plugin is split into viewer, report and ghost.
- First failing test: editor-builtins imports.test.ts builtins_import_no_other_plugin_folder, prefabs.test.tsx a_spawn_sent_by_a_project_plugin_refreshes_the_instance_groups, and viewport.test.tsx an_overlay_written_like_a_project_plugin_repaints_the_paused_viewport.

#### PLG-4: Custom inspectors keep decorations and receive readOnly and the schema

- Priority P2, kind fix, effort M, golden impact none.
- Findings: ambition-7.
- Depends on: PLG-3.
- Locations and files: `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:254-276`, `editor/packages/editor-builtins/src/inspector/fields/Field.tsx:23-40`, `editor/packages/editor-shell/src/plugin.ts:39-44`, `README.md:183-190`.
- Acceptance: Component-level decorations render in the section header whatever the body; InspectorComponent receives readOnly, the schema and a decorations(path) helper (or the public Field); inspectors.register accepts resource and asset type names.
- First failing test: prefabs.test.tsx a_custom_inspector_keeps_the_override_actions.

#### PLG-5: Plugin loading never waits on a hung bundle

- Priority P3, kind fix, effort S, golden impact none.
- Findings: plugins-3.
- Depends on: ERR-1.
- Locations and files: `editor/apps/web/src/app.ts:39-70`, `editor/packages/editor-shell/src/loader.ts:55-59`.
- Acceptance: One chain per bundle name with a generation number; an import times out after 10 seconds with a Console error naming the bundle and the likely top-level await; project switches deactivate without waiting behind pending imports.
- First failing test: editor/apps/web/test/app.test.ts a_plugin_that_never_finishes_loading_blocks_neither_the_others_nor_its_next_rebuild.

#### PLG-6: Plugin errors point at the source file and line

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: plugins-6.
- Depends on: ERR-1.
- Locations and files: `crates/ludenea-project/src/plugins.rs:321-339,521-529`, `editor/apps/web/src/app.ts:50-55`, `editor/packages/editor-shell/src/loader.ts:7-13`.
- Acceptance: Bundles carry inline source maps; the Console logs the stack and maps the top plugin frame to editor/index.ts:line:column through a project request, as a clickable location; loader messages name the source entry; rejected imports carry their line and column.
- First failing test: crates/ludenea-project/tests/plugins.rs rejected_import_reports_line_and_column and bundles_carry_an_inline_source_map_to_the_entry.

#### PLG-7: Registrations owned by their plugin whatever the time of the call

- Priority P2, kind fix, effort M, golden impact none.
- Findings: plugins-8.
- Depends on: PLG-2.
- Locations and files: `editor/packages/editor-shell/src/loader.ts:26-41`, `editor/packages/editor-shell/src/plugin.ts:12-20`, `editor/packages/editor-shell/src/registry.ts:73-116`.
- Acceptance: First, an activate returning a thenable or a non-function is refused with the plugin's name and what it registered is disposed. Then a per-plugin Editor facade charges late registrations to their plugin, and a Promise-returning activate is awaited on load and reload.
- First failing test: loader.test.ts activate_returning_a_promise_is_refused_with_its_name, then reload_of_a_plugin_whose_activate_awaits_before_registering_replaces_its_registrations.

#### PLG-8: Typed writes in the plugin API

- Priority P3, kind put-in-place, effort L, golden impact none.
- Findings: plugins-9, ts-quality-18, dx-live-14.
- Depends on: PLG-3, PRJ-9.
- Locations and files: `editor/packages/editor-shell/src/entities.ts:13-16`, `editor/packages/editor-shell/src/assets.ts:12-13`, `editor/packages/editor-shell/src/plugin.ts:37-45`, `crates/ludenea-schema/src/typescript.rs:184`.
- Acceptance: entities.set and InspectorComponent.onChange are typed by key-path tuples over ComponentTypes[N], with field and index helpers for dynamic paths; handles are branded per asset type and assets.get infers from them with a runtime type check; an augmentable CommandTypes types commands.run; engine component types are always in ComponentTypes.
- First failing test: A tsc-checked typing test with @ts-expect-error on a misspelled path and a wrong value type, and assets.get(ball.settings) typed as Settings.

#### PLG-9: A cargo metadata failure does not take the project plugin down

- Priority P2, kind fix, effort M, golden impact none.
- Findings: plugins-12.
- Depends on: nothing.
- Locations and files: `crates/ludenea-project/src/plugins.rs:116-148,185-187,220-270`.
- Acceptance: The project's own entry is always bundled; a dependency listing failure is a warning; the watch covers editor/ unconditionally plus Cargo.toml and Cargo.lock and recomputes dependency entries; PluginRemoved deactivates a plugin that disappeared.
- First failing test: tests/plugins.rs a_project_plugin_bundles_and_reloads_while_cargo_toml_is_broken.

#### PLG-10: Settings: a panel, observability and validated values

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: plugins-14, shell-13, ts-quality-16.
- Depends on: PLG-2.
- Locations and files: `editor/packages/editor-shell/src/plugin.ts:47-54`, `editor/packages/editor-shell/src/editor.ts:50-54,166-182`, `editor/packages/editor-builtins/src/project/index.tsx:38-39,70,158`.
- Acceptance: A built-in Settings panel and Open settings command render each registered setting by kind; settings are observable (subscribe per id, a useSetting hook); stored values are validated against the kind, falling back to the default with a warning; register returns a typed Setting<T> handle; New project remembers the engine path.
- First failing test: settings.test.tsx a_registered_plugin_setting_is_listed_and_edited_in_the_settings_panel, and project.test.tsx creating_a_project_remembers_the_engine_path.

#### PLG-11: A plugin author guide and a typed template plugin

- Priority P3, kind put-in-place, effort M, golden impact none.
- Findings: plugins-16.
- Depends on: PLG-3, PRJ-9, VER-9.
- Locations and files: `README.md:177-193`, `crates/ludenea-project/template/editor/index.ts`, `crates/ludenea-physics/editor/index.tsx:84-85`.
- Acceptance: The guide covers lifecycle and ownership, hot reload, imports and shared modules, styling, error reporting, typed access per registration kind, and type checking; the template plugin is a small working typed panel; the README snippet is compiled by the type-check test.
- First failing test: n/a (documentation): the snippet compiles in the ignored type-check test.

#### PLG-12: The shared modules list has one source

- Priority P3, kind fix, effort S, golden impact none.
- Findings: plugins-18.
- Depends on: nothing.
- Locations and files: `crates/ludenea-project/src/plugins.rs:28-34,512-516`, `editor/shared/plugin.ts:6-15`, `editor/apps/web/index.html:8-19`, `editor/apps/desktop/index.html`.
- Acceptance: SHARED_MODULES in editor/shared/plugin.ts is the source; both index.html import maps are generated by a transformIndexHtml hook; a test asserts the bundler externals equal it; the rejection message is built from the constant.
- First failing test: editor/shared vitest the_bundler_externals_and_the_import_maps_list_the_same_modules.

#### PLG-13: JSON imports and the vendored rolldown patch tested

- Priority P3, kind fix, effort S, golden impact none.
- Findings: plugins-19.
- Depends on: nothing.
- Locations and files: `crates/ludenea-project/src/plugins.rs:38`, `vendor/rolldown_common/src/ecmascript/json_to_program.rs:78`, `crates/ludenea-project/tests/plugins.rs`.
- Acceptance: Bundling tests import a JSON file with integers, floats and a large integer and assert the emitted literals; a JSON file holding 1e400 yields a located error diagnostic, not a panic.
- First failing test: tests/plugins.rs a_json_import_bundles_its_numbers.

#### PLG-14: ListPlugins answered from the watch, outside the service lock

- Priority P3, kind fix, effort M, golden impact none.
- Findings: plugins-20.
- Depends on: PLG-9.
- Locations and files: `crates/ludenea-project/src/service.rs:265-277`, `crates/ludenea-serve/src/serve.rs:195-198`, `crates/ludenea-project/src/plugins.rs:259-270`.
- Acceptance: Bundles produced by the watch are kept per plugin and returned by ListPlugins; the dependency listing runs outside the service lock.
- First failing test: tests/service.rs list_files_answers_while_plugins_are_being_listed.

### SHL: Editor shell and panels (grade D)

Subsystem: editor-shell layout, palette, shortcuts and stores; built-in panels (console, inspector, scene tree, viewport, timeline).

**Problem.** The registry, palette accessibility, virtualized lists and document store are solid, but Save marks the scene saved before the file is written (a failed write loses the edits silently), the palette runs alphabetically first matches (typing Undo nudges the selection), the layout is never saved, empty states ignore the build, the Console neither virtualizes nor follows its tail, shortcuts are swallowed by text fields and Delete does nothing on Mac laptops, entity lists have no keyboard navigation, inspector inputs collapse to 18px on overridden instances and show f32 noise, and maps and entity references cannot be edited.

**Target state.** Saving is two-phase; commands are found by quality and their failures shown; layout persists; panels say what the build is doing; lists are keyboard listboxes; the inspector edits every value shape legibly.

#### SHL-1: Save scene is two-phase: the scene stays unsaved until the write succeeds

- Priority P0, kind fix, effort M, golden impact none.
- Findings: shell-1, ambition-1.
- Depends on: VER-3, ERR-1.
- Locations and files: `crates/ludenea-control/src/host.rs:518-522`, `crates/ludenea-control/src/session.rs:955-959,1305`, `crates/ludenea-control/src/protocol.rs`, `editor/packages/editor-builtins/src/scene-tree/index.tsx:23-36`, `editor/packages/editor-builtins/src/inspector/index.tsx:87-107`.
- Acceptance: SaveScene returns the text and leaves the scene dirty; MarkSceneSaved { text } clears it only when the text still equals the session scene, so an edit made during the write stays unsaved. scene.save marks saved only after write_text succeeds and reports a failed write as Cannot save <path>: <reason>. The host tracks data asset dirtiness the same way (unsaved assets in Timeline).
- First failing test: crates/ludenea-control/tests/editing.rs save_scene_keeps_the_scene_dirty_until_marked_saved, and scene-tree.test.tsx a_failed_write_keeps_the_scene_unsaved_and_says_why.

#### SHL-2: The command palette ranks matches by quality

- Priority P1, kind fix, effort S, golden impact none.
- Findings: shell-6, ux-live-1, gap-1-15.
- Depends on: nothing.
- Locations and files: `editor/packages/editor-shell/src/palette/CommandPalette.tsx:6-66`.
- Acceptance: Matches rank exact title, title prefix, word starts, contiguous substring, then scattered letters with a gap penalty, titles before identifiers; ties break by recent use then title. Typing undo and Enter runs Undo; typing Place prefab runs Place prefab.
- First failing test: palette.test.tsx the_best_match_comes_first.

#### SHL-3: Dock layout persisted per project; primary panels active; Reset layout

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: shell-7, ts-quality-15, ux-live-9.
- Depends on: PLG-2.
- Locations and files: `editor/packages/editor-shell/src/layout/Shell.tsx:33-134`, `editor/packages/editor-shell/src/plugin.ts:23-31`.
- Acceptance: The dock state is saved (debounced) per project under .ludenea/layout.json or browser storage keyed by root, and restored in onReady, dropping panels no longer registered and adding new ones at their defaults. Panels are added inactive and each group has a declared primary panel (Viewport, Inspector, Timeline, Scene). A Reset layout command exists.
- First failing test: shell.test.tsx a_layout_saved_by_one_shell_is_restored_by_the_next and the_viewport_stays_active_when_a_plugin_adds_a_center_panel.

#### SHL-4: Build-aware empty states

- Priority P2, kind fix, effort M, golden impact none.
- Findings: shell-8, ux-live-10.
- Depends on: PLG-3.
- Locations and files: `editor/packages/editor-builtins/src/scene-tree/SceneTree.tsx:16`, `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:291-293`, `editor/packages/editor-builtins/src/explorer/ExplorerPanel.tsx:17`, `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:21`, `editor/packages/editor-builtins/src/timeline/TimelinePanel.tsx:51`.
- Acceptance: One empty-state component reads the public build and game state (none, building with progress, failed with Show console, running) and every game panel uses it, plugins included. EntityList says No entity matches <filter> and shows the total. See the console is a link. Compiling lines fold into one progress entry.
- First failing test: Built-ins test game_panels_say_building_while_the_first_build_runs.

#### SHL-5: The Console is virtualized, follows its tail, and can be cleared and filtered

- Priority P2, kind fix, effort M, golden impact none.
- Findings: shell-10, ts-quality-13.
- Depends on: ERR-1.
- Locations and files: `editor/packages/editor-builtins/src/console/index.tsx:6-87`, `editor/packages/editor-builtins/src/console/ConsolePanel.tsx:28-58`.
- Acceptance: The list uses the existing virtualizer over a ring buffer with additions batched per frame; it follows the tail while at the bottom and offers Jump to latest otherwise; a failed build reveals its first error; Clear and a text filter exist; sources show as badges; a Panicked location is clickable; the list is role=log with polite error announcements.
- First failing test: console.test.tsx new_lines_keep_a_bottom_scrolled_view_at_the_bottom and renders_only_visible_lines_for_5000_entries.

#### SHL-6: Shortcuts work from text fields and on every keyboard

- Priority P2, kind fix, effort M, golden impact none.
- Findings: shell-9, shell-15.
- Depends on: PLG-2.
- Locations and files: `editor/packages/editor-shell/src/shortcuts.ts:42-108`, `editor/packages/editor-shell/src/palette/CommandPalette.tsx:39-50`, `editor/packages/editor-shell/src/registry.ts:31-35`, `editor/packages/editor-builtins/src/scene-tree/SceneTree.tsx:28-36`, `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:245`.
- Acceptance: F5 and Mod combinations run from text fields (except the field's own editing keys), blurring first so the value commits. Backspace despawns or deletes on macOS. Letters match by code when Alt is held. The palette is a registered command on Mod+K. Panel keys are focus-scoped shortcuts. A conflicting plugin shortcut is a warning, not a failure. User overrides come from a keybindings setting. shortcuts.keyOf gives button hints.
- First failing test: palette.test.tsx mod_s_in_a_text_field_commits_it_then_saves, and scene-tree.test.tsx backspace_despawns_the_selection_on_macos.

#### SHL-7: Entity lists are keyboard listboxes that reveal the selection

- Priority P2, kind put-in-place, effort L, golden impact none.
- Findings: shell-5, ux-live-20.
- Depends on: AUT-2.
- Locations and files: `editor/packages/editor-builtins/src/shared/EntityList.tsx:84-117`, `editor/packages/editor-builtins/src/scene-tree/SceneTree.tsx:29-59`, `editor/packages/editor-builtins/src/explorer/ExplorerPanel.tsx:47-89`, `crates/ludenea-control/src/protocol.rs`.
- Acceptance: One tab stop (roving tabindex); ArrowUp and ArrowDown, Home and End, PageUp and PageDown; Shift extends and Ctrl or Cmd toggles; the virtualizer scrolls to the focused row. A locate_entity { filter, entity } query returns an entity's index, used by a Go to entity command and to reveal a selection made elsewhere. The explorer shows its match count.
- First failing test: scene-tree.test.tsx arrow_down_moves_the_active_row_and_selects_it and selecting_an_entity_in_the_viewport_scrolls_the_scene_tree_to_it.

#### SHL-8: Accessibility basics

- Priority P3, kind fix, effort M, golden impact none.
- Findings: shell-14.
- Depends on: FND-2.
- Locations and files: `editor/packages/editor-shell/src/palette/CommandPalette.tsx:26-59`, `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:288`, `editor/packages/editor-shell/src/theme.css:53-56`, `editor/packages/editor-builtins/src/console/ConsolePanel.tsx:46`.
- Acceptance: The palette restores focus when it closes; the viewport uses the theme's focus ring and describes its keys; the explorer tabs follow the tabs pattern; non-interactive rows lose tabIndex; the console is role=log; eslint-plugin-jsx-a11y recommended rules run on editor/packages.
- First failing test: palette.test.tsx closing_the_palette_returns_focus_to_where_it_was.

#### SHL-9: EntityList keeps only the pages in view

- Priority P2, kind fix, effort S, golden impact none.
- Findings: shell-16.
- Depends on: INT-1.
- Locations and files: `editor/packages/editor-builtins/src/shared/EntityList.tsx:30,62-82`, `editor/e2e/performance.spec.ts:44-79`.
- Acceptance: The rows map holds the visible pages plus one on each side; the performance end-to-end test scrolls the scene tree to the end of the bench scene and reports its 95th percentile frame time.
- First failing test: scene-tree.test.tsx rows_kept_stay_bounded_after_scrolling_100000_entities (at most about 600 cached rows).

#### SHL-10: The document store as an explicit state machine; large components split

- Priority P3, kind deep-refactor, effort L, golden impact none.
- Findings: shell-17, ts-quality-9.
- Depends on: VER-5, AST-2, AST-7.
- Locations and files: `editor/packages/editor-shell/src/stores/document.ts:178-471`, `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:48-284`, `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:1-375`.
- Acceptance: document.ts is split into a session codec, a document loader and an asset-reload coordinator state machine (idle, opening, loaded, reloading, restoring) with one serialized queue and AbortController cancellation; the viewport effect is split into useRenderer, useFrameFeed, usePointerTools and useViewportKeys; the inspector is split into data asset, resource and add-component files; the 25 document tests stay green.
- First failing test: document.test.ts interleaved_opens_reloads_and_game_changes_keep_the_last_open_document_loaded_once_per_game, plus reload-coordinator unit tests.

#### SHL-11: Scene tree list errors and action errors kept apart

- Priority P2, kind fix, effort S, golden impact none.
- Findings: shell-18, gap-1-8.
- Depends on: ERR-1.
- Locations and files: `editor/packages/editor-builtins/src/scene-tree/SceneTree.tsx:24-52`, `editor/packages/editor-builtins/src/shared/EntityList.tsx:66-76`.
- Acceptance: listError and actionError are separate; an action error stays until the next action or Dismiss, uses role=alert and is logged to editor.console; the list refresh clears only its own error.
- First failing test: scene-tree.test.tsx a_despawn_error_stays_after_the_rows_refresh.

#### SHL-12: Viewport and list multi-selection behave consistently

- Priority P3, kind fix, effort S, golden impact none.
- Findings: shell-19, gap-1-14.
- Depends on: AUT-1.
- Locations and files: `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:164-221`, `editor/packages/editor-builtins/src/viewport/overlay.ts:44-45`, `editor/packages/editor-shell/src/stores/selection.ts:8-29`, `editor/packages/editor-builtins/src/shared/EntityList.tsx:85-90`.
- Acceptance: A drag moves every selected entity by one offset in one batch and outlines them all; Ctrl or Cmd-click toggles membership (a toggle action in the selection store) in the viewport and the lists; Shift selects a range in lists; a repeated click on one spot cycles through the picked entities.
- First failing test: viewport.test.tsx dragging_a_multi_selection_moves_every_selected_entity, and scene-tree.test.tsx cmd_click_on_a_selected_row_deselects_it.

#### SHL-13: Shared helpers replace duplicated ones

- Priority P3, kind fix, effort S, golden impact none.
- Findings: shell-20, ts-quality-20.
- Depends on: PLG-3.
- Locations and files: `editor/packages/editor-shell/src/stores/selection.ts:17`, `editor/packages/editor-builtins/src/viewport/overlay.ts:21`, `editor/packages/editor-builtins/src/viewport/camera.ts:39`, `editor/packages/editor-shell/src/entities.ts:27-41`, `editor/packages/editor-shell/src/assets.ts:54-58`, `editor/packages/editor-shell/src/stores/branches.ts:24-28`.
- Acceptance: sameEntity and entityKey are exported from control-client next to Entity; one editor.requireGame() with one message; one errorText; shared/index.ts split by concern.
- First failing test: control-client test/entity.test.ts same_entity_compares_index_and_generation.

#### SHL-14: Inspector inputs never collapse; override marks sit in the component header

- Priority P1, kind fix, effort M, golden impact none.
- Findings: ux-live-3.
- Depends on: nothing.
- Locations and files: `editor/packages/editor-builtins/src/inspector/fields/Field.tsx:27-38`, `editor/packages/editor-builtins/src/panels.css:566-593`, `editor/packages/editor-builtins/src/prefabs/OverrideMark.tsx:18-51`.
- Acceptance: The component-level override mark and its actions render in the component header next to Remove and Apply to scene; field decorations sit after the input on the same row and wrap on narrow panels; the value column keeps a minimum width.
- First failing test: editor/e2e/prefabs.spec.ts: after overriding position.x, the spinbutton is at least 80px wide and exactly one Revert position.x to prefab button exists (plus a vitest structural test).

#### SHL-15: Inspector number and text fields behave predictably

- Priority P2, kind fix, effort S, golden impact none.
- Findings: ux-live-17, ux-live-22, sessions-13.
- Depends on: nothing.
- Locations and files: `editor/packages/editor-builtins/src/inspector/fields/inputs.tsx:24-81`, `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:176-192`.
- Acceptance: f32 values show the shortest decimal that round-trips through Math.fround; a dot-based text input with inputmode=decimal makes display and parsing independent of locale; a rejected edit marks the input aria-invalid, Escape restores the live value and only the reason is shown; a focused field keeps its draft while the live value changes.
- First failing test: inspector.test.tsx an_f32_field_shows_its_shortest_decimal and a_field_being_typed_in_keeps_its_text_when_the_value_changes.

#### SHL-16: Maps and entity references editable in the inspector

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: sessions-16, ts-quality-17.
- Depends on: SHL-15.
- Locations and files: `editor/packages/editor-builtins/src/inspector/fields/Field.tsx:99-104`, `editor/packages/editor-builtins/src/inspector/fields/inputs.tsx:115-124`, `editor/packages/editor-builtins/src/assets/value.ts:14-38`, `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:267,301-316`.
- Acceptance: A MapField adds, edits and removes entries with keys edited through the key schema and sends the whole map; entity references can be picked from the scene tree or viewport or typed as index and generation, validated by the host; the new data asset form edits maps; a read-only value says why.
- First failing test: fields.test.tsx map_entries_are_edited_by_sending_the_whole_map and entity_reference_can_be_set_to_another_entity.

#### SHL-17: The selection survives scrubbing before an entity exists and notices a despawn

- Priority P2, kind fix, effort S, golden impact none.
- Findings: sessions-14.
- Depends on: AUT-2.
- Locations and files: `editor/packages/editor-builtins/src/inspector/index.tsx:17-25`, `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:181-185`, `editor/packages/editor-shell/src/entities.ts:48-56`.
- Acceptance: The selection is kept; the inspector says Entity 12v0 does not exist at tick 100 and shows values again once it exists; EntityAccess.watch handles entity_despawned.
- First failing test: inspector.test.tsx a_selected_entity_missing_at_the_current_tick_stays_selected_and_says_so.

#### SHL-18: Read-only resources reported by the game

- Priority P3, kind fix, effort S, golden impact none.
- Findings: sessions-21.
- Depends on: VER-3.
- Locations and files: `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:13-14,82`, `crates/ludenea-app/src/edit.rs:304-307`, `crates/ludenea-control/src/protocol.rs:140-158`.
- Acceptance: NamedSchema carries read_only, set by the app for Time and declarable by plugins; the inspector reads it; the hardcoded set is deleted.
- First failing test: inspector.test.tsx a_resource_the_game_reports_read_only_is_read_only.

#### SHL-19: Small editor polish: reveal, rebuild hint, consistent titles, palette parity

- Priority P3, kind fix, effort M, golden impact none.
- Findings: ux-live-15, ux-live-21, ux-live-24, ambition-13.
- Depends on: PLG-3.
- Locations and files: `editor/packages/editor-builtins/src/assets/index.tsx:195-198`, `editor/packages/editor-builtins/src/timeline/index.tsx:166-178`, `editor/packages/editor-builtins/src/project/index.tsx`, `editor/packages/editor-builtins/src/prefabs/OverrideMark.tsx:37-46`, `editor/packages/editor-builtins/src/divergence/CausalReport.tsx:49`, `editor/packages/editor-builtins/src/divergence/TraceViewer.tsx:44`, `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:132-145`.
- Acceptance: Reveal expands, selects and scrolls without replacing the filter (clearing it only when it hides the item) and Save as replay confirms with its path. A Game code changed: Build notice appears when src/ or Cargo.toml change after GameReady, with a Build shortcut. Buttons read their command's title; type short names carry full-name tooltips; stale and asset actions have tooltips; the viewport has a ghost legend; delete confirmations say the action cannot be undone; a shortcut help dialog opens on the question mark. Per-field Revert and Apply to prefab and divergence Extend and Seek are commands the buttons call.
- First failing test: assets.test.tsx reveal_keeps_the_filter_and_selects_the_item, timeline.test.tsx each_toolbar_button_text_equals_its_command_title, and keyboard.test.ts listing prefabs.revert-focused-field and divergence.extend.

### AUT: Scene authoring in the editor (grade D)

Subsystem: the level-building workflow across scene tree, inspector, viewport and documents.

**Problem.** Tick-0 edits, undo and the scene format are sound, but a designer cannot build a level comfortably: multi-entity commands send one edit per entity (after playing, only the first applies, and each entity is its own undo step), no command creates an entity, Add component inserts schema zeros the game refuses (Pose and Collider) or that misbehave (Body without gravity), the selection holds runtime handles that point at another entity after a reload, there is no New scene, Save as, copy, paste or duplicate, entities have no names, and no test builds a scene the way a designer does.

**Target state.** Multi-entity edits are atomic batches; creating, naming, duplicating and saving entities and scenes are first-class commands; components start from the game's defaults; the selection follows scene entities by load address; an end-to-end test builds a level from nothing.

#### AUT-1: Multi-entity edits are one batch

- Priority P0, kind deep-refactor, effort L, golden impact none.
- Findings: gap-1-1.
- Depends on: VER-3.
- Locations and files: `crates/ludenea-control/src/protocol.rs`, `crates/ludenea-control/src/host.rs`, `crates/ludenea-control/src/session.rs:565-577,1950-1985`, `crates/ludenea-app/src/edit.rs`, `editor/packages/editor-builtins/src/scene-tree/index.tsx:19`, `editor/packages/editor-builtins/src/viewport/index.tsx:49`, `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:215-219`, `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:26`.
- Acceptance: Command::Batch { commands } applies as one scene edit at tick 0 (one undo entry, one resimulation, all or nothing) and as one timed edit later. Despawn selection, nudge, viewport drag and inspector edits over a multi-selection send one batch. The inspector shows N entities selected with their shared components (mixed values marked) and edits all of them through one batch.
- First failing test: crates/ludenea-control/tests/editing.rs a_batch_of_tick_zero_despawns_after_play_is_one_undo_step, and scene-tree.test.tsx despawning_three_selected_entities_sends_one_batch.

#### AUT-2: The selection is keyed by load address

- Priority P1, kind fix, effort M, golden impact none.
- Findings: gap-1-4.
- Depends on: PLG-3.
- Locations and files: `editor/packages/editor-shell/src/stores/selection.ts:4-29`, `editor/packages/editor-builtins/src/scene-tree/SceneTree.tsx:56`, `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:45-46`, `crates/ludenea-control/src/session.rs:1947-1985`.
- Acceptance: Selected scene entities are stored by load address and remapped to runtime entities after every scene reload (edit, undo, redo, replace), dropping missing ones; the selection clears when the open document changes; tree rows and the inspector header show the local identifier (#6).
- First failing test: scene-tree.test.tsx selection_follows_the_same_scene_entity_after_a_tick_zero_reload, and an editor-shell test opening_another_document_clears_the_selection.

#### AUT-3: A Create entity command

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: gap-1-2.
- Depends on: ERR-1.
- Locations and files: `editor/packages/editor-builtins/src/scene-tree/index.tsx:13-37`, `editor/packages/editor-builtins/src/scene-tree/SceneTree.tsx`.
- Acceptance: scene.create-entity (Mod+Shift+N, a Scene panel button, a button in an empty tree) spawns an empty entity at tick 0, selects it and opens Add component; past tick 0 it says seek to tick 0 to add an entity to the scene.
- First failing test: scene-tree.test.tsx create_entity_spawns_an_empty_entity_at_tick_zero_and_selects_it and create_entity_past_tick_zero_says_to_seek.

#### AUT-4: Add component inserts the type's default value

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: gap-1-9, ambition-5.
- Depends on: VER-3.
- Locations and files: `editor/packages/editor-builtins/src/inspector/default-value.ts:5-58`, `editor/packages/editor-builtins/src/inspector/InspectorPanel.tsx:318-375`, `crates/ludenea-physics/src/components.rs:132-176`, `crates/ludenea-ecs/src/component.rs`, `crates/ludenea-app/src/app.rs`, `crates/ludenea-control/src/protocol.rs`.
- Acceptance: Component types may register a default (filled when they implement Default, as assets do); NewComponentValue { component } replies with its JSON or no_default_value. Body and Collider defaults match their constructors (gravity scale 1, a valid shape) and Pose's rotation is the identity. The inspector inserts the default and falls back to the schema form only without one; defaultValue is no longer used for inserts or variant switches. (Host RON encoding is SCN-11; the input-log decoder removal is RUN-5.)
- First failing test: editing.rs new_component_value_is_the_registered_default, and inspector.test.tsx add_component_inserts_the_games_default_value.

#### AUT-5: New scene and Save scene as

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: gap-1-6.
- Depends on: SHL-1.
- Locations and files: `editor/packages/editor-builtins/src/scene-tree/index.tsx:23-36`, `editor/packages/editor-builtins/src/assets/index.tsx:214-228`, `editor/packages/editor-builtins/src/assets/AssetsPanel.tsx:62,158-176`.
- Acceptance: scene.new asks for a path under scenes/ (numbered default), creates a minimal version 2 scene through create_asset and opens it; scene.save-as (Mod+Shift+S) writes a new file, switches to it and keeps the session under the new identifier; the asset context menu offers Duplicate for scenes and prefabs.
- First failing test: assets.test.tsx new_scene_creates_a_scene_file_and_opens_it, and scene-tree.test.tsx save_as_writes_a_new_scene_file_and_switches_the_document.

#### AUT-6: The default scene can be set and follows renames

- Priority P3, kind put-in-place, effort M, golden impact none.
- Findings: gap-1-7.
- Depends on: PRJ-5.
- Locations and files: `crates/ludenea-project/src/lib.rs:94-100`, `crates/ludenea-project/src/service.rs:308-313`, `crates/ludenea-project/src/protocol.rs:84-145`, `crates/ludenea-project/src/files.rs:137-165`.
- Acceptance: SetDefaultScene { path } rewrites ludenea.toml keeping its other keys and announces the change; rename and delete update or report the default scene; Set as default scene is in the context menu and the palette; the asset browser marks the default scene.
- First failing test: Service test renaming_the_default_scene_updates_the_manifest_and_info.

#### AUT-7: Entities can be named

- Priority P3, kind put-in-place, effort L, golden impact none.
- Findings: gap-1-5.
- Depends on: AUT-2, SHL-7.
- Locations and files: `crates/ludenea-app/src/`, `editor/packages/editor-builtins/src/scene-tree/SceneTree.tsx:9,84-88`, `editor/packages/editor-builtins/src/prefabs/InstanceGroups.tsx`.
- Acceptance: A core ludenea::Name(String) component in the state and the hash; tree rows are labelled by name, else by components plus local identifier; scene.rename-entity edits it inline on F2; instance groups are labelled; the viewport draws selected names and scrolls the tree to a picked entity.
- First failing test: scene-tree.test.tsx scene_tree_labels_an_entity_by_its_name_and_local_id, and a scene test a_name_component_round_trips_through_save_scene_keeping.

#### AUT-8: Copy, paste and duplicate entities

- Priority P3, kind put-in-place, effort L, golden impact none.
- Findings: gap-1-3.
- Depends on: AUT-1, SCN-3.
- Locations and files: `crates/ludenea-control/src/protocol.rs:337-351`, `crates/ludenea-app/src/prefab.rs`, `editor/packages/editor-builtins/src/scene-tree/index.tsx`, `editor/packages/editor-builtins/src/prefabs/index.tsx:145-168`.
- Acceptance: InsertEntities { text } expands prefab-format text into scene entities as one tick-0 edit with fresh identifiers from next_id, internal references remapped and instance entries copied; Mod+C, Mod+V and Mod+D commands copy, paste and duplicate the selection.
- First failing test: editing.rs insert_entities_pastes_a_copied_group_with_fresh_ids_and_remapped_references.

#### AUT-9: End-to-end: a level built from nothing survives save and reopen

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: gap-1-13.
- Depends on: AUT-3, AUT-4, AUT-5, QA-6.
- Locations and files: `editor/e2e/scene-authoring.spec.ts (new)`.
- Acceptance: editor/e2e/scene-authoring.spec.ts creates a scene and an entity with Pose, Body and Collider (defaults accepted), plays, seeks to 0, multi-selects and nudges, saves, saves as, reopens in a fresh browser context and finds the entities with a falling dynamic body.
- First failing test: a_level_built_from_nothing_survives_a_save_and_reopen.

### INT: Explorer and introspection (grade D)

Subsystem: Explorer panel, introspection protocol and subscriptions.

**Problem.** Subscriptions are reference-counted, records are hash-validated and the filter parser is fully covered, but in the real dock the Entities list is not virtualized (it renders every row and freezes the UI at 100,000 entities), Events and Commands answer only from a passive 600-tick ring so ordinary states show impossible advice, events render as raw JSON with entities that cannot be clicked, an Entities watch rehashes every matching entity every frame even when paused, and most of these states have no test.

**Target state.** The Explorer scales in the real layout; any simulated tick can be introspected by re-running it; events read as fields with clickable entities; watches cost nothing when nothing changed; every state is tested.

#### INT-1: Explorer lists virtualized in the real dock

- Priority P0, kind fix, effort M, golden impact none.
- Findings: gap-3-1, ux-live-18.
- Depends on: nothing.
- Locations and files: `editor/packages/editor-builtins/src/panels.css:34-67`, `editor/packages/editor-builtins/src/shared/EntityList.tsx:36-78`, `editor/packages/editor-builtins/src/explorer/ExplorerPanel.tsx:27,46-89`, `editor/e2e/performance.spec.ts`.
- Acceptance: The explorer tabpanel is a column flex child (flex 1, min-height 0) and .ludenea-entities uses flex 1 and min-height 0 instead of height 100 percent; rows stay on one line with ellipsis; EntityList caps the pages one refresh may request to the visible range plus overscan; the listbox is wider than half the panel.
- First failing test: editor/e2e/performance.spec.ts the_explorer_lists_100000_entities_with_only_the_visible_rows_while_playing (fewer than 100 options, frames within budget).

#### INT-2: Events and Commands answered for any simulated tick

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: gap-3-2.
- Depends on: RUN-2.
- Locations and files: `crates/ludenea-control/src/subscriptions.rs:223-268`, `crates/ludenea-control/src/host.rs:886-946`.
- Acceptance: When no record matches and the tick is between 1 and simulated, the host re-runs the tick observed (on a scratch simulation, or restoring the current tick, with panics caught), validates it against the recorded hashes and caches it; requests are refused with Resimulating while the future is stale; tick 0 answers that no tick ran (the scene as loaded); the subscription remains only for timings.
- First failing test: crates/ludenea-control/tests/introspection.rs tick_events_are_answered_for_any_simulated_tick.

#### INT-3: Events rendered with their fields and clickable entities

- Priority P2, kind fix, effort M, golden impact none.
- Findings: gap-3-4.
- Depends on: INT-2, PLG-3.
- Locations and files: `editor/packages/editor-builtins/src/explorer/Introspection.tsx:42-98`, `crates/ludenea-physics/editor/index.tsx`.
- Acceptance: Events render with the read-only schema field renderer; Entity fields are buttons that select the entity; a header shows Tick N, k events with step buttons; replies are tagged with their tick so a list never shows under the wrong tick; the Physics panel's contacts use the same renderer.
- First failing test: explorer.test.tsx event_entities_are_links_that_select_the_entity.

#### INT-4: Introspection states tested at every level

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: gap-3-3.
- Depends on: INT-2.
- Locations and files: `crates/ludenea-control/tests/introspection.rs`, `editor/packages/editor-builtins/test/explorer.test.tsx`, `editor/e2e/explorer.spec.ts (new)`.
- Acceptance: Host tests cover every state (ring hit, re-run, tick 0, stale future, after a seek, after a branch switch); vitest renders non-empty Events and Commands, error states and the refetch on a tick change; editor/e2e/explorer.spec.ts steps to a Collision, scrubs to a checkpoint tick, switches branch and opens Entities on the bench scene.
- First failing test: explorer.test.tsx events_tab_lists_the_events_of_the_current_tick.

#### INT-5: Entity watches skip unchanged frames

- Priority P2, kind fix, effort M, golden impact none.
- Findings: gap-3-8, gap-5-3.
- Depends on: ECS-19.
- Locations and files: `crates/ludenea-control/src/subscriptions.rs:23,118-199`, `crates/ludenea-control/src/host.rs:142-196`, `crates/ludenea-control/src/stdio.rs:47-79`.
- Acceptance: deltas skip Entities watches when the tick, the hash and an edit generation are unchanged; while playing, Entities pushes are throttled like resource pushes; a paused frame runs no matching pass.
- First failing test: Subscriptions unit test with a counting Serialize component: ten paused advances hash nothing after the first pass; an ignored budget test paused_entities_watch_costs_nothing.

#### INT-6: Entity query errors positioned

- Priority P3, kind fix, effort M, golden impact none.
- Findings: gap-3-7.
- Depends on: nothing.
- Locations and files: `crates/ludenea-ecs/src/filter.rs:7-58`, `editor/packages/editor-builtins/src/explorer/ExplorerPanel.tsx:47-59`.
- Acceptance: The filter is tokenized and errors carry an offset and a length (Syntax { expected }, Unknown { did_you_mean }, IsResource); the InvalidFilter error carries the span; the input is marked invalid with the span underlined and component names suggested.
- First failing test: filter.rs missing_comma_is_a_syntax_error_at_its_offset.

#### INT-7: Schedule timings measured on every target and labelled honestly

- Priority P3, kind fix, effort M, golden impact none.
- Findings: gap-3-6.
- Depends on: nothing.
- Locations and files: `crates/ludenea-control/src/subscriptions.rs:25,270-305`, `crates/ludenea-control/src/protocol.rs:591-594`, `editor/packages/editor-builtins/src/explorer/Introspection.tsx:58-83`.
- Acceptance: The transport injects the host clock (Instant natively, performance.now passed by the worker), so timings exist on every target; only played ticks are timed, or replayed ones are labelled; the header says what is averaged, adds a total row and says run a tick to measure.
- First failing test: Host test with a fake clock advancing 5 microseconds per call: after one played tick micros is Some(5.0) on every target.

#### INT-8: One tick recorder shared by introspection and the divergence report

- Priority P3, kind deep-refactor, effort M, golden impact none.
- Findings: gap-3-9.
- Depends on: INT-2, BRN-1.
- Locations and files: `crates/ludenea-control/src/subscriptions.rs:209-359`, `crates/ludenea-control/src/divergence.rs:643-686`.
- Acceptance: One structured TickObservation (events with schema name, commands with system index and record, optional timings) is produced by one recorder and converted to the protocol only when building replies; introspection and the divergence report share it.
- First failing test: Unit test: for a tick where boxes spawns a crate, GetTickCommands and the divergence Cause::Command name the same system and entity.

### VPR: Viewport renderer and debug-draw API (grade C)

Subsystem: ludenea-debug-draw (mesh and wgpu renderer), viewport panel, game-facing debug-draw API.

**Problem.** Triangulation is GPU-free and tested and WebGL2 and WebGPU agree on a reference frame, but wgpu validation errors have no handler (WebGL2 aborts the renderer for good, WebGPU blanks the viewport) and device limits are ignored, the renderer is untested at every level and the end-to-end suite never checks a pixel, one NaN coordinate refuses the whole frame silently, every repaint re-parses and re-uploads everything (10,000 colliders pan at 12 fps), concave fills cover the wrong area, and debug layers are stringly typed and unfiltered.

**Target state.** Renderer errors are values shown with a retry; both backends are pixel-tested; invalid primitives are dropped with a located warning; geometry is retained and the camera is a uniform; layers are typed and filtered.

#### VPR-1: The renderer tested on WebGL2 and WebGPU with pixel checks

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: gap-4-2.
- Depends on: FND-1.
- Locations and files: `editor/e2e/playwright.config.ts:16-18`, `editor/e2e/renderer.spec.ts (new)`, `editor/e2e/smoke.spec.ts:53-57`, `.github/workflows/determinism.yml`.
- Acceptance: Playwright projects webgl2 and webgpu (the latter asserting an adapter exists so it cannot fall back silently); a renderer fixture page draws a reference frame and samples the background, a filled circle, an outline, a selected highlight and a concave notch; the smoke spec samples one live collider pixel; ludenea-debug-draw with render is linted for wasm32 in CI.
- First failing test: editor/e2e/renderer.spec.ts draws_the_reference_frame on each backend.

#### VPR-2: GPU errors captured, device limits respected, failures shown with a retry

- Priority P1, kind fix, effort M, golden impact none.
- Findings: gap-4-1, shell-11.
- Depends on: VPR-1.
- Locations and files: `crates/ludenea-debug-draw/src/render.rs:85-204`, `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:72-117,291-293`, `editor/packages/editor-builtins/test/fake-renderer.ts`.
- Acceptance: device.on_uncaptured_error stores errors that draw and resize return as Err naming the operation and the limit; a panic hook forwards messages; vertex data is split below max_buffer_size and the surface clamped to max_texture_dimension_2d; after repeated failures the panel stops and shows The viewport stopped rendering: <reason> (role=alert) with Retry; a lost device recreates the renderer once.
- First failing test: renderer.spec.ts: 58,300 circles then a one-circle frame both draw on both backends; viewport.test.tsx a_draw_that_keeps_failing_offers_a_retry.

#### VPR-3: Non-finite primitives dropped with a located warning

- Priority P2, kind fix, effort M, golden impact none.
- Findings: gap-4-3.
- Depends on: ERR-1.
- Locations and files: `crates/ludenea-control/src/compare.rs:197-207`, `crates/ludenea-control/src/worker.rs:98`, `crates/ludenea-debug-draw/src/render.rs:46,154`, `crates/ludenea-debug-draw/src/mesh.rs`.
- Acceptance: caught_draw (or Simulation::debug_draw) drops primitives with a non-finite coordinate, radius or point and emits one console warning per layer per tick range naming the layer, entity, primitive and field; mesh triangulation skips non-finite primitives too.
- First failing test: Host test: a drawer pushing a valid line and a circle with a NaN center yields a frame keeping the line and a warning naming the layer, circle and center.

#### VPR-4: Retained geometry; the camera is a uniform

- Priority P2, kind deep-refactor, effort L, golden impact none.
- Findings: gap-4-4.
- Depends on: VPR-2.
- Locations and files: `crates/ludenea-debug-draw/src/render.rs:148-231`, `crates/ludenea-debug-draw/src/mesh.rs:46-121`, `editor/packages/editor-builtins/src/viewport/ViewportPanel.tsx:100-124`, `editor/e2e/performance.spec.ts`.
- Acceptance: set_frame parses and triangulates once per frame in world space into a retained, growable buffer; draw only updates a camera uniform applied by the shader; lines keep a constant pixel width without re-triangulating; colors are Unorm8x4; a 10,000-collider bench pans with a 95th percentile frame time under 20 ms.
- First failing test: Performance spec panning_10000_drawn_colliders_keeps_p95_under_20_ms, and a fixture test that a camera-only change does not re-parse.

#### VPR-5: Concave fills triangulated correctly

- Priority P2, kind fix, effort S, golden impact none.
- Findings: gap-4-5.
- Depends on: nothing.
- Locations and files: `crates/ludenea-debug-draw/src/mesh.rs:96-101`, `crates/ludenea-debug-draw/src/lib.rs:35-39`.
- Acceptance: Filled polygons are triangulated by ear clipping (either winding, a fan for three points); Primitive::Polygon is documented as a simple polygon in world meters.
- First failing test: mesh.rs concave_fill_leaves_the_notch_empty.

#### VPR-6: Typed debug layers; frames filtered to the requested layers

- Priority P2, kind put-in-place, effort M, golden impact none.
- Findings: gap-4-7.
- Depends on: nothing.
- Locations and files: `crates/ludenea-debug-draw/src/lib.rs:15-104`, `crates/ludenea-app/src/app.rs:172-177`, `crates/ludenea-app/src/simulation.rs:185-192`, `crates/ludenea-physics/src/editor.rs`.
- Acceptance: app.debug_layer(name) returns a Copy handle used by push and layers.shows; Simulation::debug_draw filters frames to the requested layers; every Primitive field is documented (units, axes, fill, label sizing); the template draws a small game-owned layer documented in the README.
- First failing test: ludenea-app test: a drawer that ignores layers and pushes to a and b yields only a when only a is requested.

#### VPR-7: The ghost and the drag preview drawn by the GPU renderer

- Priority P3, kind deep-refactor, effort M, golden impact none.
- Findings: gap-4-6.
- Depends on: VPR-4.
- Locations and files: `editor/packages/editor-builtins/src/viewport/overlay.ts:63-97`, `editor/packages/editor-builtins/src/divergence/index.tsx:256-280`, `editor/packages/editor-builtins/src/viewport/camera.ts:42-73`, `crates/ludenea-debug-draw/src/render.rs`.
- Acceptance: A ghost pass draws DebugDrawFrame::ghost and a drag preview draws the dragged entity's primitives offset in the highlight color, both in the renderer; Canvas2D keeps labels and plugin overlays only; the remaining TypeScript primitive switches are exhaustive.
- First failing test: renderer.spec.ts: a ghost capsule and a current capsule at the same place cover the same outline pixels.

#### VPR-8: Labels follow the highlight and avoid clumping; Frame all

- Priority P3, kind fix, effort S, golden impact none.
- Findings: gap-4-8, ux-live-11.
- Depends on: nothing.
- Locations and files: `editor/packages/editor-builtins/src/viewport/overlay.ts:31-37`, `editor/packages/editor-builtins/src/viewport/index.tsx:20,64-83`, `editor/packages/editor-builtins/src/viewport/camera.ts`.
- Acceptance: Labels of selected entities use the highlight color; overlapping labels are skipped (or hidden below a zoom threshold); label sizing is documented; the camera fits the first frame of a newly opened document; a Frame all command has a shortcut.
- First failing test: viewport.test.tsx a_selected_entitys_label_uses_the_highlight_color and the_first_frame_is_framed.

### DSK: Desktop application (grade D)

Subsystem: editor/apps/desktop (Tauri backend and entry).

**Problem.** The backend is a minimal relay with a tight attack surface and careful rebuild handover, but it does not build for Windows (icon.ico is missing), has no test at any level, cannot replace a running game's executable on Windows (rebuilds fail, consoles flash), is never built or packaged in production form, loses startup diagnostics, and closes without storing the session.

**Target state.** A tested backend library green on three operating systems; games run from private copies; production bundles built in CI; startup failures and window closing handled.

#### DSK-1: The desktop app builds on Windows: full icon set

- Priority P0, kind fix, effort S, golden impact none.
- Findings: desktop-1.
- Depends on: nothing.
- Locations and files: `editor/apps/desktop/src-tauri/icons/`, `editor/apps/desktop/src-tauri/tauri.conf.json:18`.
- Acceptance: Icons generated with tauri icon from one source (icon.ico, icon.icns, PNG sizes) are committed and listed in bundle.icon; the desktop job is green on macos-14, ubuntu-24.04 and windows-2025 (locally through act while the repository has no remote).
- First failing test: n/a (tooling): the windows-2025 desktop clippy step fails today and must pass.

#### DSK-2: The desktop backend is a tested library

- Priority P1, kind put-in-place, effort M, golden impact none.
- Findings: desktop-3, delivery-2, test-strategy-3.
- Depends on: DSK-1.
- Locations and files: `editor/apps/desktop/src-tauri/src/main.rs:25-316`, `editor/apps/desktop/src-tauri/Cargo.toml:10-14`, `.github/workflows/determinism.yml:125-161`.
- Acceptance: Backend, the three commands and the plugin scheme handler move into a library (a lib target or a tauri-plugin-ludenea crate) with channels behind a two-method sink trait. Unit tests cover events before hello delivered once after welcome, stale drop_editor identifiers, lines from a replaced game dropped, plugin path refusals (.., a/b, C:x, missing project) and a control send with no game. Command tests use Tauri's mock runtime. cargo test --manifest-path editor/apps/desktop/src-tauri/Cargo.toml --locked runs in the three-OS job.
- First failing test: events_sent_before_hello_reach_the_editor_once_after_welcome.

#### DSK-3: Windows: games run from a private copy, without console windows, with plain paths

- Priority P1, kind fix, effort M, golden impact none.
- Findings: desktop-4, readiness-1.
- Depends on: FND-6.
- Locations and files: `crates/ludenea-project/src/service.rs:489-533`, `crates/ludenea-project/src/game.rs:36-43`, `crates/ludenea-project/src/build.rs:120-171`, `crates/ludenea-project/src/lib.rs`.
- Acceptance: Each native build's executable is copied to .ludenea/build/native/<hash>/<name> and started from there (current and previous copies kept); CREATE_NO_WINDOW is set on the game and cargo commands on Windows; canonical paths lose the verbatim prefix; the real-build tests run on windows-2025.
- First failing test: Ignored build test a_rebuild_while_the_game_runs_succeeds, plus service.rs the_running_game_runs_from_a_private_copy.

#### DSK-4: The production desktop build packaged in CI; cargo found outside PATH

- Priority P2, kind put-in-place, effort L, golden impact none.
- Findings: desktop-5, projects-14.
- Depends on: DSK-1.
- Locations and files: `editor/apps/desktop/src-tauri/tauri.conf.json:6-18`, `crates/ludenea-project/src/build.rs:77-120`, `crates/ludenea-project/src/plugins.rs:260-270`, `.github/workflows/determinism.yml:159-161`, `README.md:121-127`.
- Acceptance: bundle.active is true with app and dmg, nsis or msi, deb and appimage targets; CI runs pnpm -C editor/apps/desktop tauri build and uploads the bundles; cargo is resolved from PATH, CARGO, CARGO_HOME/bin and ~/.cargo/bin for builds and cargo metadata, failing with BuildError::CargoNotFound naming the places searched and rustup; the README documents installing and launching.
- First failing test: build.rs cargo_is_found_in_cargo_home_when_path_lacks_it.

#### DSK-5: Startup failures visible

- Priority P2, kind fix, effort S, golden impact none.
- Findings: desktop-6, errors-11.
- Depends on: DSK-2.
- Locations and files: `editor/apps/desktop/src-tauri/src/main.rs:135-142,283-315`, `editor/apps/web/src/main.tsx:15-18`, `editor/apps/desktop/src/main.tsx:15-22`, `editor/packages/control-client/src/client.ts:177-181`.
- Acceptance: A failed command-line open is replayed as a warning at hello; web and desktop tell a version mismatch apart from a connection failure and name both versions and the rebuild command; VersionMismatch texts end with the fix; a window that cannot be created is reported in a native message box and on stderr with exit code 1.
- First failing test: Backend test a_project_that_cannot_open_is_reported_at_hello, and a web test a_version_mismatch_names_both_versions_and_the_rebuild_command.

#### DSK-6: Closing the window stores the session first

- Priority P1, kind fix, effort M, golden impact none.
- Findings: desktop-8.
- Depends on: DSK-2.
- Locations and files: `editor/apps/desktop/src-tauri/src/main.rs:301-315`, `editor/apps/web/src/session-guard.ts:128-135`, `editor/apps/web/src/app.ts:72`.
- Acceptance: CloseRequested is intercepted; the front end awaits guard.flush() and then calls a close command; the backend closes on that command or after 3 seconds; RunEvent::Exit drops the ProjectService so the game is stopped.
- First failing test: Desktop entry vitest closing_the_desktop_window_stores_the_session_before_it_closes.

#### DSK-7: Desktop conveniences: folder picker, remembered window, project title

- Priority P3, kind put-in-place, effort M, golden impact none.
- Findings: desktop-10.
- Depends on: DSK-2.
- Locations and files: `editor/packages/editor-builtins/src/project/index.tsx:194-222`, `editor/apps/desktop/src-tauri/tauri.conf.json:13`, `editor/apps/desktop/src-tauri/capabilities/default.json`.
- Acceptance: An optional EditorHost.pickFolder is implemented on desktop with tauri-plugin-dialog (only dialog:allow-open granted) and a Browse button appears where the host has it; the window state is remembered with a minimum size; the title shows the project name before Ludenea.
- First failing test: Project plugin test open_project_offers_a_folder_picker_when_the_host_has_one.

#### DSK-8: The plugin origin comes from convertFileSrc

- Priority P3, kind fix, effort S, golden impact none.
- Findings: desktop-12.
- Depends on: DSK-2.
- Locations and files: `editor/apps/desktop/src/main.tsx:5-7`, `editor/apps/desktop/src-tauri/tauri.conf.json:15`, `editor/apps/desktop/src-tauri/src/main.rs:227-230`.
- Acceptance: Bundles are served at <origin>/<file> and their URLs built with convertFileSrc(file, ludenea-plugin); no user-agent sniffing; the CSP lists only the forms Tauri produces.
- First failing test: Desktop unit test plugin_url_uses_the_origin_tauri_reports.

## Strengths to preserve

What the auditors judged solid. Changes in these areas should keep these properties and lean on the tests that prove them.

- Determinism core: math uses only exactly rounded operations and sqrt; clippy.toml bans transcendental functions, min and max, mul_add, clocks and hash collections; the hash serializer is canonical by construction (fixed-width little-endian, canonical NaN) with a pinned value; Parallel works in fixed chunks with results in item order; graph coloring and gather-then-scatter make the solve independent of thread count (proven at 1 to 16 threads, including a color larger than two chunks). Do not trade these for speed.
- Goldens: a 10,001-hash trace and a replay with prefab, data asset and edits verified on macOS, Linux and Windows at 1, 2 and 8 threads in release, in debug inside cargo test, under wasmtime and in Chromium; an editor session replays hash for hash in the native command line tool; verify re-runs to tell nondeterminism from a platform change.
- Decoders and restore: Snapshot, Replay and EditLog read their version first and reject trailing bytes; World::restore validates allocator state and liveness before mutating; replay decoding checks every input and edit before playing; tens of thousands of fuzzed snapshots, traces, scenes, replays and protocol batches produced no panic other than the thread count.
- Type-level guards: the Schema derive refuses shape-changing serde attributes and generic types with spanned errors; HashMap and HashSet have no Schema; derive(Query) checks read-only access syntactically and at type level; unsafe code is confined to query.rs with SAFETY arguments; access conflicts are refused at build with positions and a fix.
- Scenes and prefabs: loading is transactional; local identifiers are never reused and stale references survive; override text splicing keeps formatting and comments byte for byte (4,000 fuzzed edits, no panic); prefab cycles are detected with their chain; data asset errors give the file's line and column.
- Sessions and branches: each branch stores only its own segment and checkpoints are Arc-shared across branches; checkpoint spacing is logarithmic with tested seek bounds; game panics during step, seek, resimulation, restore, compare, divergence and picking are isolated per tick; undo history migrations from earlier versions are tested; the divergence report is incremental and pays nothing unless requested.
- Control protocol: the envelope and handshake survive invalid JSON, non-UTF-8 lines and unknown variants; the wire format is pinned by a golden JSON test and an exhaustive error-code list; 64-bit integers travel as decimal strings; generated TypeScript bindings are regenerated from scratch in CI; work per advance is bounded (resimulation, ghost, report, resource pushes, pacer) as back-pressure.
- Projects and serving: path confinement through symlinks with O_NOFOLLOW; crash-safe atomic writes; a watcher that does not trust event kinds; content-hashed wasm builds that keep the previous game; a careful rebuild handover; late-joining editors catch up without missed or repeated events; ludenea serve checks Host against DNS rebinding and Origin against cross-site WebSocket hijacking.
- Editor architecture: the plugin loader charges registrations to their plugin, rolls back a failed activate, swaps a reload in one step and restores the previous plugin on failure; one React instance across plugins, checked end to end; plugins bundle without Node.js with one failing plugin never blocking others; the control client correlates by id and reference-counts topic subscriptions; browser crash recovery restarts the worker with the latest session and a crash budget.
- Editor quality signals: strict TypeScript with noUncheckedIndexedAccess and no any; about 90 percent of editor lines covered by behaviour-named tests; accessibility roles throughout with no unlabelled control in the live audit; virtualized entity lists measured at 60 fps on 100,000 entities in the scene tree.

## Dropped findings

- **projects-13**: deliberate, documented and tested design (console.test.tsx:50-54 asserts that a source location is copied for the user's own editor); the spec's clickable location is met; opening an external editor is a feature request, not a defect.
- **delivery-5**: choosing a licence is the owner's decision, not an engineering defect, and the repository has no remote yet; the changelog part is covered by VER-1 and the security notes for ludenea serve by PRJ-6 and README work.
- **gap-3-5**: attributing events to systems and a FindTicks search go beyond the 2a introspection design; the structured command records it asks for come with INT-8.
- **gap-3-10**: the Schedule tab meets its specification (systems in run order with their accesses); exposing ordering constraints is a feature extension with a protocol bump, and the real ordering trap (an event reader before every writer) is fixed at build time by ECS-7.

## Finding index

Every audit finding and the work item that covers it.

| Finding | Item |
| --- | --- |
| ambition-1 | SHL-1 |
| ambition-2 | REP-3 |
| ambition-3 | PLG-3 |
| ambition-4 | ERR-1 |
| ambition-5 | AUT-4 |
| ambition-6 | PRJ-9 |
| ambition-7 | PLG-4 |
| ambition-8 | PLG-3 |
| ambition-9 | VER-12 |
| ambition-10 | ERR-2 |
| ambition-11 | PHY-6 |
| ambition-13 | SHL-19 |
| branches-1 | BRN-1 |
| branches-2 | BRN-2 |
| branches-3 | BRN-3 |
| branches-4 | BRN-4 |
| branches-5 | SES-1 |
| branches-6 | VER-6 |
| branches-7 | BRN-5 |
| branches-8 | BRN-6 |
| branches-9 | REP-7 |
| branches-10 | BRN-7 |
| branches-11 | ERR-1 |
| branches-12 | BRN-8 |
| branches-13 | SES-12 |
| branches-14 | SES-9 |
| branches-15 | BRN-9 |
| branches-16 | BRN-10 |
| branches-17 | SES-5 |
| branches-19 | PLG-3 |
| branches-20 | BRN-11 |
| delivery-1 | FND-4 |
| delivery-2 | DSK-2 |
| delivery-3 | VER-8 |
| delivery-4 | PRJ-8 |
| delivery-5 | dropped |
| delivery-6 | PRJ-8 |
| delivery-7 | FND-1 |
| delivery-8 | DET-6 |
| delivery-9 | FND-6 |
| delivery-10 | FND-10 |
| delivery-11 | QA-6 |
| delivery-12 | FND-1 |
| delivery-13 | FND-11 |
| delivery-14 | FND-7 |
| delivery-15 | REP-2 |
| delivery-16 | QA-5 |
| delivery-17 | FND-12 |
| delivery-18 | FND-9 |
| delivery-19 | FND-3 |
| delivery-20 | FND-11 |
| delivery-21 | QA-6 |
| desktop-1 | DSK-1 |
| desktop-2 | CTL-1 |
| desktop-3 | DSK-2 |
| desktop-4 | DSK-3 |
| desktop-5 | DSK-4 |
| desktop-6 | DSK-5 |
| desktop-7 | CTL-3 |
| desktop-8 | DSK-6 |
| desktop-9 | CTL-6 |
| desktop-10 | DSK-7 |
| desktop-11 | PRJ-13 |
| desktop-12 | DSK-8 |
| determinism-1 | FND-3 |
| determinism-2 | RUN-1 |
| determinism-3 | DET-5 |
| determinism-4 | REP-3 |
| determinism-5 | DET-6 |
| determinism-6 | PRJ-8 |
| determinism-7 | REP-2 |
| determinism-8 | VER-2 |
| determinism-9 | FND-3 |
| determinism-10 | FND-5 |
| determinism-11 | FND-3 |
| dx-live-1 | ECS-2 |
| dx-live-2 | SCN-8 |
| dx-live-3 | ECS-7 |
| dx-live-4 | ECS-4 |
| dx-live-5 | PHY-5 |
| dx-live-6 | REP-3 |
| dx-live-7 | PLG-3 |
| dx-live-8 | ECS-8 |
| dx-live-9 | ECS-13 |
| dx-live-10 | ECS-16 |
| dx-live-11 | RUN-2 |
| dx-live-12 | ECS-4 |
| dx-live-13 | PRJ-9 |
| dx-live-14 | PLG-8 |
| dx-live-15 | VER-10 |
| dx-live-16 | ERR-5 |
| dx-live-17 | RUN-1 |
| dx-live-18 | FND-13 |
| dx-live-19 | RUN-9 |
| dx-live-20 | PRJ-8 |
| dx-live-21 | PRJ-12 |
| dx-live-22 | PRJ-6 |
| dx-live-23 | ECS-12 |
| ecs-1 | DET-4 |
| ecs-2 | ECS-4 |
| ecs-3 | ECS-3 |
| ecs-4 | ECS-10 |
| ecs-5 | ECS-8 |
| ecs-6 | ECS-7 |
| ecs-7 | ECS-5 |
| ecs-8 | ECS-1 |
| ecs-9 | ECS-12 |
| ecs-10 | ECS-11 |
| ecs-11 | ECS-15 |
| ecs-12 | ECS-6 |
| ecs-13 | ECS-14 |
| ecs-14 | ECS-9 |
| ecs-15 | DET-2 |
| ecs-16 | DET-3 |
| ecs-17 | FND-13 |
| ecs-18 | ECS-16 |
| ecs-19 | ECS-17 |
| ecs-20 | ECS-18 |
| errors-1 | ERR-2 |
| errors-2 | ERR-3 |
| errors-3 | ERR-1 |
| errors-4 | PLG-1 |
| errors-5 | RUN-1 |
| errors-6 | VER-4 |
| errors-7 | ERR-5 |
| errors-8 | ERR-4 |
| errors-9 | PRJ-5 |
| errors-10 | ERR-6 |
| errors-11 | DSK-5 |
| errors-12 | RUN-2 |
| errors-13 | ECS-9 |
| gap-1-editor-scene-authoring-1 | AUT-1 |
| gap-1-editor-scene-authoring-2 | AUT-3 |
| gap-1-editor-scene-authoring-3 | AUT-8 |
| gap-1-editor-scene-authoring-4 | AUT-2 |
| gap-1-editor-scene-authoring-5 | AUT-7 |
| gap-1-editor-scene-authoring-6 | AUT-5 |
| gap-1-editor-scene-authoring-7 | AUT-6 |
| gap-1-editor-scene-authoring-8 | SHL-11 |
| gap-1-editor-scene-authoring-9 | AUT-4 |
| gap-1-editor-scene-authoring-10 | PLG-3 |
| gap-1-editor-scene-authoring-11 | SES-14 |
| gap-1-editor-scene-authoring-12 | SES-13 |
| gap-1-editor-scene-authoring-13 | AUT-9 |
| gap-1-editor-scene-authoring-14 | SHL-12 |
| gap-1-editor-scene-authoring-15 | SHL-2 |
| gap-2-outside-editor-file-ops-1 | AST-1 |
| gap-2-outside-editor-file-ops-2 | AST-3 |
| gap-2-outside-editor-file-ops-3 | AST-4 |
| gap-2-outside-editor-file-ops-4 | AST-2 |
| gap-2-outside-editor-file-ops-5 | AST-7 |
| gap-2-outside-editor-file-ops-6 | AST-5 |
| gap-2-outside-editor-file-ops-7 | AST-6 |
| gap-2-outside-editor-file-ops-8 | AST-8 |
| gap-2-outside-editor-file-ops-9 | SES-14 |
| gap-3-introspection-explorer-1 | INT-1 |
| gap-3-introspection-explorer-2 | INT-2 |
| gap-3-introspection-explorer-3 | INT-4 |
| gap-3-introspection-explorer-4 | INT-3 |
| gap-3-introspection-explorer-5 | dropped |
| gap-3-introspection-explorer-6 | INT-7 |
| gap-3-introspection-explorer-7 | INT-6 |
| gap-3-introspection-explorer-8 | INT-5 |
| gap-3-introspection-explorer-9 | INT-8 |
| gap-3-introspection-explorer-10 | dropped |
| gap-4-viewport-renderer-1 | VPR-2 |
| gap-4-viewport-renderer-2 | VPR-1 |
| gap-4-viewport-renderer-3 | VPR-3 |
| gap-4-viewport-renderer-4 | VPR-4 |
| gap-4-viewport-renderer-5 | VPR-5 |
| gap-4-viewport-renderer-6 | VPR-7 |
| gap-4-viewport-renderer-7 | VPR-6 |
| gap-4-viewport-renderer-8 | VPR-8 |
| gap-5-engine-scaling-1 | ECS-19 |
| gap-5-engine-scaling-3 | INT-5 |
| gap-5-engine-scaling-4 | BRN-3 |
| gap-5-engine-scaling-5 | PHY-14 |
| gap-5-engine-scaling-7 | PHY-13 |
| gap-5-engine-scaling-8 | SES-8 |
| gap-5-engine-scaling-9 | ECS-20 |
| physics-1 | PHY-1 |
| physics-2 | PHY-2 |
| physics-3 | PHY-16 |
| physics-4 | PHY-4 |
| physics-5 | PHY-5 |
| physics-6 | PHY-7 |
| physics-7 | PHY-3 |
| physics-8 | PHY-11 |
| physics-9 | PHY-12 |
| physics-10 | PHY-17 |
| physics-11 | PHY-7 |
| physics-12 | PHY-6 |
| physics-13 | PHY-15 |
| physics-14 | PHY-9 |
| physics-15 | PHY-10 |
| physics-16 | FND-14 |
| physics-17 | PHY-13 |
| plugins-1 | PLG-1 |
| plugins-2 | CTL-4 |
| plugins-3 | PLG-5 |
| plugins-4 | PLG-3 |
| plugins-5 | PLG-2 |
| plugins-6 | PLG-6 |
| plugins-7 | VER-9 |
| plugins-8 | PLG-7 |
| plugins-9 | PLG-8 |
| plugins-10 | ECS-14 |
| plugins-11 | PRJ-9 |
| plugins-12 | PLG-9 |
| plugins-13 | ERR-1 |
| plugins-14 | PLG-10 |
| plugins-15 | PHY-6 |
| plugins-16 | PLG-11 |
| plugins-17 | PLG-2 |
| plugins-18 | PLG-12 |
| plugins-19 | PLG-13 |
| plugins-20 | PLG-14 |
| projects-1 | PRJ-1 |
| projects-2 | PRJ-2 |
| projects-3 | PRJ-4 |
| projects-4 | PRJ-3 |
| projects-5 | CTL-5 |
| projects-6 | PRJ-5 |
| projects-7 | PRJ-6 |
| projects-8 | PRJ-7 |
| projects-9 | RUN-9 |
| projects-10 | PRJ-8 |
| projects-11 | FND-12 |
| projects-12 | CTL-1 |
| projects-13 | dropped |
| projects-14 | DSK-4 |
| projects-15 | PRJ-11 |
| projects-16 | PRJ-13 |
| projects-17 | PRJ-14 |
| projects-18 | PRJ-12 |
| projects-19 | ERR-6 |
| projects-20 | CTL-7 |
| projects-21 | PRJ-15 |
| projects-22 | PRJ-9 |
| protocol-1 | RUN-1 |
| protocol-2 | RUN-2 |
| protocol-3 | CTL-1 |
| protocol-4 | CTL-2 |
| protocol-5 | CTL-3 |
| protocol-6 | CTL-4 |
| protocol-7 | CTL-5 |
| protocol-8 | VER-3 |
| protocol-9 | ERR-2 |
| protocol-10 | ERR-6 |
| protocol-11 | CTL-7 |
| protocol-12 | VER-3 |
| protocol-13 | CTL-6 |
| protocol-14 | CTL-8 |
| protocol-15 | CTL-10 |
| protocol-16 | RUN-5 |
| protocol-17 | CTL-11 |
| readiness-1 | DSK-3 |
| readiness-2 | SES-11 |
| readiness-3 | CTL-2 |
| readiness-4 | SES-10 |
| readiness-5 | SES-12 |
| readiness-6 | RUN-8 |
| readiness-7 | CTL-5 |
| readiness-8 | RUN-13 |
| readiness-9 | REP-2 |
| readiness-10 | AST-2 |
| readiness-11 | PRJ-11 |
| readiness-12 | CTL-3 |
| readiness-13 | VER-5 |
| readiness-14 | AST-10 |
| readiness-15 | VER-8 |
| readiness-16 | AST-10 |
| readiness-17 | PRJ-17 |
| replays-1 | REP-3 |
| replays-2 | RUN-1 |
| replays-3 | REP-1 |
| replays-4 | ERR-4 |
| replays-5 | VER-2 |
| replays-6 | VER-8 |
| replays-7 | REP-3 |
| replays-8 | REP-5 |
| replays-9 | REP-4 |
| replays-10 | RUN-2 |
| replays-11 | ERR-5 |
| replays-12 | REP-6 |
| replays-13 | FND-9 |
| replays-14 | REP-8 |
| runtime-1 | SCN-8 |
| runtime-2 | RUN-3 |
| runtime-3 | RUN-2 |
| runtime-4 | RUN-1 |
| runtime-5 | RUN-4 |
| runtime-6 | RUN-5 |
| runtime-7 | ERR-4 |
| runtime-8 | RUN-7 |
| runtime-9 | RUN-8 |
| runtime-10 | RUN-9 |
| runtime-11 | RUN-6 |
| runtime-12 | RUN-10 |
| runtime-13 | RUN-11 |
| runtime-14 | RUN-12 |
| runtime-15 | FND-3 |
| runtime-16 | SCN-8 |
| rust-quality-1 | ECS-6 |
| rust-quality-2 | RUN-2 |
| rust-quality-3 | RUN-1 |
| rust-quality-4 | SES-9 |
| rust-quality-5 | CTL-8 |
| rust-quality-6 | AST-1 |
| rust-quality-7 | RUN-7 |
| rust-quality-8 | VER-6 |
| rust-quality-9 | ECS-15 |
| rust-quality-10 | FND-13 |
| rust-quality-11 | FND-14 |
| rust-quality-12 | SCN-14 |
| rust-quality-13 | CTL-9 |
| rust-quality-14 | ECS-18 |
| rust-quality-15 | FND-3 |
| scenes-1 | SCN-1 |
| scenes-2 | SCN-2 |
| scenes-3 | VER-4 |
| scenes-4 | VER-4 |
| scenes-5 | SCN-5 |
| scenes-6 | SCN-4 |
| scenes-7 | SCN-10 |
| scenes-8 | AST-9 |
| scenes-9 | SCN-1 |
| scenes-10 | ERR-3 |
| scenes-11 | ERR-2 |
| scenes-12 | SCN-6 |
| scenes-13 | ERR-5 |
| scenes-14 | SCN-7 |
| scenes-15 | SCN-12 |
| scenes-16 | SCN-9 |
| scenes-17 | SCN-11 |
| scenes-18 | SCN-13 |
| scenes-19 | QA-5 |
| scenes-20 | SCN-14 |
| scenes-21 | VER-2 |
| scenes-22 | FND-12 |
| scenes-23 | SCN-3 |
| scenes-24 | SCN-15 |
| sessions-1 | SES-1 |
| sessions-2 | SES-2 |
| sessions-3 | SES-1 |
| sessions-4 | SES-3 |
| sessions-5 | SES-4 |
| sessions-6 | SES-5 |
| sessions-7 | PHY-5 |
| sessions-8 | ERR-1 |
| sessions-9 | SES-8 |
| sessions-10 | SES-6 |
| sessions-11 | SES-7 |
| sessions-12 | VER-5 |
| sessions-13 | SHL-15 |
| sessions-14 | SHL-17 |
| sessions-15 | PLG-3 |
| sessions-16 | SHL-16 |
| sessions-17 | QA-2 |
| sessions-18 | SES-9 |
| sessions-19 | SES-10 |
| sessions-20 | VER-7 |
| sessions-21 | SHL-18 |
| shell-1 | SHL-1 |
| shell-2 | ERR-1 |
| shell-3 | PLG-3 |
| shell-4 | CTL-5 |
| shell-5 | SHL-7 |
| shell-6 | SHL-2 |
| shell-7 | SHL-3 |
| shell-8 | SHL-4 |
| shell-9 | SHL-6 |
| shell-10 | SHL-5 |
| shell-11 | VPR-2 |
| shell-12 | VER-5 |
| shell-13 | PLG-10 |
| shell-14 | SHL-8 |
| shell-15 | SHL-6 |
| shell-16 | SHL-9 |
| shell-17 | SHL-10 |
| shell-18 | SHL-11 |
| shell-19 | SHL-12 |
| shell-20 | SHL-13 |
| test-strategy-1 | DET-1 |
| test-strategy-2 | RUN-1 |
| test-strategy-3 | DSK-2 |
| test-strategy-4 | ECS-10 |
| test-strategy-5 | QA-1 |
| test-strategy-6 | FND-10 |
| test-strategy-7 | FND-6 |
| test-strategy-8 | PRJ-10 |
| test-strategy-9 | QA-4 |
| test-strategy-10 | QA-7 |
| test-strategy-11 | QA-9 |
| test-strategy-12 | QA-5 |
| test-strategy-13 | QA-3 |
| test-strategy-14 | PHY-8 |
| test-strategy-15 | PRJ-16 |
| test-strategy-16 | FND-7 |
| test-strategy-17 | FND-8 |
| test-strategy-18 | QA-6 |
| test-strategy-19 | QA-8 |
| ts-quality-1 | PLG-1 |
| ts-quality-2 | VER-5 |
| ts-quality-3 | PLG-3 |
| ts-quality-4 | PLG-2 |
| ts-quality-5 | ERR-1 |
| ts-quality-6 | PHY-6 |
| ts-quality-7 | ERR-2 |
| ts-quality-8 | VER-3 |
| ts-quality-9 | SHL-10 |
| ts-quality-10 | PRJ-14 |
| ts-quality-11 | QA-4 |
| ts-quality-12 | FND-2 |
| ts-quality-13 | SHL-5 |
| ts-quality-14 | CTL-2 |
| ts-quality-15 | SHL-3 |
| ts-quality-16 | PLG-10 |
| ts-quality-17 | SHL-16 |
| ts-quality-18 | PLG-8 |
| ts-quality-19 | VER-9 |
| ts-quality-20 | SHL-13 |
| untrusted-input-1 | RUN-1 |
| untrusted-input-2 | SCN-3 |
| untrusted-input-3 | CTL-3 |
| untrusted-input-4 | SES-3 |
| untrusted-input-5 | VER-5 |
| untrusted-input-6 | VER-2 |
| untrusted-input-7 | ERR-4 |
| untrusted-input-8 | CTL-1 |
| untrusted-input-9 | RUN-6 |
| untrusted-input-10 | QA-1 |
| untrusted-input-11 | AST-6 |
| ux-live-1 | SHL-2 |
| ux-live-2 | ERR-1 |
| ux-live-3 | SHL-14 |
| ux-live-4 | PHY-5 |
| ux-live-5 | SCN-9 |
| ux-live-6 | SCN-1 |
| ux-live-7 | PHY-6 |
| ux-live-8 | SES-14 |
| ux-live-9 | SHL-3 |
| ux-live-10 | SHL-4 |
| ux-live-11 | VPR-8 |
| ux-live-12 | SES-5 |
| ux-live-13 | BRN-9 |
| ux-live-14 | BRN-6 |
| ux-live-15 | SHL-19 |
| ux-live-16 | REP-6 |
| ux-live-17 | SHL-15 |
| ux-live-18 | INT-1 |
| ux-live-19 | SCN-15 |
| ux-live-20 | SHL-7 |
| ux-live-21 | SHL-19 |
| ux-live-22 | SHL-15 |
| ux-live-23 | PHY-6 |
| ux-live-24 | SHL-19 |
| versioning-1 | VER-5 |
| versioning-2 | VER-4 |
| versioning-3 | VER-8 |
| versioning-4 | VER-2 |
| versioning-5 | VER-2 |
| versioning-6 | VER-7 |
| versioning-7 | PRJ-8 |
| versioning-8 | VER-9 |
| versioning-9 | VER-1 |
| versioning-10 | VER-3 |
| versioning-11 | RUN-5 |
| versioning-12 | VER-11 |

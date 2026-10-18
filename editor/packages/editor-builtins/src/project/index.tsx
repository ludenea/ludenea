import type { ProjectInfo } from "@ludenea/control-client";
import { definePlugin, type Editor } from "@ludenea/editor-shell";
import { useState } from "react";
import { createStore, errorText, shared, showPanel, useStore, type Store } from "../shared";

const PANEL = "ludenea.project";
const ENGINE_PATH = "ludenea.engine-path";
const RECENT = "ludenea.recent-projects";
const RECENT_LIMIT = 10;

type Form = "new" | "open" | null;
type Ui = Store<{ form: Form; error: string | null }>;

export const projectPlugin = definePlugin({
  name: "ludenea.project",
  activate(editor) {
    const { project } = shared(editor);
    const ui: Ui = createStore({ form: null as Form, error: null as string | null });
    editor.settings.register({ id: ENGINE_PATH, title: "Engine path", default: "", kind: "path" });
    editor.settings.register<string[]>({ id: RECENT, title: "Recent projects", default: [], kind: "list" });

    const opened = (info: ProjectInfo) => {
      project.set({ info });
      const recent = editor.settings.get<string[]>(RECENT).filter((root) => root !== info.root);
      editor.settings.set(RECENT, [info.root, ...recent].slice(0, RECENT_LIMIT));
      ui.set({ form: null, error: null });
    };
    const attempt = (request: Promise<ProjectInfo>) =>
      request.then(opened, (error) => ui.set({ error: errorText(error) }));
    const actions = {
      create: (name: string, parent: string, enginePath: string) =>
        attempt(editor.project.request({ type: "create", name, parent, engine_path: enginePath || null })),
      open: (path: string) => attempt(editor.project.request({ type: "open", path })),
    };

    // The service may already have a project open (`ludenea serve <project>`).
    editor.project.request({ type: "info" }).then(
      (info) => project.set({ info }),
      () => {},
    );
    const stopBuild = editor.project.on("build", ({ event }) => {
      const { build } = project.get();
      if (event.type === "progress") project.set({ build: { status: "building", done: event.done, total: event.total } });
      else if (event.type === "finished") project.set({ build: { ...build, status: event.ok ? "succeeded" : "failed" } });
      else if (build.status !== "building") project.set({ build: { status: "building", done: 0, total: null } });
    });

    const showForm = (form: Form) => {
      ui.set({ form, error: null });
      showPanel(editor, PANEL);
    };
    editor.commands.register({ id: "project.new", title: "New project", run: () => showForm("new") });
    editor.commands.register({ id: "project.open", title: "Open project", run: () => showForm("open") });
    editor.commands.register({
      id: "project.build",
      title: "Build project",
      async run() {
        const { build } = project.get();
        project.set({ build: { status: "building", done: 0, total: null } });
        try {
          await editor.project.request({ type: "build" });
        } catch (error) {
          project.set({ build: { ...build, status: "failed" } });
          throw error;
        }
      },
    });
    editor.panels.register({
      id: PANEL,
      title: "Project",
      defaultPosition: "left",
      component: () => <ProjectPanel editor={editor} ui={ui} actions={actions} />,
    });
    return stopBuild;
  },
});

type Actions = { create(name: string, parent: string, enginePath: string): void; open(path: string): void };

function ProjectPanel({ editor, ui, actions }: { editor: Editor; ui: Ui; actions: Actions }) {
  const { info, build } = useStore(shared(editor).project);
  const { form, error } = useStore(ui);
  const run = (command: string) => () =>
    void editor.commands.run(command).catch((failure) => ui.set({ error: errorText(failure) }));

  return (
    <div className="ludenea-project">
      <h2>{info ? info.name : "No project open"}</h2>
      {info && <p className="ludenea-muted ludenea-mono">{info.root}</p>}
      <BuildStatus build={build} />
      <div className="ludenea-toolbar">
        <button onClick={run("project.new")}>New project</button>
        <button onClick={run("project.open")}>Open project</button>
        <button onClick={run("project.build")} disabled={!info}>
          Build
        </button>
      </div>
      {error && <p className="ludenea-error">{error}</p>}
      {form === "new" && (
        <NewProjectForm
          enginePath={editor.settings.get<string>(ENGINE_PATH)}
          onCreate={actions.create}
          onCancel={() => ui.set({ form: null })}
        />
      )}
      {form === "open" && (
        <OpenProjectForm recent={editor.settings.get<string[]>(RECENT)} onOpen={actions.open} onCancel={() => ui.set({ form: null })} />
      )}
    </div>
  );
}

function BuildStatus({ build }: { build: { status: string; done: number; total: number | null } }) {
  if (build.status === "building") {
    const count = build.total === null ? `${build.done}` : `${build.done} of ${build.total}`;
    return (
      <p>
        Building ({count} crates) <progress value={build.done} max={build.total ?? undefined} aria-valuenow={build.done} />
      </p>
    );
  }
  if (build.status === "failed") return <p className="ludenea-error">Build failed: see the console.</p>;
  return null;
}

function NewProjectForm(props: { enginePath: string; onCreate: Actions["create"]; onCancel(): void }) {
  const [name, setName] = useState("");
  const [parent, setParent] = useState("");
  const [enginePath, setEnginePath] = useState(props.enginePath);
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        props.onCreate(name, parent, enginePath);
      }}
    >
      <label>
        Name <input autoFocus value={name} onChange={(event) => setName(event.target.value)} />
      </label>
      <label>
        Parent folder <input value={parent} onChange={(event) => setParent(event.target.value)} />
      </label>
      <label>
        Engine path <input value={enginePath} onChange={(event) => setEnginePath(event.target.value)} />
      </label>
      <button type="submit">Create</button>
      <button type="button" onClick={props.onCancel}>
        Cancel
      </button>
    </form>
  );
}

function OpenProjectForm(props: { recent: string[]; onOpen: Actions["open"]; onCancel(): void }) {
  const [path, setPath] = useState("");
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        props.onOpen(path);
      }}
    >
      <label>
        Project folder <input autoFocus value={path} onChange={(event) => setPath(event.target.value)} />
      </label>
      <button type="submit">Open</button>
      <button type="button" onClick={props.onCancel}>
        Cancel
      </button>
      {props.recent.length > 0 && (
        <ul aria-label="Recent projects">
          {props.recent.map((root) => (
            <li key={root}>
              <button type="button" onClick={() => props.onOpen(root)}>
                {root}
              </button>
            </li>
          ))}
        </ul>
      )}
    </form>
  );
}

import type {
  AssetInspector,
  CommandDefinition,
  Disposable,
  ExplorerColumn,
  FieldDecoration,
  InspectorComponent,
  PanelDefinition,
  SettingDefinition,
  ViewportOverlay,
  ViewportTool,
} from "./plugin";

/** Thrown when an id (or shortcut key) is registered twice; names both owners. */
export class DuplicateRegistration extends Error {
  constructor(kind: string, id: string, existing: string, incoming: string) {
    super(`${kind} "${id}" is already registered by "${existing}"; "${incoming}" cannot register it again`);
    this.name = "DuplicateRegistration";
  }
}

/** Registrations of one kind, keyed by id, observable for the shell's UI. */
export class Collection<T> {
  private readonly items = new Map<string, { value: T; owner: string }>();
  private readonly listeners = new Set<() => void>();
  private snapshot: T[] = [];

  constructor(
    private readonly kind: string,
    private readonly registry: Registry,
  ) {}

  register(id: string, value: T): Disposable {
    const owner = this.registry.owner();
    const existing = this.items.get(id);
    if (existing) throw new DuplicateRegistration(this.kind, id, existing.owner, owner);
    this.items.set(id, { value, owner });
    this.changed();
    return this.registry.own(() => {
      if (this.items.get(id)?.value !== value) return;
      this.items.delete(id);
      this.changed();
    });
  }

  get(id: string): T | undefined {
    return this.items.get(id)?.value;
  }

  /** Who registered `value`; undefined when it is not registered here. */
  ownerOf(value: unknown): string | undefined {
    for (const item of this.items.values()) if (item.value === value) return item.owner;
    return undefined;
  }

  /** Every value in registration order; the same array until the next change. */
  values = (): T[] => this.snapshot;

  /** Every `[id, value]` pair in registration order. */
  entries(): [string, T][] {
    return [...this.items].map(([id, item]) => [id, item.value]);
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  private changed(): void {
    this.snapshot = [...this.items.values()].map((item) => item.value);
    for (const listener of [...this.listeners]) listener();
  }
}

/** What a plugin's `activate` registered, disposed together on deactivation. */
export interface Activation {
  name: string;
  disposables: Disposable[];
}

/**
 * Every plugin gets the same `Editor`, so ownership comes from the activation in progress:
 * registrations made while `activate` runs belong to that plugin. Later ones belong to
 * "editor" and live until disposed by hand.
 */
export class Registry {
  readonly panels: Collection<PanelDefinition> = new Collection("panel", this);
  readonly commands: Collection<CommandDefinition> = new Collection("command", this);
  readonly inspectors: Collection<InspectorComponent<unknown>> = new Collection("inspector", this);
  /** By data type name, or `"Texture"`. */
  readonly assetInspectors: Collection<AssetInspector> = new Collection("asset inspector", this);
  /** Normalized key to command id. */
  readonly shortcuts: Collection<string> = new Collection("shortcut", this);
  readonly overlays: Collection<ViewportOverlay> = new Collection("overlay", this);
  readonly tools: Collection<ViewportTool> = new Collection("viewport tool", this);
  readonly settings: Collection<SettingDefinition<unknown>> = new Collection("setting", this);
  readonly decorations: Collection<FieldDecoration> = new Collection("decoration", this);
  readonly columns: Collection<ExplorerColumn> = new Collection("column", this);
  private activation: Activation | null = null;

  owner(): string {
    return this.activation?.name ?? "editor";
  }

  /** Who registered `value`, of any kind but a shortcut; undefined when it is not registered. */
  ownerOf(value: object): string | undefined {
    const kinds = [this.panels, this.commands, this.inspectors, this.assetInspectors, this.overlays, this.tools, this.settings, this.decorations, this.columns];
    for (const kind of kinds) {
      const owner = kind.ownerOf(value);
      if (owner !== undefined) return owner;
    }
    return undefined;
  }

  /** Wraps `dispose` once-only and charges it to the activating plugin. */
  own(dispose: () => void): Disposable {
    let disposed = false;
    const disposable = {
      dispose() {
        if (disposed) return;
        disposed = true;
        dispose();
      },
    };
    this.activation?.disposables.push(disposable);
    return disposable;
  }

  during<R>(activation: Activation, run: () => R): R {
    const previous = this.activation;
    this.activation = activation;
    try {
      return run();
    } finally {
      this.activation = previous;
    }
  }
}

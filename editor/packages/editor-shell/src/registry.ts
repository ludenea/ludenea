import type {
  CommandDefinition,
  Disposable,
  InspectorComponent,
  PanelDefinition,
  SettingDefinition,
  ViewportOverlay,
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

  /** Every value in registration order; the same array until the next change. */
  values = (): T[] => this.snapshot;

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
  /** Normalized key to command id. */
  readonly shortcuts: Collection<string> = new Collection("shortcut", this);
  readonly overlays: Collection<ViewportOverlay> = new Collection("overlay", this);
  readonly settings: Collection<SettingDefinition<unknown>> = new Collection("setting", this);
  private activation: Activation | null = null;

  owner(): string {
    return this.activation?.name ?? "editor";
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

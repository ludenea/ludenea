import type { LiveDevices, RawGamepad } from "@ludenea/control-client";
import { isEditorKey, type Disposable, type Editor } from "@ludenea/editor-shell";

const IDLE: LiveDevices = { keys: [], gamepads: [] };

/** Every gamepad slot, in the Gamepad API's standard mapping; `null` for an empty slot. */
function gamepads(): (RawGamepad | null)[] {
  const slots = typeof navigator.getGamepads === "function" ? navigator.getGamepads() : [];
  return Array.from(slots, (pad) => (pad ? { buttons: pad.buttons.map((button) => button.value), axes: [...pad.axes] } : null));
}

/** The players the "Set the player count" command offers. */
const PLAYER_COUNTS = [1, 2, 3, 4];

/** Registers one palette command per player count; each replaces the recorded inputs, so its title says so. */
export function registerPlayerCommands(editor: Editor): Disposable[] {
  return PLAYER_COUNTS.map((players) =>
    editor.commands.register({
      id: `input.set-players-${players}`,
      title: `Set the player count to ${players} (replaces the recorded inputs)`,
      async run() {
        const control = editor.control;
        if (!control) throw new Error("no game is running");
        const { devices } = await control.request({ type: "set_players", players, claims: null });
        const assigned = devices.map((held, index) => `player ${index + 1}: ${held}`).join("; ");
        editor.console.log("info", `${players} ${players === 1 ? "player" : "players"}, ${assigned}`);
      },
    }),
  );
}

/**
 * While a game with an input type plays and `element` has focus, sends every key held
 * down (`KeyboardEvent.code` values) and every gamepad slot with `SetLiveInput`; the host splits them
 * between the players, at most once
 * per animation frame and only when it changed, and captures shortcuts. Keys the editor keeps
 * (`F5`, `Mod` combinations) are not sent. Losing focus, pausing, or the returned stop
 * function sends an idle state.
 */
export function captureInput(editor: Editor, element: HTMLElement): () => void {
  const pressed = new Set<string>();
  let sent: string | null = null;
  let capture: Disposable | null = null;

  const send = (devices: LiveDevices) => {
    const control = editor.control;
    // A game without an input type has nothing to map the keys to.
    if (!control?.game.input) return;
    const text = JSON.stringify(devices);
    if (text === sent) return;
    sent = text;
    control.request({ type: "set_live_input", devices }).catch((error) => editor.console.report(error, "Sending the live input to the game"));
  };
  const release = () => {
    capture?.dispose();
    capture = null;
    if (sent !== null) send(IDLE);
    sent = null;
  };

  let frame = requestAnimationFrame(function loop() {
    if (editor.timeline.getState().playing && document.activeElement === element && editor.control?.game.input) {
      capture ??= editor.shortcuts.capture();
      send({ keys: [...pressed], gamepads: gamepads() });
    } else if (capture) release();
    frame = requestAnimationFrame(loop);
  });

  const onKeyDown = (event: KeyboardEvent) => {
    if (!isEditorKey(event)) pressed.add(event.code);
  };
  const onKeyUp = (event: KeyboardEvent) => void pressed.delete(event.code);
  const onBlur = () => {
    pressed.clear();
    release();
  };
  element.addEventListener("keydown", onKeyDown);
  element.addEventListener("keyup", onKeyUp);
  element.addEventListener("blur", onBlur);
  // A new game knows nothing of what was sent to the previous one.
  const game = editor.onGameChanged(() => (sent = null));

  return () => {
    cancelAnimationFrame(frame);
    element.removeEventListener("keydown", onKeyDown);
    element.removeEventListener("keyup", onKeyUp);
    element.removeEventListener("blur", onBlur);
    game.dispose();
    release();
  };
}

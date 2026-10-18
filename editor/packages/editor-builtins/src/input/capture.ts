import type { RawGamepad, RawInput } from "@ludenea/control-client";
import { isEditorKey, type Disposable, type Editor } from "@ludenea/editor-shell";

const IDLE: RawInput = { keys: [], gamepads: [] };

/** The first connected gamepad, in the Gamepad API's standard mapping. */
function firstGamepad(): RawGamepad[] {
  const pad = (typeof navigator.getGamepads === "function" ? navigator.getGamepads() : []).find((pad) => pad !== null);
  return pad ? [{ buttons: pad.buttons.map((button) => button.value), axes: [...pad.axes] }] : [];
}

/**
 * While a game with an input type plays and `element` has focus, sends what player 0 holds
 * down (`KeyboardEvent.code` values and the first gamepad) with `SetLiveInput`, at most once
 * per animation frame and only when it changed, and captures shortcuts. Keys the editor keeps
 * (`F5`, `Mod` combinations) are not sent. Losing focus, pausing, or the returned stop
 * function sends an idle state. One player for now.
 */
export function captureInput(editor: Editor, element: HTMLElement): () => void {
  const pressed = new Set<string>();
  let sent: string | null = null;
  let capture: Disposable | null = null;

  const send = (input: RawInput) => {
    const control = editor.control;
    // A game without an input type has nothing to map the keys to.
    if (!control?.game.input) return;
    const text = JSON.stringify(input);
    if (text === sent) return;
    sent = text;
    control.request({ type: "set_live_input", players: [input] }).catch((error) => console.error("live input not sent:", error));
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
      send({ keys: [...pressed], gamepads: firstGamepad() });
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

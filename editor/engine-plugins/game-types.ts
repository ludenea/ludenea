// Stands for a game's generated `game.d.ts`, which types the engine's components in full. A
// `.ts` file, so `skipLibCheck` does not hide a conflicting declaration.
import "@ludenea/editor-shell";

declare module "@ludenea/editor-shell" {
  interface ComponentTypes {
    "ludenea::Body": {
      kind: "Static" | "Kinematic" | "Dynamic";
      linear_velocity: { x: number; y: number };
      angular_velocity: number;
      linear_damping: number;
      angular_damping: number;
      gravity_scale: number;
    };
  }
}

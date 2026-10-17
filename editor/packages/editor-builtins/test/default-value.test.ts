import { describe, expect, it } from "vitest";
import { defaultValue } from "../src/inspector/default-value";
import { schemas } from "./support";

describe("default value", () => {
  it("default_value_from_schema_for_body", () => {
    expect(defaultValue(schemas.body)).toEqual({
      kind: "Static",
      linear_velocity: { x: 0, y: 0 },
      angular_velocity: 0,
      linear_damping: 0,
      angular_damping: 0,
      gravity_scale: 0,
    });
  });

  it("newtype_variant_and_containers_take_their_content_default", () => {
    expect(defaultValue(schemas.collider)).toEqual({ shape: { Circle: { center: { x: 0, y: 0 }, radius: 0 } }, density: 0 });
    expect(defaultValue({ kind: "struct", name: "t", fields: [
      { name: "name", schema: { kind: "string" } },
      { name: "alive", schema: { kind: "bool" } },
      { name: "tags", schema: { kind: "list", item: { kind: "string" } } },
      { name: "pair", schema: { kind: "array", item: { kind: "u8" }, len: 2 } },
      { name: "target", schema: { kind: "option", item: { kind: "entity" } } },
    ] })).toEqual({ name: "", alive: false, tags: [], pair: [0, 0], target: null });
  });
});

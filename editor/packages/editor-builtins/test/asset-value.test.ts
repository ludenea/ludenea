import type { TypeSchema } from "@ludenea/control-client";
import { describe, expect, it } from "vitest";
import { emptyValue, isComplete, toRon } from "../src/assets/value";

const weapon: TypeSchema = {
  kind: "struct",
  name: "demo::Weapon",
  fields: [
    { name: "speed", schema: { kind: "f32" } },
    { name: "name", schema: { kind: "string" } },
    { name: "tags", schema: { kind: "list", item: { kind: "u8" } } },
    { name: "parent", schema: { kind: "option", item: { kind: "handle", asset: "demo::Weapon" } } },
    {
      name: "kind",
      schema: {
        kind: "enum",
        name: "demo::Kind",
        variants: [
          { name: "Melee", fields: { kind: "unit" } },
          { name: "Ranged", fields: { kind: "struct", fields: [{ name: "range", schema: { kind: "u32" } }] } },
        ],
      },
    },
  ],
};

describe("new data asset values", () => {
  it("an_empty_value_is_complete_once_every_field_is_given", () => {
    const empty = emptyValue(weapon);
    expect(empty).toEqual({ speed: null, name: null, tags: [], parent: null, kind: null });
    expect(isComplete(weapon, empty)).toBe(false);
    expect(isComplete(weapon, { speed: 1, name: "sword", tags: [], parent: null, kind: { Ranged: { range: null } } })).toBe(false);
    expect(isComplete(weapon, { speed: 1, name: "sword", tags: [], parent: null, kind: { Ranged: { range: 3 } } })).toBe(true);
  });

  it("a_complete_value_is_written_as_ron", () => {
    const value = { speed: 2, name: "bow", tags: [1, 2], parent: "0000000000000000000000000000000a", kind: { Ranged: { range: 30 } } };
    expect(toRon(weapon, value)).toBe('(speed: 2.0, name: "bow", tags: [1, 2], parent: Some("0000000000000000000000000000000a"), kind: Ranged(range: 30))');
    expect(toRon(weapon, { ...value, speed: 0.5, parent: null, kind: "Melee" })).toBe('(speed: 0.5, name: "bow", tags: [1, 2], parent: None, kind: Melee)');
  });
});

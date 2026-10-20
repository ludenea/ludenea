import type { TypeSchema } from "@ludenea/control-client";
import { describe, expect, it } from "vitest";
import { mixedPaths } from "../src/inspector/fields/mixed";
import { schemas } from "./support";

const circle = (radius: number) => ({ Circle: { center: { x: 0, y: 0 }, radius } });
const capsule = { Capsule: { radius: 1 } };

describe("mixed paths", () => {
  it("the_same_values_mix_nothing", () => {
    const collider = { shape: circle(1), density: 2 };
    expect([...mixedPaths(schemas.collider, [collider, collider])]).toEqual([]);
  });

  it("a_field_inside_the_same_variant_is_the_one_marked", () => {
    const values = [
      { shape: circle(1), density: 2 },
      { shape: circle(3), density: 2 },
    ];
    expect([...mixedPaths(schemas.collider, values)]).toEqual(["shape.Circle.radius"]);
  });

  it("another_variant_marks_the_enum", () => {
    const values = [
      { shape: circle(1), density: 2 },
      { shape: capsule, density: 5 },
    ];
    expect([...mixedPaths(schemas.collider, values)].sort()).toEqual(["density", "shape"]);
  });

  it("a_list_of_another_length_or_an_option_set_on_some_only_marks_itself", () => {
    const schema: TypeSchema = {
      kind: "struct",
      name: "t",
      fields: [
        { name: "items", schema: { kind: "list", item: { kind: "u32" } } },
        { name: "maybe", schema: { kind: "option", item: { kind: "u32" } } },
      ],
    };
    expect([...mixedPaths(schema, [{ items: [1, 2], maybe: 1 }, { items: [1, 3], maybe: 2 }])]).toEqual(["items.1", "maybe"]);
    expect([...mixedPaths(schema, [{ items: [1], maybe: 1 }, { items: [1, 2], maybe: null }])]).toEqual(["items", "maybe"]);
  });
});

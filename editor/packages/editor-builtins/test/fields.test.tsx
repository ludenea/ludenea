import type { JsonValue, PathSegment, TypeSchema } from "@ludenea/control-client";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Field } from "../src/inspector/fields/Field";
import { schemas, settle, testEditor } from "./support";

const field = (name: string): PathSegment => ({ type: "field", name });
const ID_A = "0000000000000000000000000000000a";
const ID_B = "0000000000000000000000000000000b";
const shape = schemas.collider.fields[0]!.schema;

/** One field on its own, with `edit` recording what it sends. */
async function show(schema: TypeSchema, value: JsonValue, path: PathSegment[] = [field("value")]) {
  const host = await testEditor([], (request) =>
    request.type === "list_files"
      ? { type: "files", entries: [{ path: "assets/a.png", id: ID_A, kind: "Other" }, { path: "assets/b.png", id: ID_B, kind: "Other" }] }
      : undefined,
  );
  const edit = vi.fn(async (_path: PathSegment[], _value: JsonValue) => {});
  render(<Field schema={schema} value={value} path={path} edit={edit} readOnly={false} editor={host.editor} />);
  await settle();
  return { host, edit };
}

describe("fields", () => {
  afterEach(cleanup);

  it("asset_id_picker_lists_the_project_files_and_sends_the_chosen_id", async () => {
    const unknown = "00000000000000000000000000000fff";
    const { edit } = await show({ kind: "asset_id" }, unknown);
    const picker = screen.getByRole("combobox", { name: "value" }) as HTMLSelectElement;
    expect([...picker.options].map((option) => option.textContent)).toEqual([unknown, "assets/a.png", "assets/b.png"]);

    fireEvent.change(picker, { target: { value: ID_B } });
    expect(edit).toHaveBeenCalledWith([field("value")], ID_B);
  });

  it("entity_link_selects_the_entity", async () => {
    const { host } = await show({ kind: "entity" }, { index: 3, generation: 2 }, [field("target")]);
    fireEvent.click(screen.getByRole("button", { name: "target" }));
    expect(host.editor.selection.getState().entities).toEqual([{ index: 3, generation: 2 }]);
  });

  it("list_items_are_added_and_removed_by_sending_the_whole_list", async () => {
    const { edit } = await show({ kind: "list", item: { kind: "f32" } }, [1, 2], [field("items")]);
    fireEvent.click(screen.getByRole("button", { name: "Add item" }));
    expect(edit).toHaveBeenLastCalledWith([field("items")], [1, 2, 0]);

    fireEvent.click(screen.getByRole("button", { name: "Remove items.0" }));
    expect(edit).toHaveBeenLastCalledWith([field("items")], [2]);
  });

  it("enum_variant_switch_sends_the_new_variant_with_its_defaults", async () => {
    const { edit } = await show(shape, { Circle: { center: { x: 0, y: 0 }, radius: 0.5 } }, [field("shape")]);
    expect(screen.getByLabelText("shape.Circle.radius")).toBeTruthy();

    fireEvent.change(screen.getByRole("combobox", { name: "shape" }), { target: { value: "Capsule" } });
    expect(edit).toHaveBeenCalledWith([field("shape")], { Capsule: { radius: 0 } });
  });

  it("enum_without_a_value_lets_any_variant_be_picked", async () => {
    const { edit } = await show(shape, null, [field("shape")]);
    const picker = screen.getByRole("combobox", { name: "shape" }) as HTMLSelectElement;
    // A placeholder shows instead of the first variant, which can then be picked like the others.
    expect(picker.selectedOptions[0]?.disabled).toBe(true);
    act(() => fireEvent.change(picker, { target: { value: "Circle" } }));
    expect(edit).toHaveBeenCalledWith([field("shape")], { Circle: { center: { x: 0, y: 0 }, radius: 0 } });
  });
});

import type { JsonValue, PathSegment, TypeSchema } from "@ludenea/control-client";
import { pathText } from "./inputs";

const field = (name: string): PathSegment => ({ type: "field", name });
const index = (index: number): PathSegment => ({ type: "index", index });
const same = (a: JsonValue, b: JsonValue) => JSON.stringify(a) === JSON.stringify(b);
const isObject = (value: JsonValue): value is { [key: string]: JsonValue } => typeof value === "object" && value !== null && !Array.isArray(value);

/**
 * The paths where `values`, one per entity, differ, as the inspector lays `schema` out: the
 * deepest row or group (a list of another length, an option set on some only, another variant)
 * whose values are not all the same.
 */
export function mixedPaths(schema: TypeSchema, values: JsonValue[]): Set<string> {
  const mixed = new Set<string>();
  walk(schema, values, [], mixed);
  return mixed;
}

function walk(schema: TypeSchema, values: JsonValue[], path: PathSegment[], mixed: Set<string>): void {
  const [first = null] = values;
  if (values.every((value) => same(value, first))) return;
  const inside = (inner: TypeSchema, at: PathSegment, pick: (value: JsonValue) => JsonValue) => walk(inner, values.map(pick), [...path, at], mixed);
  const member = (value: JsonValue, name: string) => (isObject(value) ? (value[name] ?? null) : null);
  const item = (value: JsonValue, position: number) => (Array.isArray(value) ? (value[position] ?? null) : null);
  switch (schema.kind) {
    case "newtype":
      return walk(schema.item, values, path, mixed);
    case "option":
      if (values.every((value) => value !== null)) return walk(schema.item, values, path, mixed);
      break;
    case "struct":
      for (const { name, schema: inner } of schema.fields) inside(inner, field(name), (value) => member(value, name));
      return;
    case "tuple":
    case "tuple_struct":
      schema.items.forEach((inner, position) => inside(inner, index(position), (value) => item(value, position)));
      return;
    case "array":
    case "list": {
      const length = Array.isArray(first) ? first.length : -1;
      if (!values.every((value) => Array.isArray(value) && value.length === length)) break;
      for (let position = 0; position < length; position++) inside(schema.item, index(position), (value) => item(value, position));
      return;
    }
    case "enum": {
      // A unit variant is its name; any other is `{ name: content }`.
      const name = (value: JsonValue) => (typeof value === "string" ? value : isObject(value) ? Object.keys(value)[0] : undefined);
      const variant = schema.variants.find((candidate) => candidate.name === name(first));
      if (!variant || !values.every((value) => name(value) === variant.name)) break;
      const content = field(variant.name);
      const fields = variant.fields;
      if (fields.kind === "newtype") inside(fields.item, content, (value) => member(value, variant.name));
      if (fields.kind === "tuple") inside({ kind: "tuple", items: fields.items }, content, (value) => member(value, variant.name));
      if (fields.kind === "struct") inside({ kind: "struct", name: variant.name, fields: fields.fields }, content, (value) => member(value, variant.name));
      return;
    }
  }
  mixed.add(pathText(path));
}

import type { JsonValue, TypeSchema } from "@ludenea/control-client";

/**
 * A value of `schema` for the user to fill: every number, string, asset, and enum is `null`
 * (unset) until given; booleans, options, lists, and maps start valid (false, None, empty).
 */
export function emptyValue(schema: TypeSchema): JsonValue {
  switch (schema.kind) {
    case "bool":
      return false;
    case "list":
      return [];
    case "map":
      return {};
    case "array":
      return Array.from({ length: schema.len }, () => emptyValue(schema.item));
    case "tuple":
    case "tuple_struct":
      return schema.items.map(emptyValue);
    case "struct":
      return Object.fromEntries(schema.fields.map((field) => [field.name, emptyValue(field.schema)]));
    case "newtype":
      return emptyValue(schema.item);
    case "entity":
      return { index: 0, generation: 0 };
    default:
      return null;
  }
}

/** Whether every value `emptyValue` left unset in `value` was given. */
export function isComplete(schema: TypeSchema, value: JsonValue): boolean {
  switch (schema.kind) {
    case "unit":
    case "unit_struct":
    case "option":
    case "map":
      return true;
    case "list":
    case "array":
      return (value as JsonValue[]).every((item) => isComplete(schema.item, item));
    case "tuple":
    case "tuple_struct":
      return schema.items.every((item, index) => isComplete(item, (value as JsonValue[])[index] ?? null));
    case "struct":
      return schema.fields.every((field) => isComplete(field.schema, (value as { [name: string]: JsonValue })[field.name] ?? null));
    case "newtype":
      return isComplete(schema.item, value);
    case "enum": {
      if (value === null) return false;
      if (typeof value === "string") return true;
      const [name, content] = Object.entries(value as { [name: string]: JsonValue })[0]!;
      const fields = schema.variants.find((variant) => variant.name === name)?.fields;
      if (fields?.kind === "newtype") return isComplete(fields.item, content);
      if (fields?.kind === "tuple") return isComplete({ kind: "tuple", items: fields.items }, content);
      if (fields?.kind === "struct") return isComplete({ kind: "struct", name, fields: fields.fields }, content);
      return true;
    }
    default:
      return value !== null;
  }
}

const float = (value: JsonValue) => (Number.isInteger(value) ? (value as number).toFixed(1) : String(value));
const joined = (items: string[]) => items.join(", ");

/** `value` (in the host's JSON form, complete) as the RON text of a data asset file. */
export function toRon(schema: TypeSchema, value: JsonValue): string {
  switch (schema.kind) {
    case "f32":
    case "f64":
      return float(value);
    case "string":
    case "asset_id":
    case "handle":
      return JSON.stringify(value);
    case "unit":
    case "unit_struct":
      return "()";
    case "option":
      return value === null ? "None" : `Some(${toRon(schema.item, value)})`;
    case "list":
      return `[${joined((value as JsonValue[]).map((item) => toRon(schema.item, item)))}]`;
    case "array":
      return `(${joined((value as JsonValue[]).map((item) => toRon(schema.item, item)))})`;
    case "tuple":
    case "tuple_struct":
      return `(${joined(schema.items.map((item, index) => toRon(item, (value as JsonValue[])[index]!)))})`;
    case "map":
      // JSON keys are text: a string key is quoted, any other written as it is.
      return `{${joined(Object.entries(value as { [key: string]: JsonValue }).map(([key, item]) => `${schema.key.kind === "string" ? JSON.stringify(key) : key}: ${toRon(schema.value, item)}`))}}`;
    case "struct":
      return `(${joined(schema.fields.map((field) => `${field.name}: ${toRon(field.schema, (value as { [name: string]: JsonValue })[field.name]!)}`))})`;
    case "newtype":
      return `(${toRon(schema.item, value)})`;
    case "entity": {
      const { index, generation } = value as { index: number; generation: number };
      return `(index: ${index}, generation: ${generation})`;
    }
    case "enum": {
      if (typeof value === "string") return value;
      const [name, content] = Object.entries(value as { [name: string]: JsonValue })[0]!;
      const fields = schema.variants.find((variant) => variant.name === name)!.fields;
      if (fields.kind === "newtype") return `${name}(${toRon(fields.item, content)})`;
      if (fields.kind === "tuple") return `${name}${toRon({ kind: "tuple", items: fields.items }, content)}`;
      if (fields.kind === "struct") return `${name}${toRon({ kind: "struct", name, fields: fields.fields }, content)}`;
      return name;
    }
    default:
      // Integers (64-bit ones are strings, to keep their digits) and booleans.
      return String(value);
  }
}

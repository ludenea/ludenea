import type { JsonValue, PathSegment, TypeSchema, VariantSchema } from "@ludenea/control-client";

const ZERO_ASSET = "0".repeat(32);

/**
 * A value of `schema` in its wire form: zeros, `false`, empty strings and lists, `None`, and
 * the first enum variant. It may still be invalid for its type (a zero rotation); the host
 * says so with `InvalidEdit`.
 */
export function defaultValue(schema: TypeSchema): JsonValue {
  switch (schema.kind) {
    case "bool":
      return false;
    case "string":
      return "";
    case "unit":
    case "unit_struct":
    case "option":
      return null;
    case "list":
      return [];
    case "map":
      return {};
    case "array":
      return Array.from({ length: schema.len }, () => defaultValue(schema.item));
    case "tuple":
    case "tuple_struct":
      return schema.items.map(defaultValue);
    case "struct":
      return Object.fromEntries(schema.fields.map((field) => [field.name, defaultValue(field.schema)]));
    case "newtype":
      return defaultValue(schema.item);
    case "enum":
      return schema.variants[0] ? variantValue(schema.variants[0]) : null;
    case "entity":
      return { index: 0, generation: 0 };
    case "asset_id":
    case "handle":
      return ZERO_ASSET;
    default:
      return 0;
  }
}

/** A variant in serde's externally tagged form: its name alone, or `{ name: content }`, the content from `seed`. */
export function variantValue(variant: VariantSchema, seed: (schema: TypeSchema) => JsonValue = defaultValue): JsonValue {
  const { fields } = variant;
  switch (fields.kind) {
    case "unit":
      return variant.name;
    case "newtype":
      return { [variant.name]: seed(fields.item) };
    case "tuple":
      return { [variant.name]: fields.items.map(seed) };
    case "struct":
      return { [variant.name]: Object.fromEntries(fields.fields.map((field) => [field.name, seed(field.schema)])) };
  }
}

/** `root` with the value at `path` replaced, in the host's path semantics. */
export function setAt(root: JsonValue, path: PathSegment[], value: JsonValue): JsonValue {
  const [first, ...rest] = path;
  if (!first) return value;
  if (first.type === "index") {
    const items = [...(root as JsonValue[])];
    items[first.index] = setAt(items[first.index] ?? null, rest, value);
    return items;
  }
  const object = root as { [name: string]: JsonValue };
  return { ...object, [first.name]: setAt(object[first.name] ?? null, rest, value) };
}

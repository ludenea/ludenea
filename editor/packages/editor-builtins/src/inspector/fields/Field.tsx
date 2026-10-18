import type { JsonValue, PathSegment, TypeSchema } from "@ludenea/control-client";
import type { Editor } from "@ludenea/editor-shell";
import type { ReactNode } from "react";
import { defaultValue, variantValue } from "../default-value";
import { AssetField, BoolField, EntityField, FieldRow, LargeIntegerField, NumberField, StringField, pathText, useCommit, type Edit } from "./inputs";

export interface FieldProps {
  schema: TypeSchema;
  value: JsonValue;
  path: PathSegment[];
  edit: Edit;
  readOnly: boolean;
  editor: Editor;
}

const field = (name: string): PathSegment => ({ type: "field", name });
const index = (index: number): PathSegment => ({ type: "index", index });

/** One value rendered from its schema; every change goes through `edit` with its path. */
export function Field(props: FieldProps) {
  const { schema, value, path } = props;
  switch (schema.kind) {
    case "bool":
      return <BoolField {...props} />;
    case "u64":
    case "i64":
      return <LargeIntegerField {...props} />;
    case "f32":
    case "f64":
      return <NumberField {...props} integer={false} />;
    case "u8":
    case "u16":
    case "u32":
    case "i8":
    case "i16":
    case "i32":
      return <NumberField {...props} integer />;
    case "string":
      return <StringField {...props} />;
    case "entity":
      return <EntityField {...props} />;
    case "asset_id":
      return <AssetField {...props} />;
    case "unit":
    case "unit_struct":
      return null;
    // A newtype struct is its content on the wire, and so is `Some`.
    case "newtype":
      return <Field {...props} schema={schema.item} />;
    case "option":
      return <OptionField {...props} item={schema.item} />;
    case "struct":
      return (
        <Group path={path}>
          {schema.fields.map((member) => (
            <Field key={member.name} {...props} schema={member.schema} value={(value as Record<string, JsonValue>)[member.name] ?? null} path={[...path, field(member.name)]} />
          ))}
        </Group>
      );
    case "tuple":
    case "tuple_struct":
      return <Items {...props} schemas={schema.items} />;
    case "array":
      return <Items {...props} schemas={(value as JsonValue[]).map(() => schema.item)} />;
    case "list":
      return <ListField {...props} item={schema.item} />;
    case "enum":
      return <EnumField {...props} schema={schema} />;
    case "map":
      return (
        <FieldRow path={path}>
          <code aria-label={pathText(path)}>{JSON.stringify(value)}</code>
        </FieldRow>
      );
  }
}

function Group({ path, children }: { path: PathSegment[]; children: ReactNode }) {
  if (path.length === 0) return <>{children}</>;
  return (
    <fieldset className="ludenea-group">
      <legend>{pathText(path.slice(-1))}</legend>
      {children}
    </fieldset>
  );
}

/** Indexed rows, one per element. */
function Items(props: FieldProps & { schemas: TypeSchema[] }) {
  const items = props.value as JsonValue[];
  return (
    <Group path={props.path}>
      {props.schemas.map((schema, position) => (
        <Field key={position} {...props} schema={schema} value={items[position] ?? null} path={[...props.path, index(position)]} />
      ))}
    </Group>
  );
}

/** Indexed rows, plus adding and removing elements by sending the whole list. */
function ListField(props: FieldProps & { item: TypeSchema }) {
  const { path, edit, readOnly, item } = props;
  const items = props.value as JsonValue[];
  const { error, commit } = useCommit(edit, path);
  return (
    <Group path={path}>
      {items.map((element, position) => (
        <div key={position} className="ludenea-list-row">
          <Field {...props} schema={item} value={element} path={[...path, index(position)]} />
          {!readOnly && (
            <button aria-label={`Remove ${pathText([...path, index(position)])}`} onClick={() => commit(items.filter((_, other) => other !== position))}>
              Remove
            </button>
          )}
        </div>
      ))}
      {!readOnly && <button onClick={() => commit([...items, defaultValue(item)])}>Add item</button>}
      {error && <p className="ludenea-error">{error}</p>}
    </Group>
  );
}

function OptionField(props: FieldProps & { item: TypeSchema }) {
  const { value, path, edit, readOnly, item } = props;
  const { error, commit } = useCommit(edit, path);
  if (value === null) {
    return (
      <FieldRow path={path} error={error}>
        <span className="ludenea-muted">None</span> {!readOnly && <button onClick={() => commit(defaultValue(item))}>Set</button>}
      </FieldRow>
    );
  }
  return (
    <div className="ludenea-option">
      <Field {...props} schema={item} />
      {!readOnly && (
        <button aria-label={`Clear ${pathText(path)}`} onClick={() => commit(null)}>
          Clear
        </button>
      )}
      {error && <p className="ludenea-error">{error}</p>}
    </div>
  );
}

/** A variant select, then the variant's content under the field named after it. */
function EnumField(props: FieldProps & { schema: Extract<TypeSchema, { kind: "enum" }> }) {
  const { schema, value, path, edit, readOnly } = props;
  const { error, commit } = useCommit(edit, path);
  // A unit variant is its name; any other is `{ name: content }`; anything else picks none.
  const name = typeof value === "string" ? value : typeof value === "object" && value !== null ? Object.keys(value)[0] : undefined;
  const variant = schema.variants.find((candidate) => candidate.name === name);
  const content = typeof value === "object" && value !== null && name ? (value as Record<string, JsonValue>)[name]! : null;
  const inner = [...path, field(name ?? "")];
  const fields = variant?.fields;
  return (
    <div className="ludenea-enum">
      <FieldRow path={path} error={error}>
        <select
          aria-label={pathText(path)}
          value={name ?? ""}
          disabled={readOnly}
          onChange={(event) => {
            const next = schema.variants.find((candidate) => candidate.name === event.target.value);
            if (next) commit(variantValue(next));
          }}
        >
          {name === undefined && (
            <option value="" disabled>
              Choose a variant
            </option>
          )}
          {schema.variants.map((candidate) => (
            <option key={candidate.name}>{candidate.name}</option>
          ))}
        </select>
      </FieldRow>
      {fields?.kind === "newtype" && <Field {...props} schema={fields.item} value={content} path={inner} />}
      {fields?.kind === "tuple" && <Items {...props} schemas={fields.items} value={content} path={inner} />}
      {fields?.kind === "struct" && <Field {...props} schema={{ kind: "struct", name: name!, fields: fields.fields }} value={content} path={inner} />}
    </div>
  );
}

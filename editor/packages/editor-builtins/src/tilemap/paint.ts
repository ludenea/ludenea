import type { AssetId, Rot, TileArea, Vec2 } from "@ludenea/control-client";

/** A `ludenea::Tilemap` as its JSON form gives it: `tiles` row-major from the bottom-left cell. */
export interface Tilemap {
  tileset: AssetId;
  cell_size: Vec2;
  width: number;
  height: number;
  tiles: (number | null)[];
  layer: string;
  order: number;
}

/** A `ludenea::Pose` as its JSON form gives it. */
export interface Pose {
  position: Vec2;
  rotation: Rot;
}

/** A cell of a tilemap's grid, (0, 0) the bottom-left one; outside the grid when out of its bounds. */
export type Cell = { x: number; y: number };

/**
 * The cell of the grid under `world` (taken on the tilemap's layer, with its parallax), wherever
 * it falls, out of the grid included. The renderer draws a tilemap at its pose's position only:
 * the rotation does not turn it.
 */
export function cellUnder(world: Vec2, pose: Pose, tilemap: Tilemap): Cell {
  return {
    x: Math.floor((world.x - pose.position.x) / tilemap.cell_size.x),
    y: Math.floor((world.y - pose.position.y) / tilemap.cell_size.y),
  };
}

const inside = (cell: Cell, tilemap: Tilemap) => cell.x >= 0 && cell.y >= 0 && cell.x < tilemap.width && cell.y < tilemap.height;

/** The cell of the grid under `world` (taken on the tilemap's layer, with its parallax), null outside the grid. */
export function cellAt(world: Vec2, pose: Pose, tilemap: Tilemap): Cell | null {
  const cell = cellUnder(world, pose, tilemap);
  return inside(cell, tilemap) ? cell : null;
}

/**
 * The rectangle of cells from `a` to `b` (both included, in any order), clamped to the grid.
 * At least one of them lies in the grid: a stroke starts on a cell.
 */
export function areaBetween(a: Cell, b: Cell, tilemap: Tilemap): TileArea {
  const clamp = (value: number, size: number) => Math.min(Math.max(value, 0), size - 1);
  const left = clamp(Math.min(a.x, b.x), tilemap.width);
  const right = clamp(Math.max(a.x, b.x), tilemap.width);
  const bottom = clamp(Math.min(a.y, b.y), tilemap.height);
  const top = clamp(Math.max(a.y, b.y), tilemap.height);
  return { x: left, y: bottom, width: right - left + 1, height: top - bottom + 1 };
}

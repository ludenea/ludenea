# Forms written on each cell

The prepared forms of the smallest ladder size, as each cell of the research CI wrote them in the release run of 2026-11-12 ([run](https://github.com/ludenea/ludenea/actions/runs/43254006864)), one directory per cell. The spike reads them on every cell in a later run: a world whose shapes come from a form written on another cell is stepped against a world that built its shapes locally.

- `rapier-triangle-mesh.bin`, `rapier-triangle-mesh-fixed-internal-edges.bin`, `rapier-convex-hull.bin`: the `bincode` bytes of a Parry shape.
- `box2d-convex-polygons.bin`: the spike's own bytes of 125 Box2D polygons.
- `box2d-chain-world-image.bin`: the image of a Box2D world that holds a static body and its chain, which is not a prepared form of the chain and is read beside it.

The files are measurements, never assets of the engine.

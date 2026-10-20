// jsdom has no canvas: its `getContext` logs "Not implemented" and returns null. The viewport
// draws only when it gets a context, so tests that draw give it one with `vi.spyOn`.
HTMLCanvasElement.prototype.getContext = () => null;

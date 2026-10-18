import { afterEach, beforeEach, vi } from "vitest";

// jsdom has no layout, hence no `scrollIntoView`: each test gets a stub, removed after it.
beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  delete (Element.prototype as Partial<Element>).scrollIntoView;
});

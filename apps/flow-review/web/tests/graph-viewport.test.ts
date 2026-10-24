import { describe, expect, it } from "vitest";
import { readableViewport } from "../src/graph-viewport";

describe("readable graph viewport", () => {
  it("keeps long graphs at readable scale with selected node and downstream context visible", () => {
    const focus = { x: 5000, y: 300, width: 230, height: 96 };
    const view = readableViewport(
      { x: 0, y: -200, width: 12000, height: 800 },
      focus,
      1000,
      600,
    );
    expect(view.zoom).toBeGreaterThanOrEqual(0.8);
    expect(view.zoom).toBeLessThanOrEqual(1);
    const left = focus.x * view.zoom + view.x;
    expect(left).toBeGreaterThan(0);
    expect(left + focus.width * view.zoom).toBeLessThan(500);
    expect(focus.y * view.zoom + view.y).toBeGreaterThan(0);
  });
  it("fits a small graph normally", () => {
    const bounds = { x: 0, y: 0, width: 400, height: 200 };
    const view = readableViewport(
      bounds,
      { x: 0, y: 0, width: 230, height: 96 },
      1000,
      600,
    );
    expect(view.zoom).toBe(1);
    expect(bounds.x * view.zoom + view.x).toBe(300);
  });
  it("read selected explicitly restores readable focus after fitting the whole graph", () => {
    const bounds = { x: 0, y: 0, width: 800, height: 600 };
    const focus = { x: 500, y: 300, width: 230, height: 96 };
    const view = readableViewport(bounds, focus, 1000, 800, true);
    expect(view.zoom).toBe(1);
    expect((focus.x + focus.width / 2) * view.zoom + view.x).toBe(280);
    expect((focus.y + focus.height / 2) * view.zoom + view.y).toBe(400);
  });
});

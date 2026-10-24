import { getViewportForBounds, type Rect } from "@xyflow/react";

/** Whole-graph fitting is useful for orientation, but never a readable default
 * for a long flow. Keep the current node toward the left to expose successors. */
export function readableViewport(
  bounds: Rect,
  focus: Rect,
  width: number,
  height: number,
  forceFocus = false,
) {
  const fitted = getViewportForBounds(bounds, width, height, 0.001, 1, 0.1);
  if (!forceFocus && fitted.zoom >= 0.8) return fitted;
  const zoom = 1;
  return {
    x: width * 0.28 - (focus.x + focus.width / 2) * zoom,
    y: height * 0.5 - (focus.y + focus.height / 2) * zoom,
    zoom,
  };
}

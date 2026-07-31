import {
  type MutableRefObject,
  type PointerEvent as ReactPointerEvent,
  type ReactElement,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState
} from "react";
import type { MaterializedEntity } from "./contracts";
import { groundBasis } from "./first-person-look";
import {
  clampSize,
  clampZoom,
  projectToMinimap,
  type MinimapView
} from "./minimap-projection";

// The observer's top-down pose, surfaced from ObserverControls through a ref so a
// camera move never re-renders the 3D scene — only the minimap marker.
export interface MinimapPose {
  readonly x: number;
  readonly z: number;
  readonly yaw: number;
}

const PAD = 10;
const MIN_ZOOM = 0.25;
const MAX_ZOOM = 8;
// Opening panel size (square), flush in the bottom-left corner.
const DEFAULT_SIZE = 300;

type DragState =
  | { readonly mode: "move"; readonly sx: number; readonly sy: number; readonly ox: number; readonly oy: number }
  | {
      readonly mode: "resize";
      readonly sx: number;
      readonly sy: number;
      readonly ow: number;
      readonly oh: number;
      readonly oy: number;
    }
  | null;

/**
 * A draggable, resizable, zoomable bird's-eye overview of the city, drawn in plain
 * SVG (no WebGL — renders even when the 3D loop is suspended). Every node is a dot
 * at its (x, z), coloured by its material; the observer's current position glows
 * and shows a facing needle.
 *
 * The whole map is grabbable — drag-and-drop it anywhere (the zoom buttons and the
 * resize grip opt out). It opens flush in the bottom-left corner, so the resize
 * grip sits at the TOP-right: dragging it grows the panel up and to the right while
 * the bottom-left corner stays pinned.
 */
export function Minimap({
  entities,
  poseRef,
  cityRadius
}: {
  readonly entities: ReadonlyMap<string, MaterializedEntity>;
  readonly poseRef: MutableRefObject<MinimapPose>;
  readonly cityRadius: number;
}) {
  // Minimaps live bottom-left, flush to the corner (the user can drag it anywhere).
  const [size, setSize] = useState({ w: DEFAULT_SIZE, h: DEFAULT_SIZE });
  const [pos, setPos] = useState(() => ({
    x: 0,
    y: (typeof window !== "undefined" ? window.innerHeight : 720) - DEFAULT_SIZE
  }));
  const [zoom, setZoom] = useState(1);
  const [pose, setPose] = useState<MinimapPose>(() => ({ ...poseRef.current }));
  const drag = useRef<DragState>(null);

  // Poll the pose ref on its own frame loop so only the marker re-renders (the ref
  // write in ObserverControls never triggers a React render). Idle frames bail out
  // via the identity-preserving state update, so a still camera costs nothing.
  useEffect(() => {
    let raf = 0;
    const tick = () => {
      const p = poseRef.current;
      setPose((prev) =>
        prev.x === p.x && prev.z === p.z && prev.yaw === p.yaw
          ? prev
          : { x: p.x, z: p.z, yaw: p.yaw }
      );
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [poseRef]);

  const onPointerMove = useCallback((event: PointerEvent) => {
    const state = drag.current;
    if (!state) return;
    if (state.mode === "move") {
      setPos({ x: state.ox + event.clientX - state.sx, y: state.oy + event.clientY - state.sy });
    } else {
      // Top-right resize: width grows to the right, height grows upward while the
      // bottom edge (oy + oh) stays pinned, so the flush corner never lifts off.
      const nextW = clampSize(state.ow + event.clientX - state.sx);
      const nextH = clampSize(state.oh - (event.clientY - state.sy));
      const bottom = state.oy + state.oh;
      setSize({ w: nextW, h: nextH });
      setPos((prev) => ({ x: prev.x, y: bottom - nextH }));
    }
  }, []);
  const onPointerUp = useCallback(() => {
    drag.current = null;
    window.removeEventListener("pointermove", onPointerMove);
    window.removeEventListener("pointerup", onPointerUp);
  }, [onPointerMove]);
  const beginDrag = useCallback(
    (state: DragState) => {
      drag.current = state;
      window.addEventListener("pointermove", onPointerMove);
      window.addEventListener("pointerup", onPointerUp);
    },
    [onPointerMove, onPointerUp]
  );
  useEffect(
    () => () => {
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", onPointerUp);
    },
    [onPointerMove, onPointerUp]
  );

  // Drag-and-drop the whole panel by grabbing the map. The zoom buttons and the
  // resize grip opt out (their own handlers stop propagation / are excluded here).
  const onPanelPointerDown = useCallback(
    (event: ReactPointerEvent) => {
      const target = event.target as Element;
      if (target.closest(".minimap__zoom") || target.closest(".minimap__resize")) return;
      event.preventDefault();
      beginDrag({ mode: "move", sx: event.clientX, sy: event.clientY, ox: pos.x, oy: pos.y });
    },
    [beginDrag, pos.x, pos.y]
  );

  const bodyW = size.w;
  const bodyH = size.h;
  const view: MinimapView = {
    width: bodyW,
    height: bodyH,
    radius: Math.max(1, cityRadius),
    zoom,
    pad: PAD
  };

  // The dots do not depend on the pose, so memoise them: pose-only re-renders reuse
  // this element and React skips reconciling ~hundreds of circles every frame.
  const dots = useMemo(() => {
    const nodes: ReactElement[] = [];
    for (const entity of entities.values()) {
      const [cx, cy] = projectToMinimap(entity.position[0], entity.position[2], view);
      nodes.push(
        <circle
          key={entity.id}
          cx={cx}
          cy={cy}
          r={1.6}
          fill={entity.visual.material.color}
          fillOpacity={0.85}
        />
      );
    }
    return <g>{nodes}</g>;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entities, bodyW, bodyH, zoom, cityRadius]);

  // Clamp the observer marker to the panel edge when it sits outside the fitted
  // city (the opening camera is pulled back beyond the city radius, and you can
  // roam past the edges). Standard minimap behaviour: the marker rides the border
  // and the needle still points where you face.
  const [rawX, rawY] = projectToMinimap(pose.x, pose.z, view);
  const inset = 6;
  const mx = Math.min(bodyW - inset, Math.max(inset, rawX));
  const my = Math.min(bodyH - inset, Math.max(inset, rawY));
  const forward = groundBasis(pose.yaw).forward;
  const needle = 12;

  return (
    <div
      className="minimap"
      style={{ left: pos.x, top: pos.y, width: size.w, height: size.h }}
      onPointerDown={onPanelPointerDown}
    >
      <svg
        className="minimap__body"
        width={bodyW}
        height={bodyH}
        onWheel={(event) =>
          setZoom((z) => clampZoom(z * (event.deltaY < 0 ? 1.12 : 1 / 1.12), MIN_ZOOM, MAX_ZOOM))
        }
      >
        {/* Plaza cross-hair (the city centre / origin). */}
        <line x1={bodyW / 2 - 4} y1={bodyH / 2} x2={bodyW / 2 + 4} y2={bodyH / 2} className="minimap__axis" />
        <line x1={bodyW / 2} y1={bodyH / 2 - 4} x2={bodyW / 2} y2={bodyH / 2 + 4} className="minimap__axis" />
        {dots}
        {/* Current observer position: a pulsing glow + a facing needle. */}
        <g transform={`translate(${mx}, ${my})`}>
          <circle className="minimap__halo" r={9} />
          <line
            className="minimap__needle"
            x1={0}
            y1={0}
            x2={forward[0] * needle}
            y2={forward[2] * needle}
          />
          <circle className="minimap__you" r={3.2} />
        </g>
      </svg>
      <span className="minimap__label">minimap</span>
      <div className="minimap__zoom">
        <button
          type="button"
          aria-label="Dézoomer"
          onClick={() => setZoom((z) => clampZoom(z / 1.3, MIN_ZOOM, MAX_ZOOM))}
        >
          −
        </button>
        <button
          type="button"
          aria-label="Zoomer"
          onClick={() => setZoom((z) => clampZoom(z * 1.3, MIN_ZOOM, MAX_ZOOM))}
        >
          +
        </button>
      </div>
      <div
        className="minimap__resize"
        aria-label="Redimensionner"
        onPointerDown={(event) => {
          event.preventDefault();
          event.stopPropagation();
          beginDrag({
            mode: "resize",
            sx: event.clientX,
            sy: event.clientY,
            ow: size.w,
            oh: size.h,
            oy: pos.y
          });
        }}
      />
    </div>
  );
}

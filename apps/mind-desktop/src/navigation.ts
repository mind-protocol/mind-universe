// World-native navigation & interaction (G3 item 5). A pure reducer over focus,
// selection, expansion, replay trails, and Actor/Observer control — the logic
// layer beneath the 3D renderer, kept renderer-free so it is deterministic and
// testable. It never invents entities: a navigation state can be pruned against
// the live view so releasing/removing an entity drops it from focus/selection.

import type { EntityId } from "./contracts";
import type { UniverseView } from "./universe-state";

export type ControlMode = "observer" | "actor";

export interface NavState {
  readonly focus: EntityId | null;
  readonly selection: ReadonlySet<EntityId>;
  readonly expanded: ReadonlySet<EntityId>;
  readonly trailsVisible: boolean;
  readonly mode: ControlMode;
  readonly actor: EntityId | null;
}

export type NavCommand =
  | { readonly kind: "focus"; readonly entity: EntityId }
  | { readonly kind: "clear_focus" }
  | { readonly kind: "select"; readonly entity: EntityId; readonly additive?: boolean }
  | { readonly kind: "deselect"; readonly entity: EntityId }
  | { readonly kind: "clear_selection" }
  | { readonly kind: "expand"; readonly entity: EntityId }
  | { readonly kind: "release"; readonly entity: EntityId }
  | { readonly kind: "toggle_trails" }
  | { readonly kind: "request_control"; readonly actor: EntityId }
  | { readonly kind: "release_control" };

export const initialNav = (): NavState => ({
  focus: null,
  selection: new Set(),
  expanded: new Set(),
  trailsVisible: false,
  mode: "observer",
  actor: null
});

export function applyNav(state: NavState, command: NavCommand): NavState {
  switch (command.kind) {
    case "focus":
      return { ...state, focus: command.entity };
    case "clear_focus":
      return { ...state, focus: null };
    case "select": {
      const selection = new Set(command.additive ? state.selection : []);
      selection.add(command.entity);
      return { ...state, selection };
    }
    case "deselect": {
      if (!state.selection.has(command.entity)) return state;
      const selection = new Set(state.selection);
      selection.delete(command.entity);
      return { ...state, selection };
    }
    case "clear_selection":
      return state.selection.size === 0 ? state : { ...state, selection: new Set() };
    case "expand": {
      if (state.expanded.has(command.entity)) return state;
      const expanded = new Set(state.expanded);
      expanded.add(command.entity);
      return { ...state, expanded };
    }
    case "release": {
      if (!state.expanded.has(command.entity)) return state;
      const expanded = new Set(state.expanded);
      expanded.delete(command.entity);
      return { ...state, expanded };
    }
    case "toggle_trails":
      return { ...state, trailsVisible: !state.trailsVisible };
    case "request_control":
      // Requesting control moves to actor mode over the named Actor. Whether the
      // Universe grants it is decided by the control channel (UniverseView.control);
      // this only records the local intent.
      return { ...state, mode: "actor", actor: command.actor };
    case "release_control":
      return { ...state, mode: "observer", actor: null };
  }
}

/**
 * Drops any focus/selection/expansion/actor that no longer exists in the view —
 * so a released or delta-removed entity cannot linger in the interaction state.
 */
export function pruneNav(state: NavState, view: UniverseView): NavState {
  const present = (id: EntityId) => view.entities.has(id);
  const selection = new Set([...state.selection].filter(present));
  const expanded = new Set([...state.expanded].filter(present));
  const focus = state.focus && present(state.focus) ? state.focus : null;
  const actorGone = state.actor !== null && !present(state.actor);
  return {
    ...state,
    focus,
    selection,
    expanded,
    mode: actorGone ? "observer" : state.mode,
    actor: actorGone ? null : state.actor
  };
}

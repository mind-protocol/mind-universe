import type {
  ControlState,
  EnergyTransfer,
  MaterializedEntity,
  MaterializedRelation,
  UniverseEvent
} from "./contracts";
import { isRenderableEnergyTransfer } from "./contracts";

export interface UniverseView {
  readonly revision: number | null;
  readonly sequence: number;
  readonly synchronized: boolean;
  readonly entities: ReadonlyMap<string, MaterializedEntity>;
  readonly relations: ReadonlyMap<string, MaterializedRelation>;
  readonly transfers: ReadonlyMap<string, EnergyTransfer>;
  readonly control: ControlState;
}

export const emptyUniverseView = (): UniverseView => ({
  revision: null,
  sequence: -1,
  synchronized: false,
  entities: new Map(),
  relations: new Map(),
  transfers: new Map(),
  control: { kind: "observer" }
});

export function applyUniverseEvent(
  state: UniverseView,
  event: UniverseEvent
): UniverseView {
  if (
    event.kind !== "snapshot_started" &&
    event.sequence !== state.sequence + 1
  ) {
    return { ...state, synchronized: false };
  }

  const next = { ...state, sequence: event.sequence };
  switch (event.kind) {
    case "snapshot_started":
      return {
        ...emptyUniverseView(),
        revision: event.revision,
        sequence: event.sequence,
        synchronized: true
      };
    case "entity_materialized": {
      const entities = new Map(state.entities);
      entities.set(event.entity.id, event.entity);
      return { ...next, entities };
    }
    case "entity_released": {
      const entities = new Map(state.entities);
      entities.delete(event.entity);
      const relations = new Map(
        [...state.relations].filter(
          ([, relation]) =>
            relation.source !== event.entity && relation.target !== event.entity
        )
      );
      const transfers = new Map(
        [...state.transfers].filter(
          ([, transfer]) =>
            transfer.source !== event.entity && transfer.target !== event.entity
        )
      );
      return { ...next, entities, relations, transfers };
    }
    case "relation_materialized": {
      const relations = new Map(state.relations);
      relations.set(event.relation.id, event.relation);
      return { ...next, relations };
    }
    case "energy_transferred": {
      if (!isRenderableEnergyTransfer(event.transfer)) {
        return { ...next, synchronized: false };
      }
      const transfers = new Map(state.transfers);
      transfers.set(event.transfer.transferId, event.transfer);
      return { ...next, transfers };
    }
    case "energy_transfer_released": {
      const transfers = new Map(state.transfers);
      transfers.delete(event.transferId);
      return { ...next, transfers };
    }
    case "control_changed":
      return { ...next, control: event.control };
  }
}

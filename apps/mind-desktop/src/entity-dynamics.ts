// Per-node dynamic modulation of a base embodiment form (ALIGN §2/§5b). A node
// "is what it does with energy": the SAME graph-resolved form varies per node by
// three orthogonal live signals, each on its own perceptual channel and each
// strictly derived from graph-declared bounds — the renderer invents nothing.
//
//   • energy  (MEASURED)          → emission  (emit / glow)
//   • weight  (poids, accumulated)→ scale     (plus grand)
//   • embedding                    → orientation + procedural micro-variation
//
// Honesty (ALIGN §3): energy brightens a node ONLY when it is epistemically
// confident. An absent signal leaves its channel at identity, so a node that
// declares nothing draws exactly as the base form — never a faked value. The
// embedding channel is what makes two same-role nodes distinct without authoring
// new forms: one base silhouette, infinitely many procedural variants.

import type { EmbodimentDynamics, EntityDynamicSignals, Vector3 } from "./contracts";
import { mapBounded } from "./embodiment";

// The derivation input keeps `embedding` OPTIONAL even though a stored entity's
// `dynamics.embedding` is mandatory: the pure function must still resolve to a
// sane identity for a caller that supplies none (e.g. a unit test).
export interface EntityDynamicsInput {
  readonly energy?: number;
  readonly weight?: number;
  readonly embedding?: readonly number[];
  /** May the node emit as if measured? (ALIGN §3 gate for the energy channel.) */
  readonly confident: boolean;
}

export interface EntityDynamics {
  /** Overall size multiplier from weight/poids. */
  readonly scale: number;
  /** Emissive-intensity multiplier from measured energy (1 when not applied). */
  readonly emissiveMultiplier: number;
  /** Orientation yaw in radians from the embedding direction. */
  readonly yaw: number;
  /** Micro-variation amplitude (fraction) for per-primitive jitter. */
  readonly microvariation: number;
}

export const IDENTITY_DYNAMICS: EntityDynamics = {
  scale: 1,
  emissiveMultiplier: 1,
  yaw: 0,
  microvariation: 0
};

const finite = (value: number | undefined): value is number =>
  typeof value === "number" && Number.isFinite(value);

const finiteVector = (embedding: readonly number[] | undefined): embedding is readonly number[] =>
  Array.isArray(embedding) && embedding.length > 0 && embedding.every(finite);

// A stable scalar mixed from the whole embedding and a salt — the deterministic
// substitute for randomness (no Date.now / Math.random: the scene must be
// byte-reproducible). Returns a value in [-1, 1].
function embeddingHash(embedding: readonly number[], salt: number): number {
  let accumulator = salt * 0.6180339887498949;
  for (let index = 0; index < embedding.length; index += 1) {
    accumulator += embedding[index] * Math.sin((index + 1) * 1.7 + salt * 0.37);
  }
  return Math.sin(accumulator * 12.9898 + salt * 78.233);
}

// Orientation follows the embedding's own direction (first two components), so
// nodes pointing "the same way" in latent space face the same way in the scene.
function embeddingYaw(embedding: readonly number[], maxRadians: number): number {
  const x = embedding[0];
  const y = embedding.length > 1 ? embedding[1] : 0;
  if (x === 0 && y === 0) return 0;
  return (Math.atan2(y, x) / Math.PI) * maxRadians;
}

/**
 * Resolves the per-node modulation from the graph-declared `dynamics` bounds and
 * the node's live signals. Returns identity when no bounds are declared, so a
 * mapping without a `dynamics` block renders every node at its base form.
 */
export function deriveEntityDynamics(
  bounds: EmbodimentDynamics | undefined,
  signals: EntityDynamicsInput
): EntityDynamics {
  if (!bounds) return IDENTITY_DYNAMICS;

  // weight (poids) → scale. Never invent a size: an unknown weight stays 1.
  const scale = finite(signals.weight)
    ? mapBounded(signals.weight, bounds.weight_to_scale)
    : 1;

  // energy → emission, GATED on epistemic confidence. An unconfident node (or one
  // with no energy) is not brightened, so an unmeasured node can never glow.
  const emissiveMultiplier =
    signals.confident && finite(signals.energy)
      ? mapBounded(signals.energy, bounds.energy_to_emissive)
      : 1;

  // embedding → orientation + micro-variation amplitude.
  let yaw = 0;
  let microvariation = 0;
  if (finiteVector(signals.embedding)) {
    yaw = embeddingYaw(signals.embedding, bounds.embedding_orientation_max_rad);
    // Amplitude from the embedding's own spread, in [0, embedding_microvariation].
    const spread = (embeddingHash(signals.embedding, 0) + 1) / 2; // [0, 1]
    microvariation = spread * bounds.embedding_microvariation;
  }

  return { scale, emissiveMultiplier, yaw, microvariation };
}

// A deterministic pseudo-vector from a node's id — the DEFAULT for the embedding
// channel when the graph declares no real embedding. It is a visual-individuation
// seed, NOT a measured embedding: it only drives orientation + micro-variation
// (aesthetic channels), never a truth claim. It is what lets "each asset varies"
// hold for every node, even one the graph knows nothing semantic about.
export function proceduralEmbedding(seed: string): number[] {
  const out: number[] = [];
  for (let axis = 0; axis < 4; axis += 1) {
    let hash = (2166136261 ^ ((axis + 1) * 0x01000193)) >>> 0;
    for (let i = 0; i < seed.length; i += 1) {
      hash = Math.imul(hash ^ seed.charCodeAt(i), 0x01000193) >>> 0;
    }
    out.push((hash / 0xffffffff) * 2 - 1); // [-1, 1]
  }
  return out;
}

/**
 * Fills the mandatory `dynamics` field's default: the declared signals, plus a
 * procedural embedding keyed by `id` when none was declared. Energy and weight
 * are left as declared (never invented) — only the individuation channel is
 * defaulted, so an undeclared node still renders as a distinct being without a
 * faked energy or size.
 */
export function withDefaultDynamics(
  id: string,
  signals?: Partial<EntityDynamicSignals>
): EntityDynamicSignals {
  if (signals?.embedding && signals.embedding.length > 0) {
    return signals as EntityDynamicSignals;
  }
  return { ...(signals ?? {}), embedding: proceduralEmbedding(id) };
}

// The neutral value of the mandatory `dynamics` field: no energy, no weight, and
// an empty embedding (⇒ identity — no orientation, no micro-variation). Used by
// hand-built fixtures that don't model live signals; the procedural per-node
// default is applied at the wire boundary (protocol-adapter / the Rust bin).
export const NEUTRAL_DYNAMICS: EntityDynamicSignals = { embedding: [] };

export interface PrimitiveJitter {
  readonly offset: Vector3;
  readonly scale: Vector3;
}

const NO_JITTER: PrimitiveJitter = { offset: [0, 0, 0], scale: [1, 1, 1] };

/**
 * A deterministic per-primitive perturbation seeded by the embedding and the
 * primitive's index: a small world-space offset and a per-axis scale factor,
 * each bounded by `amplitude`. This is what breaks the rigid symmetry of a
 * shared form so two nodes with the same silhouette read as distinct individuals.
 * `amplitude <= 0` (or no embedding) ⇒ exact identity, preserving base output.
 */
export function primitiveJitter(
  embedding: readonly number[] | undefined,
  index: number,
  amplitude: number
): PrimitiveJitter {
  if (!finiteVector(embedding) || !(amplitude > 0)) return NO_JITTER;
  const at = (salt: number) => embeddingHash(embedding, index * 6 + salt);
  return {
    offset: [at(1) * amplitude, at(2) * amplitude, at(3) * amplitude],
    scale: [1 + at(4) * amplitude, 1 + at(5) * amplitude, 1 + at(6) * amplitude]
  };
}

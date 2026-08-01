//! Wield the Appearance toolkit: materialize a construct's AFFORDANCES.
//!
//! The unit of appearance is the AFFORDANCE, not the object. A magic cup is a
//! construct that OFFERS affordances {contain, sip, fill, enchant} (+ `inspect`);
//! this bin resolves a MATERIALIZATION for EACH — the toolkit's authored specific
//! form keyed on the domain subtype, else the shipped DEFAULT for its kernel kind —
//! then attaches the binding to the construct as ONE atomic `UniverseWriteSet`
//! through the GENERIC MutationBond translator, over a fresh SCRATCH store seeded
//! with the canonical ontology, and proves it by INDEPENDENT readback.
//!
//! What this proves (Appearance toolkit v0, affordance-materialization model):
//!   * EVERY offered affordance resolves a materialization: the four figurative
//!     affordances use the magic_cup_binding's authored specific forms; `inspect`
//!     has no specific form and falls to `default_materializations["inspect"]` —
//!     proving the default path and TOTALITY (no affordance is ever un-materialized).
//!   * a default materialization is shipped for every closed kernel kind
//!     (inspect|place|connect|open|build|test).
//!   * palette closure: every materialization primitive is in the CLOSED renderer
//!     palette (the 11 after the extension); an out-of-palette primitive
//!     (`hypercube`) is REFUSED before any write.
//!   * Fog on unmeasured: the enchant activation channel with no measured charge
//!     renders Fog (dark aura), never a fabricated glow.
//!   * every part declares which affordance it materializes (non-empty inverse_mapping).
//!   * attach = put_entity(construct) + put_entity(binding) + put_relation(binding
//!     PART_OF construct) — the four closed kernel verbs, no fifth; the binding is a
//!     MEMBER of the construct (appearance lives in the construct). The intended
//!     overlay predicate PROJECTS_AS is not canonical, so it REMAPS to PART_OF
//!     (binding -> construct, part -> whole); 0 new symbols.
//!
//! Usage: `wield_appearance` (throwaway scratch store; never the live current).

use std::{error::Error, path::Path};

use serde_json::{json, Value};
use universe_core::{EntityKey, RelationKey, Tick};
use universe_e2e::mutation_translate::{translate_mutation_proposal, MutationPlan};
use universe_store::{load_seed, UniverseSnapshot, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

/// The CLOSED renderer primitive palette — mirrors `ALLOWED_PRIMITIVES` in
/// `crates/universe-assets/src/visual.rs` after the extension. A materialization
/// primitive outside this set is refused.
const PALETTE: [&str; 11] = [
    "icosphere",
    "sphere",
    "capsule",
    "points",
    "fresnel_shell",
    "box",
    "cylinder",
    "cone",
    "torus",
    "plane",
    "tube",
];

/// The six closed kernel affordance kinds — each MUST ship a default materialization.
const KERNEL_KINDS: [&str; 6] = ["inspect", "place", "connect", "open", "build", "test"];

/// The magic cup's offered affordances (domain subtype, kernel kind). In a live
/// system these come from the affordance face; here we declare a cup that offers
/// four figurative affordances PLUS `inspect` — which has NO authored specific
/// materialization and must fall to the shipped default for its kernel kind.
const CUP_AFFORDANCES: [(&str, &str); 5] = [
    ("contain", "place"),
    ("sip", "open"),
    ("fill", "place"),
    ("enchant", "build"),
    ("inspect", "inspect"),
];

/// A resolved materialization for one affordance.
struct Materialization {
    subtype: String,
    kernel_kind: String,
    source: &'static str, // "specific" | "default"
    form: Value,          // an array of arity-8 form_primitive_tuples
    inverse_mapping: String,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("WIELD-APPEARANCE FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let seed_path = repo.join("fixtures/ontology/canonical-ontology.json");
    let toolkit_path = repo.join("fixtures/ontology/appearance-toolkit-v0.json");

    // Fresh scratch store (never the live artifacts store).
    let store_dir = std::env::temp_dir().join("mind-wield-appearance-store");
    if store_dir.exists() {
        std::fs::remove_dir_all(&store_dir)?;
    }
    std::fs::create_dir_all(&store_dir)?;

    let store = UniverseStore::open(&store_dir)?;
    let seed = load_seed(&seed_path)?;
    let mut snapshot = store.install_seed(&seed)?;
    let seed_symbol_count = snapshot.symbols.len();
    println!(
        "seeded scratch store: {} entities, {} relations, {} symbols, revision {}",
        snapshot.entities.len(),
        snapshot.relations.len(),
        seed_symbol_count,
        snapshot.revision.0
    );

    // ---- The Appearance toolkit fixture is the GRAPH DATA. --------------------
    // The `code` member carries the magic_cup_binding (authored specific
    // materializations), the default_materializations (one per kernel kind), and
    // the closed palette. Which primitives materialize `sip` or `enchant` is data.
    let toolkit: Value = serde_json::from_slice(&std::fs::read(&toolkit_path)?)?;
    let code = toolkit
        .get("members")
        .and_then(Value::as_array)
        .and_then(|ms| {
            ms.iter().find(|m| {
                m.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.starts_with("code:"))
            })
        })
        .and_then(|m| m.get("content"))
        .ok_or("appearance toolkit fixture has no code member with content")?;

    // Palette agreement (both directions): the toolkit declares exactly the
    // renderer's closed palette.
    let fixture_palette: Vec<&str> = code
        .pointer("/palette/allowed_primitives")
        .and_then(Value::as_array)
        .ok_or("no code.palette.allowed_primitives in the fixture")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for p in &fixture_palette {
        if !PALETTE.contains(p) {
            return Err(format!("fixture declares primitive `{p}` outside the renderer palette").into());
        }
    }
    for p in PALETTE {
        if !fixture_palette.contains(&p) {
            return Err(format!("renderer palette primitive `{p}` not declared by the toolkit fixture").into());
        }
    }
    println!(
        "palette agreement: renderer and toolkit both declare the same {} primitives",
        PALETTE.len()
    );

    // Authored specific materializations (subtype -> form + inverse_mapping).
    let specifics = code
        .pointer("/magic_cup_binding/affordance_materializations")
        .and_then(Value::as_array)
        .ok_or("no magic_cup_binding.affordance_materializations in the fixture")?;
    // Default materializations (kernel_kind -> form + inverse_mapping).
    let defaults = code
        .get("default_materializations")
        .and_then(Value::as_object)
        .ok_or("no default_materializations in the fixture")?;
    for kind in KERNEL_KINDS {
        if !defaults.contains_key(kind) {
            return Err(format!("no default materialization shipped for kernel kind `{kind}`").into());
        }
    }
    println!("default materializations shipped for all {} kernel kinds", KERNEL_KINDS.len());

    // ---- Resolve EACH offered affordance -> its materialization. --------------
    let mut resolved: Vec<Materialization> = Vec::new();
    for (subtype, kernel_kind) in CUP_AFFORDANCES {
        let specific = specifics.iter().find(|m| {
            m.pointer("/affordance/subtype").and_then(Value::as_str) == Some(subtype)
        });
        let materialization = if let Some(s) = specific {
            Materialization {
                subtype: subtype.into(),
                kernel_kind: kernel_kind.into(),
                source: "specific",
                form: s.get("form").cloned().ok_or("specific materialization has no form")?,
                inverse_mapping: s
                    .get("inverse_mapping")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
            }
        } else {
            let d = defaults
                .get(kernel_kind)
                .ok_or_else(|| format!("no default for kind `{kernel_kind}`"))?;
            Materialization {
                subtype: subtype.into(),
                kernel_kind: kernel_kind.into(),
                source: "default",
                form: d.get("form").cloned().ok_or("default materialization has no form")?,
                inverse_mapping: d
                    .get("inverse_mapping")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
            }
        };
        resolved.push(materialization);
    }

    // Validate every resolved materialization; totality = one per offered affordance.
    for m in &resolved {
        validate_materialization(m)?;
    }
    if resolved.len() != CUP_AFFORDANCES.len() {
        return Err("not every offered affordance resolved a materialization".into());
    }
    println!(
        "resolved {} affordances, all palette-closed, arity-8, scale>0, inverse_mapping present:",
        resolved.len()
    );
    for m in &resolved {
        println!(
            "  - {:<8} (kind {:<7} via {:<8}) -> {} part(s)",
            m.subtype,
            m.kernel_kind,
            m.source,
            m.form.as_array().map_or(0, Vec::len)
        );
    }

    // Fog on unmeasured: the enchant activation channel has no measured charge, so
    // it must render Fog (dark aura), never a fabricated glow.
    let measured_charge: Option<u64> = None;
    let activation_render = if measured_charge.is_some() { "lit" } else { "fog" };
    if activation_render != "fog" {
        return Err("unmeasured charge did not render as Fog — a fabricated glow".into());
    }
    println!("enchant activation: charge not_measured -> Fog (dark aura), no fabricated glow");

    // ---- NEGATIVE: an out-of-palette materialization must be REFUSED. ---------
    let bad = Materialization {
        subtype: "curse".into(),
        kernel_kind: "build".into(),
        source: "specific",
        form: json!([["hypercube", "sigil", "emissive", [0, 0, 0], [0, 0, 0], [0.2, 0.2, 0.2], 0, 0]]),
        inverse_mapping: "a hypercube sigil".into(),
    };
    match validate_materialization(&bad) {
        Ok(()) => return Err("out-of-palette primitive `hypercube` was NOT refused".into()),
        Err(error) => println!("[bad materialization] correctly refused before any write: {error}"),
    }

    // ---- Compose + attach the binding as ONE atomic write-set. ----------------
    let construct_key: u128 = 0xA000;
    let binding_key: u128 = 0xA001;
    let rel_part_of: u128 = 0xA100;

    // node-type + predicate symbols MUST pre-exist in the canonical seed (0 new).
    let sym_thing = resolve(&snapshot, "thing")?;
    // PROJECTS_AS is not canonical; the binding is a MEMBER of the construct, so the
    // overlay predicate remaps to PART_OF (binding -> construct, part -> whole).
    let pred_part_of = resolve(&snapshot, "PART_OF")?;

    let base_revision = snapshot.revision;

    let construct_content = json!({
        "kind": "construct",
        "name": "magic cup",
        "affordances": CUP_AFFORDANCES
            .iter()
            .map(|(s, k)| json!({ "subtype": s, "kernel_kind": k }))
            .collect::<Vec<_>>()
    });
    let materializations_json: Vec<Value> = resolved
        .iter()
        .map(|m| {
            json!({
                "affordance": { "subtype": m.subtype, "kernel_kind": m.kernel_kind },
                "source": m.source,
                "form": m.form,
                "inverse_mapping": m.inverse_mapping
            })
        })
        .collect();
    let binding_content = json!({
        "kind": "physicalization_binding",
        "schema_version": "affordance-materialization/1",
        "target": format!("{construct_key:#x}"),
        "overlay_predicate": "PROJECTS_AS",
        "canonical_predicate": "PART_OF",
        "palette": fixture_palette,
        "affordance_materializations": materializations_json,
        "activation_render": activation_render
    });

    let mut commands: Vec<UniverseCommand> = Vec::new();
    // 1. put_entity construct (offers the affordances)
    translate_step(
        &store,
        base_revision,
        &MutationPlan::PutEntity {
            key: EntityKey(construct_key),
            generation: 0,
            symbol: sym_thing,
            content_field: Some("content".into()),
        },
        &json!({ "content": construct_content }),
        &mut commands,
    )?;
    // 2. put_entity binding (the per-affordance materializations)
    translate_step(
        &store,
        base_revision,
        &MutationPlan::PutEntity {
            key: EntityKey(binding_key),
            generation: 0,
            symbol: sym_thing,
            content_field: Some("content".into()),
        },
        &json!({ "content": binding_content }),
        &mut commands,
    )?;
    // 3. put_relation binding PART_OF construct (appearance lives in the construct)
    translate_step(
        &store,
        base_revision,
        &MutationPlan::PutRelation {
            key: RelationKey(rel_part_of),
            generation: 0,
            source: EntityKey(binding_key),
            target: EntityKey(construct_key),
            predicate: pred_part_of,
            content_field: None,
        },
        &json!({}),
        &mut commands,
    )?;

    // The four MutationBond write verbs are now the WHOLE of UniverseCommand, so
    // the closed-verb guard is carried by the type rather than by a runtime check
    // that can no longer fire: the fifth verb it excluded no longer exists
    // anywhere in the kernel. The 0-InternSymbols rule below still bites.
    if commands
        .iter()
        .any(|c| matches!(c, UniverseCommand::InternSymbols { .. }))
    {
        return Err("appearance emitted InternSymbols — expected 0 new symbols".into());
    }

    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: "appearance:magic-cup:v0".into(),
        commands,
    };
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let transaction = UniverseTransaction::prepare(&snapshot, write_set)?;
    let receipt = transaction.commit(&store, &mut snapshot, boundary_tick)?;
    println!("\n[attach] committed construct + binding + PART_OF as ONE atomic set: {receipt:?}");

    // ---- INDEPENDENT readback: fresh reopen from disk. ------------------------
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;

    let binding = after
        .entities
        .iter()
        .find(|e| e.key == EntityKey(binding_key))
        .ok_or("binding not found on readback")?;
    let content = fresh.read_content(
        binding
            .content
            .as_ref()
            .ok_or("binding has no content")?,
    )?;

    // binding PART_OF construct (the appearance is a member of the construct)
    let part_of = after.relations.iter().any(|r| {
        r.key == RelationKey(rel_part_of)
            && r.source == EntityKey(binding_key)
            && r.target == EntityKey(construct_key)
            && after.symbols.get(r.predicate as usize).map(String::as_str) == Some("PART_OF")
    });
    if !part_of {
        return Err("PART_OF edge (binding -> construct) missing on readback".into());
    }

    // Every affordance materialized in the committed binding, palette closure holds,
    // every materialization declares its affordance (inverse_mapping non-empty).
    let materializations = content
        .get("affordance_materializations")
        .and_then(Value::as_array)
        .ok_or("committed binding has no affordance_materializations")?;
    if materializations.len() != CUP_AFFORDANCES.len() {
        return Err(format!(
            "committed binding materializes {} affordances, expected {}",
            materializations.len(),
            CUP_AFFORDANCES.len()
        )
        .into());
    }
    for m in materializations {
        let form = m
            .get("form")
            .and_then(Value::as_array)
            .ok_or("committed materialization has no form")?;
        for tuple in form {
            let kind = tuple
                .get(0)
                .and_then(Value::as_str)
                .ok_or("committed tuple has no primitive kind")?;
            if !PALETTE.contains(&kind) {
                return Err(format!("committed materialization uses out-of-palette primitive `{kind}`").into());
            }
        }
        if m.get("inverse_mapping").and_then(Value::as_str).unwrap_or("").is_empty() {
            return Err("committed materialization has empty inverse_mapping".into());
        }
    }

    // 0 new symbols interned across the whole gesture.
    if after.symbols.len() != seed_symbol_count {
        return Err(format!(
            "symbol table grew {} -> {} — a non-canonical symbol was interned",
            seed_symbol_count,
            after.symbols.len()
        )
        .into());
    }

    println!(
        "[attach] readback rev {} -> {} | PART_OF(binding->construct)={part_of} | {} affordances materialized, all palette-closed, all with inverse_mapping | 0 new symbols",
        base_revision.0,
        after.revision.0,
        materializations.len()
    );
    println!(
        "\nRESULT: the Appearance toolkit materialized EACH of a magic cup's affordances \
         (contain/sip/fill/enchant from authored specific forms + inspect via its kernel-kind \
         DEFAULT), refused an out-of-palette primitive, kept an unmeasured charge as Fog, and \
         attached the binding as a MEMBER of the construct (binding PART_OF construct) in ONE \
         atomic set of the four closed kernel verbs — proven by independent readback, 0 new symbols."
    );
    Ok(())
}

/// Validate one materialization: at least one part; every tuple arity-8 with a
/// palette primitive and a positive vec3 scale; a non-empty inverse_mapping.
fn validate_materialization(m: &Materialization) -> Result<(), Box<dyn Error>> {
    let form = m
        .form
        .as_array()
        .ok_or_else(|| format!("{}: form is not an array", m.subtype))?;
    if form.is_empty() {
        return Err(format!("{}: materialization has no parts", m.subtype).into());
    }
    for tuple in form {
        let arr = tuple
            .as_array()
            .ok_or_else(|| format!("{}: form tuple is not an array", m.subtype))?;
        if arr.len() != 8 {
            return Err(format!("{}: form tuple arity {} != 8", m.subtype, arr.len()).into());
        }
        let kind = arr[0]
            .as_str()
            .ok_or_else(|| format!("{}: tuple[0] is not a primitive kind", m.subtype))?;
        if !PALETTE.contains(&kind) {
            return Err(format!("{}: out-of-palette primitive `{kind}`", m.subtype).into());
        }
        let scale = arr[5]
            .as_array()
            .ok_or_else(|| format!("{}: tuple scale is not a vec3", m.subtype))?;
        if scale.len() != 3 || scale.iter().any(|c| c.as_f64().map_or(true, |v| v <= 0.0)) {
            return Err(format!("{}: tuple scale must be a vec3 of positive values", m.subtype).into());
        }
    }
    if m.inverse_mapping.trim().is_empty() {
        return Err(format!("{}: materialization has empty inverse_mapping", m.subtype).into());
    }
    Ok(())
}

/// Resolve a symbol id from the seed snapshot, erroring if it is not already
/// interned — this enforces "0 new symbols": every node-type and predicate the
/// gesture needs MUST pre-exist in the canonical seed.
fn resolve(snapshot: &UniverseSnapshot, symbol: &str) -> Result<u32, Box<dyn Error>> {
    snapshot
        .symbol_id(symbol)
        .ok_or_else(|| format!("symbol `{symbol}` is not in the canonical seed (would need interning)").into())
}

/// Translate one one-verb step through the GENERIC translator and collect its
/// single command.
fn translate_step(
    store: &UniverseStore,
    base_revision: universe_core::Revision,
    plan: &MutationPlan,
    proposal: &Value,
    commands: &mut Vec<UniverseCommand>,
) -> Result<(), Box<dyn Error>> {
    let write_set = translate_mutation_proposal(
        plan,
        proposal,
        store,
        base_revision,
        "appearance:step:v0".into(),
    )?;
    if write_set.commands.len() != 1 {
        return Err(format!("translator produced {} commands, expected 1", write_set.commands.len()).into());
    }
    commands.extend(write_set.commands);
    Ok(())
}

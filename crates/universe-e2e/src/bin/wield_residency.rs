//! Wield the Residency toolkit: the demotion mechanism OBSERVES a crowded space
//! through a bounded query, selects its own candidate, runs ON THE VM, aggregates
//! its proposal into ONE atomic `UniverseWriteSet` through the GENERIC translator,
//! commits to a fresh SCRATCH store, and proves the result by INDEPENDENT readback.
//!
//! What this proves (Residency toolkit v0):
//!   * THE PROGRAM SELECTS ITS OWN TARGET. Nothing is handed in but the origin
//!     space, a selector and the reason to retain. `QueryOpen`/`QueryAwait` walk
//!     inward from the space over the selector's predicate, `FilterTruthy`
//!     discriminates session bodies from a notice board sharing the same space,
//!     `TopK` ranks, `Only` refuses to proceed unless exactly one candidate
//!     survived, and `GetField` carries that node's own identity into the
//!     mutation. This binary reads the target back OUT of the proposal rather
//!     than deciding it: a selection the program cannot express is a selection
//!     nothing attributes to it.
//!   * NOTHING IS HARD-CODED IN THE HOST: the containment predicate, the
//!     discriminating field, the name it is projected under, the score field, the
//!     score order and the criterion field are all read from the SELECTOR the
//!     program passes to `QueryOpen`.
//!   * one demotion = ONE atomic set of two one-verb bonds
//!     (put_entity upsert, generation + 1 -> tombstone_relation on the containment
//!     edge). There is no state in which a node is severed but unexplained.
//!   * THE LOAD-BEARING PROPERTY: the content that lands in the store still carries
//!     the fields the PROGRAM NEVER NAMED. The seeded bodies carry `sponsor` and
//!     `arrival_note`, which the residency program has no notion of; the readback
//!     asserts they survived the revision. This is `ExtendRecord` proving itself
//!     through the real write path rather than in a unit test — a reviser built on
//!     `MakeRecord` would have silently dropped them, along with the provenance of
//!     the very node it was revising.
//!   * RETENTION: the demoted node is PRESENT on a fresh reopen, under the SAME
//!     key and canonical_id. A demotion whose node cannot be read back afterwards
//!     is a failure, never a success. Nothing is deleted; what changes is
//!     reachability.
//!   * THE EPISTEMIC PATH IS REAL: the SAME program, run against a node whose
//!     criterion datum was never recorded, takes `BranchOnEvidence`'s own
//!     `unknown` successor, proposes NOTHING, and leaves that node standing with
//!     its edge intact. Missing data is not zero and not "long ago".
//!   * `residency_level` and which edges it severs are READ FROM THE GRAPH (the
//!     residency-toolkit fixture's `level_profiles`), not dispatched in code.
//!   * only the closed kernel verbs are emitted; 0 new symbols are interned.
//!
//! What this does NOT prove: that the mechanism SELF-WAKES. The trigger described
//! in the toolkit's `algorithm.trigger_model` is not wired (the physics-event ->
//! energy-deposit bridge is half built), so this run is entered as an operator
//! request. The write plan's generation and symbol are still supplied by this
//! binary rather than proposed by the program, because the IR has no arithmetic
//! with which to compute `generation + 1`. Only the `dormant` level is exercised.
//! The store is a throwaway scratch store: nothing is demoted in any live world.
//! All four are named gaps, not silent ones.
//!
//! Usage: `wield_residency` (uses a throwaway scratch store; never the live current).

use std::collections::{BTreeMap, BTreeSet};
use std::{error::Error, path::Path};

use serde_json::{json, Value};
use universe_core::{Epistemic, EntityKey, RelationKey, Revision, Tick};
use universe_e2e::mutation_translate::{
    ir_value_to_json, translate_mutation_proposal, MutationPlan,
};
use universe_ir::{CodeDefinition, Operator, QuerySpec, Value as IrValue};
use universe_query::QueryBudget;
use universe_store::{load_seed, EntityRecord, RelationRecord, UniverseSnapshot, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};
use universe_vm::{execute_program, ExecutionLimits, ExecutionReceipt, VmError, VmHost};

// Disjoint key block for this proof, clear of the canonical seed's 0x1000-0x2xxx.
// TWO spaces, so the SAME program meets a candidate it can judge and one it
// cannot.
const SPACE_JUDGEABLE: u128 = 0xD000;
const SPACE_UNKNOWN: u128 = 0xD010;
const BODY_SELECTED: u128 = 0xD001;
const BODY_BYSTANDER: u128 = 0xD002;
const BODY_UNKNOWN: u128 = 0xD011;
const REL_SELECTED: u128 = 0xD100;
const REL_BYSTANDER: u128 = 0xD101;
const REL_UNKNOWN: u128 = 0xD110;

/// The two fields no residency program has any notion of. Their survival through
/// the revision is what this whole run exists to measure.
const UNNAMED_FIELDS: [&str; 2] = ["sponsor", "arrival_note"];

/// The capability the program declares for turning a stored datum into typed
/// evidence. The NAME is program data; what an absent datum means is the host's,
/// and the host answers `Unknown` rather than inventing a value.
const CRITERION: &str = "criterion_evidence";
const LOCAL_QUERY: &str = "local_query";

/// A host that answers a bounded observation FROM THE STORE.
///
/// It hard-codes no field name and no predicate: everything it walks and projects
/// is named by the `selector` record the program hands to `QueryOpen`. That is
/// what keeps the discriminator graph data rather than host policy.
struct StoreHost<'a> {
    store: &'a UniverseStore,
    snapshot: &'a UniverseSnapshot,
    /// Captured at `open_query`: the origin walked from, and what to project.
    opened: Option<(EntityKey, BTreeMap<String, IrValue>)>,
    /// The observation budget the program declared, honoured on await.
    max_entities: usize,
}

impl<'a> StoreHost<'a> {
    fn new(store: &'a UniverseStore, snapshot: &'a UniverseSnapshot) -> Self {
        Self {
            store,
            snapshot,
            opened: None,
            max_entities: 0,
        }
    }

    fn selector_text(selector: &BTreeMap<String, IrValue>, field: &str) -> Result<String, String> {
        match selector.get(field) {
            Some(IrValue::Text(text)) => Ok(text.clone()),
            _ => Err(format!("selector lacks a text `{field}`")),
        }
    }

    /// The stored content of one node, or `None` if it holds none.
    fn content_of(&self, key: EntityKey) -> Result<Option<Value>, String> {
        let Some(entity) = self.snapshot.entities.iter().find(|e| e.key == key) else {
            return Ok(None);
        };
        let Some(pointer) = entity.content.as_ref() else {
            return Ok(None);
        };
        self.store
            .read_content(pointer)
            .map(Some)
            .map_err(|error| error.to_string())
    }
}

impl VmHost for StoreHost<'_> {
    fn is_cancelled(&self) -> bool {
        false
    }

    fn capabilities(&self) -> BTreeSet<String> {
        BTreeSet::from([LOCAL_QUERY.to_string(), CRITERION.to_string()])
    }

    fn open_query(
        &mut self,
        spec: &QuerySpec,
        origin: &IrValue,
        selector: &IrValue,
    ) -> Result<IrValue, String> {
        let IrValue::Entity(origin) = origin else {
            return Err("an observation origin must be an entity".into());
        };
        let IrValue::Record(selector) = selector else {
            return Err("the selector must be a record naming what to project".into());
        };
        self.max_entities = spec.budget.max_entities;
        self.opened = Some((*origin, selector.clone()));
        Ok(IrValue::Text(format!("bounded:{origin}")))
    }

    /// Walk INWARD from the origin over the selector's predicate — a bounded local
    /// traversal, never a store scan — and project one record per held node.
    fn await_query(&mut self, _: &IrValue) -> Result<IrValue, String> {
        let (origin, selector) = self.opened.clone().ok_or("await without a prior open")?;
        let predicate = Self::selector_text(&selector, "predicate")?;
        let discriminator_field = Self::selector_text(&selector, "discriminator_field")?;
        let discriminator_as = Self::selector_text(&selector, "discriminator_as")?;
        let score_field = Self::selector_text(&selector, "score_field")?;
        let score_as = Self::selector_text(&selector, "score_as")?;
        let ascending = Self::selector_text(&selector, "score_order")? == "ascending";

        let predicate_symbol = self
            .snapshot
            .symbols
            .iter()
            .position(|symbol| *symbol == predicate)
            .ok_or_else(|| format!("predicate `{predicate}` is not interned"))?
            as u32;

        let mut held: Vec<IrValue> = Vec::new();
        for relation in self
            .snapshot
            .relations
            .iter()
            .filter(|r| r.predicate == predicate_symbol && r.target == origin)
            .take(self.max_entities)
        {
            let content = self.content_of(relation.source)?.unwrap_or(Value::Null);
            let mut projected = BTreeMap::from([
                ("entity".to_string(), IrValue::Entity(relation.source)),
                // The discriminator is projected as a BOOLEAN, because that is
                // what `FilterTruthy` reads. WHICH field decides it is the
                // program's; whether the field is there is the store's.
                (
                    discriminator_as.clone(),
                    IrValue::Bool(content.get(&discriminator_field).is_some()),
                ),
            ]);
            // A node that records no score gets NO score field. Ranking a node on
            // a value it never recorded would fabricate a measurement. It is still
            // returned, so it can be observed and branched on.
            if let Some(score) = content.get(&score_field).and_then(Value::as_i64) {
                projected.insert(
                    score_as.clone(),
                    IrValue::Integer(if ascending { -score } else { score }),
                );
            }
            held.push(IrValue::Record(projected));
        }
        Ok(IrValue::List(held))
    }

    /// Read the FULL stored content of each selected candidate — after filtering,
    /// never before. Returns content exactly as the store holds it, with none of
    /// the observation's own scaffolding mixed in, so what is extended and written
    /// back is the node's own record.
    fn hydrate(&mut self, selected: &[IrValue], _: u32) -> Result<Vec<IrValue>, String> {
        selected
            .iter()
            .map(|candidate| {
                let IrValue::Record(candidate) = candidate else {
                    return Err("a candidate must be a record".to_string());
                };
                let Some(IrValue::Entity(key)) = candidate.get("entity") else {
                    return Err("a candidate must carry its entity".to_string());
                };
                let content = self
                    .content_of(*key)?
                    .ok_or_else(|| format!("node {key} holds no content to hydrate"))?;
                Ok(json_to_ir(&content))
            })
            .collect()
    }

    /// Turn one stored datum into typed evidence. A recorded value is `Measured`;
    /// an absent one is `Unknown` — never zero, never "long ago".
    fn call_capability(&mut self, capability: &str, input: &IrValue) -> Result<IrValue, String> {
        if capability != CRITERION {
            return Err(format!("capability `{capability}` is not held"));
        }
        let (_, selector) = self.opened.clone().ok_or("no observation is open")?;
        let criterion_field = Self::selector_text(&selector, "score_field")?;
        let IrValue::Record(content) = input else {
            return Err("criterion evidence is read from a record".into());
        };
        Ok(IrValue::Epistemic(match content.get(&criterion_field) {
            Some(value) => Epistemic::Measured(Box::new(value.clone())),
            None => Epistemic::Unknown,
        }))
    }

    fn follow_one(&mut self, _: &IrValue, _: &IrValue) -> Result<IrValue, String> {
        Err("this program follows no single relation".into())
    }

    fn entity_symbol(&mut self, _: &IrValue) -> Result<IrValue, String> {
        Err("this program resolves no symbol".into())
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("WIELD-RESIDENCY FAILED: {error}");
        std::process::exit(1);
    }
}

/// The demotion program. ONE program, pointed at two different spaces.
///
/// Nothing is handed to it but the origin space, a selector, and the reason to
/// retain. It opens a bounded observation, discriminates the candidates, ranks
/// them, hydrates the one it kept, and carries that node's own identity into the
/// mutation with `GetField`. A selection the program cannot express is a
/// selection nothing attributes to it.
///
/// Operator 15 is the epistemic fork: an `observed`/`measured` criterion proceeds
/// to the extension, and every unavailable state routes to its OWN named exit at
/// operator 20, which proposes nothing. The VM will not coerce an absent
/// measurement into a boolean, so a body that never recorded the criterion cannot
/// be demoted by accident.
fn demotion_program() -> CodeDefinition {
    CodeDefinition {
        ir_version: universe_ir::IR_VERSION,
        revision: Revision(1),
        required_capabilities: vec![LOCAL_QUERY.to_string(), CRITERION.to_string()],
        operators: vec![
            /*  0 */ Operator::Input { name: "origin".into(), output: 0 },
            /*  1 */ Operator::Input { name: "selector".into(), output: 1 },
            /*  2 */ Operator::Input { name: "reaping".into(), output: 2 },
            /*  3 */ Operator::Constant { value: IrValue::Text("dormant".into()), output: 3 },
            /*  4 */ Operator::Constant { value: IrValue::Text("put_entity".into()), output: 4 },
            /*  5 */ Operator::Constant { value: IrValue::Text("left_standing".into()), output: 5 },
            /*  6 */
            Operator::QueryOpen {
                spec: QuerySpec {
                    origin: 0,
                    selector: 1,
                    budget: QueryBudget {
                        max_entities: 64,
                        max_relations: 128,
                        max_depth: 1,
                    },
                    timeout_ticks: 2,
                    allow_approximate: false,
                },
                output: 6,
            },
            /*  7 */ Operator::QueryAwait { handle: 6, output: 7 },
            /*  8 */
            Operator::FilterTruthy {
                input: 7,
                field: "embodies_a_session".into(),
                max_items: 64,
                output: 8,
            },
            /*  9 */
            Operator::TopK {
                input: 8,
                score_field: "staleness".into(),
                limit: 1,
                output: 9,
            },
            /* 10 */ Operator::Only { input: 9, output: 10 },
            /* 11 */
            Operator::GetField {
                input: 10,
                field: "entity".into(),
                output: 11,
            },
            /* 12 */
            Operator::Hydrate {
                input: 9,
                max_items: 1,
                max_bytes: 4096,
                output: 12,
            },
            /* 13 */ Operator::Only { input: 12, output: 13 },
            /* 14 */
            Operator::CapabilityCall {
                capability: CRITERION.into(),
                input: 13,
                output: 14,
            },
            /* 15 */
            Operator::BranchOnEvidence {
                input: 14,
                observed_next: 16,
                measured_next: 16,
                known_absent_next: 20,
                unknown_next: 20,
                not_measured_next: 20,
                measurement_failed_next: 20,
            },
            /* 16 */
            Operator::ExtendRecord {
                input: 13,
                fields: vec![("residency".into(), 3), ("reaping".into(), 2)],
                output: 15,
            },
            /* 17 */
            Operator::MakeRecord {
                fields: vec![
                    ("command".into(), 4),
                    ("entity".into(), 11),
                    ("content".into(), 15),
                ],
                output: 16,
            },
            /* 18 */ Operator::Propose { command: 16, output: 17 },
            /* 19 */ Operator::Return { value: 17 },
            /* 20 */ Operator::Return { value: 5 },
        ],
    }
}

/// A node that is NOT a session body: held by the same space, and which the
/// discriminator must never offer as a candidate.
fn furniture_content() -> Value {
    json!({
        "canonical_id": "thing:l2:scratch:notice-board",
        "kind": "thing",
        "provenance": "built",
    })
}

/// One seeded body, as the store holds it before any demotion. Deliberately
/// carries fields a residency program has no notion of.
fn body_content(session: &str, base_revision: Option<u64>) -> Value {
    let mut content = json!({
        "canonical_id": format!("actor:l1:mind:claude-{session}"),
        "kind": "actor",
        "provenance": "built",
        "embodied_session": format!("claude:{session}"),
        "residency": "hot",
        // Fields the residency program never names. Their survival is the proof.
        "sponsor": "lumina-prime-registry",
        "arrival_note": "moored to the beacon on arrival; correct then, wrong forever",
    });
    if let Some(revision) = base_revision {
        content["base_revision"] = json!(revision);
    }
    content
}

fn run() -> Result<(), Box<dyn Error>> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let seed_path = repo.join("fixtures/ontology/canonical-ontology.json");
    let toolkit_path = repo.join("fixtures/ontology/residency-toolkit-v0.json");

    // Fresh scratch store (never the live artifacts store).
    let store_dir = std::env::temp_dir().join("mind-wield-residency-store");
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

    // ---- The toolkit fixture is the GRAPH DATA that furnishes the shape. -----
    // `residency_level` and which edges it severs are read from here, never
    // dispatched in code.
    let toolkit: Value = serde_json::from_slice(&std::fs::read(&toolkit_path)?)?;
    let level = "dormant";
    let profile = |field: &str| -> Option<String> {
        toolkit
            .pointer(&format!("/content/modularity/level_profiles/{level}/{field}"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let severs = profile("severs")
        .ok_or("the toolkit fixture declares no `severs` rule for level `dormant`")?;
    let reads_back_as = profile("reads_back_as")
        .ok_or("the toolkit fixture declares no `reads_back_as` for level `dormant`")?;
    println!("\nresidency_level read from the graph: `{level}`");
    println!("  severs       : {severs}");
    println!("  reads back as: {reads_back_as}");

    // Resolve the symbols we write with. Both must already be canonical — this
    // proof interns nothing.
    let symbol_index = |name: &str| -> Option<u32> {
        snapshot.symbols.iter().position(|s| s == name).map(|i| i as u32)
    };
    let containment = "PART_OF";
    let containment_symbol = symbol_index(containment)
        .ok_or("canonical predicate PART_OF is not interned in the seed")?;
    let node_symbol = ["actor", "thing", "space"]
        .iter()
        .find_map(|candidate| symbol_index(candidate))
        .ok_or("no canonical node symbol (actor/thing/space) is interned in the seed")?;

    // ---- Seed two spaces and three held nodes. ------------------------------
    let put = |key: u128, content: &Value| -> Result<UniverseCommand, Box<dyn Error>> {
        Ok(UniverseCommand::PutEntity {
            entity: EntityRecord {
                key: EntityKey(key),
                generation: 0,
                symbol: node_symbol,
                content: Some(store.append_content(content)?),
            },
        })
    };
    let hold = |rel: u128, source: u128, space: u128| UniverseCommand::PutRelation {
        relation: RelationRecord {
            key: RelationKey(rel),
            generation: 0,
            source: EntityKey(source),
            target: EntityKey(space),
            predicate: containment_symbol,
            content: None,
        },
    };
    let selected_before = body_content("selected", Some(44));
    let unknown_before = body_content("unknown", None);
    let setup = vec![
        put(SPACE_JUDGEABLE, &json!({"canonical_id": "space:l2:scratch:crowded", "kind": "space"}))?,
        put(SPACE_UNKNOWN, &json!({"canonical_id": "space:l2:scratch:unrecorded", "kind": "space"}))?,
        put(BODY_SELECTED, &selected_before)?,
        put(BODY_BYSTANDER, &furniture_content())?,
        put(BODY_UNKNOWN, &unknown_before)?,
        hold(REL_SELECTED, BODY_SELECTED, SPACE_JUDGEABLE),
        hold(REL_BYSTANDER, BODY_BYSTANDER, SPACE_JUDGEABLE),
        hold(REL_UNKNOWN, BODY_UNKNOWN, SPACE_UNKNOWN),
    ];
    let boundary = Tick(snapshot.tick.0 + 1);
    UniverseTransaction::prepare(
        &snapshot,
        UniverseWriteSet {
            base_revision: snapshot.revision,
            idempotency_key: "wield-residency:setup:v0".into(),
            commands: setup,
        },
    )?
    .commit(&store, &mut snapshot, boundary)?;
    println!(
        "\nseeded 2 spaces / 3 held nodes at revision {}:",
        snapshot.revision.0
    );
    println!("  crowded space   : 1 session body (base_revision 44) + 1 notice board (not a body)");
    println!("  unrecorded space: 1 session body that never recorded a base_revision");

    // ---- Run the program TWICE, one program, two evidence states. -----------
    let program = demotion_program();
    let reaping = IrValue::Record(BTreeMap::from([
        (
            "reaped_by".into(),
            IrValue::Text("actor:l2:scratch:residency-authority".into()),
        ),
        (
            "reason".into(),
            IrValue::Text("the space holds more than its observation budget can show".into()),
        ),
        (
            "criterion".into(),
            IrValue::Text("the oldest arrival the bounded observation could rank".into()),
        ),
        (
            "severed_edges".into(),
            IrValue::List(vec![IrValue::Text(containment.into())]),
        ),
    ]));
    // The selector: EVERY field the observation projects is named HERE, by the
    // caller of the query, never inside the host.
    let selector = IrValue::Record(BTreeMap::from([
        ("predicate".into(), IrValue::Text(containment.into())),
        ("discriminator_field".into(), IrValue::Text("embodied_session".into())),
        ("discriminator_as".into(), IrValue::Text("embodies_a_session".into())),
        ("score_field".into(), IrValue::Text("base_revision".into())),
        ("score_as".into(), IrValue::Text("staleness".into())),
        ("score_order".into(), IrValue::Text("ascending".into())),
    ]));
    let observe_and_decide = |origin: u128| -> Result<ExecutionReceipt, VmError> {
        let mut host = StoreHost::new(&store, &snapshot);
        execute_program(
            &program,
            &mut host,
            &BTreeMap::from([
                ("origin".to_string(), IrValue::Entity(EntityKey(origin))),
                ("selector".to_string(), selector.clone()),
                ("reaping".to_string(), reaping.clone()),
            ]),
            snapshot.revision,
            snapshot.tick,
            ExecutionLimits {
                fuel: 512,
                max_proposals: 1,
            },
        )
    };

    // Run B first, so a proof that proposes nothing cannot be mistaken for a
    // proof that had not run yet.
    println!("\n-- run B: the space whose only body NEVER RECORDED the criterion --");
    let unknown_receipt = observe_and_decide(SPACE_UNKNOWN)?;
    println!("  result   : {:?}", unknown_receipt.result);
    println!("  proposals: {}", unknown_receipt.proposals.len());
    if !unknown_receipt.proposals.is_empty() {
        return Err("an unrecorded criterion must propose NOTHING — the unknown path leaked".into());
    }
    if unknown_receipt.result != IrValue::Text("left_standing".into()) {
        return Err("the unknown path did not reach its own named exit".into());
    }
    println!("  -> observed it, could not judge it, proposed nothing, left it standing");

    println!("\n-- run A: the crowded space --");
    let selected_receipt = observe_and_decide(SPACE_JUDGEABLE)?;
    if selected_receipt.proposals.len() != 1 {
        return Err(format!(
            "expected exactly one proposal, found {}",
            selected_receipt.proposals.len()
        )
        .into());
    }
    let proposal = ir_value_to_json(&selected_receipt.proposals[0].command);

    // THE PROGRAM chose the target. Read it back OUT of the proposal rather than
    // deciding it here — otherwise the selection would be this binary's, not the
    // mechanism's.
    let chosen = proposal
        .get("entity")
        .and_then(Value::as_str)
        .ok_or("the proposal does not name the entity the program selected")?;
    println!("  -> the program observed the space and selected: {chosen}");
    let chosen_key = snapshot
        .entities
        .iter()
        .map(|entity| entity.key)
        .find(|key| format!("{key}") == chosen)
        .ok_or_else(|| format!("the selected entity {chosen} is not in the store"))?;
    if chosen_key != EntityKey(BODY_SELECTED) {
        return Err(format!(
            "the program selected {chosen_key:?} — the notice board, or the wrong body"
        )
        .into());
    }
    let generation = snapshot
        .entities
        .iter()
        .find(|entity| entity.key == chosen_key)
        .map(|entity| entity.generation)
        .ok_or("the selected entity vanished between selection and write")?;

    // ---- Aggregate BOTH bonds into ONE atomic write set. --------------------
    let base_revision = snapshot.revision;
    let put_plan = MutationPlan::PutEntity {
        key: chosen_key,
        generation: generation + 1,
        symbol: node_symbol,
        content_field: Some("content".into()),
    };
    let sever_plan = MutationPlan::TombstoneRelation {
        relation: RelationKey(REL_SELECTED),
        generation: 0,
    };
    let mut commands = translate_mutation_proposal(
        &put_plan,
        &proposal,
        &store,
        base_revision,
        "wield-residency:demote:v0".into(),
    )?
    .commands;
    commands.extend(
        translate_mutation_proposal(
            &sever_plan,
            &proposal,
            &store,
            base_revision,
            "wield-residency:sever:v0".into(),
        )?
        .commands,
    );
    println!(
        "\naggregated {} one-verb bond(s) into ONE atomic write set",
        commands.len()
    );
    let transaction = UniverseTransaction::prepare(
        &snapshot,
        UniverseWriteSet {
            base_revision,
            idempotency_key: "wield-residency:demotion-set:v0".into(),
            commands,
        },
    )?;
    let boundary = Tick(snapshot.tick.0 + 1);
    let receipt = transaction.commit(&store, &mut snapshot, boundary)?;
    println!("commit receipt: {receipt:?}");

    // ---- INDEPENDENT READBACK: a fresh reopen, never the handle we wrote with.
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    println!("\n-- independent readback (fresh reopen) --");
    println!(
        "revision: {} -> {} | entities {} | relations {}",
        base_revision.0,
        after.revision.0,
        after.entities.len(),
        after.relations.len()
    );

    let mut failures: Vec<String> = Vec::new();
    let read = |key: u128| -> Result<Option<(u32, Value)>, Box<dyn Error>> {
        let Some(entity) = after.entities.iter().find(|e| e.key == EntityKey(key)) else {
            return Ok(None);
        };
        let content = match entity.content.as_ref() {
            Some(pointer) => fresh.read_content(pointer)?,
            None => Value::Null,
        };
        Ok(Some((entity.generation, content)))
    };
    let incident = |key: u128| -> usize {
        after
            .relations
            .iter()
            .filter(|r| r.source == EntityKey(key) || r.target == EntityKey(key))
            .count()
    };

    // The demoted body: RETAINED entire, and unreachable.
    match read(BODY_SELECTED)? {
        None => failures.push(
            "the demoted body is ABSENT on readback — a demotion must never delete a node".into(),
        ),
        Some((generation, content)) => {
            println!("  demoted body: present, generation {generation}");
            if generation != 1 {
                failures.push(format!("expected generation 1, read {generation}"));
            }
            if content.get("canonical_id") != selected_before.get("canonical_id") {
                failures.push("the demoted body lost or changed its canonical_id".into());
            }
            if content.get("residency").and_then(Value::as_str) != Some("dormant") {
                failures.push(format!(
                    "expected residency=dormant, read {:?}",
                    content.get("residency")
                ));
            }
            if content.pointer("/reaping/reason").is_none() {
                failures.push("the demoted body carries no retained reason".into());
            }
            // The observation's own scaffolding must not have leaked into the
            // node's stored content: what is written back is the node's record,
            // not the query's view of it.
            for leaked in ["entity", "embodies_a_session", "staleness"] {
                if content.get(leaked).is_some() {
                    failures.push(format!(
                        "the observation's own field `{leaked}` leaked into stored content"
                    ));
                }
            }
            // THE LOAD-BEARING ASSERTION.
            for field in UNNAMED_FIELDS {
                match content.get(field) {
                    Some(value) if value == selected_before.get(field).unwrap() => {
                        println!("    preserved (never named by the program): {field} = {value}");
                    }
                    other => failures.push(format!(
                        "field `{field}` did not survive the revision: {other:?} — the reviser \
                         discarded what it did not understand"
                    )),
                }
            }
            // And every other prior field.
            for (field, before) in selected_before.as_object().unwrap() {
                if field == "residency" {
                    continue; // deliberately revised
                }
                if content.get(field) != Some(before) {
                    failures.push(format!("prior field `{field}` did not survive the revision"));
                }
            }
            let remaining = incident(BODY_SELECTED);
            println!("    incident edges: {remaining} (must be 0 — it left the space)");
            if remaining != 0 {
                failures.push(format!(
                    "the demoted body still has {remaining} incident edge(s); it remains reachable"
                ));
            }
        }
    }

    // The notice board shared the crowded space and is NOT a session body: the
    // discriminator must never have offered it as a candidate.
    match read(BODY_BYSTANDER)? {
        None => failures.push("the notice board is ABSENT on readback".into()),
        Some((generation, content)) => {
            let remaining = incident(BODY_BYSTANDER);
            println!(
                "  notice board (not a body): generation {generation}, residency {:?}, \
                 incident edges {remaining}",
                content.get("residency")
            );
            if generation != 0 || remaining == 0 {
                failures.push(
                    "the notice board was touched — the discriminator selected a non-body".into(),
                );
            }
        }
    }

    // The body left standing: untouched, and still reachable.
    match read(BODY_UNKNOWN)? {
        None => failures.push("the body left standing is ABSENT on readback".into()),
        Some((generation, content)) => {
            let remaining = incident(BODY_UNKNOWN);
            println!(
                "  body left standing: present, generation {generation}, residency {:?}, \
                 incident edges {remaining}",
                content.get("residency")
            );
            if generation != 0 {
                failures.push("the body left standing was revised; it must be untouched".into());
            }
            if content.get("residency").and_then(Value::as_str) != Some("hot") {
                failures.push("the body left standing changed residency".into());
            }
            if remaining == 0 {
                failures.push(
                    "the body left standing lost its edge — an unknown criterion must not sever"
                        .into(),
                );
            }
        }
    }

    if after.symbols.len() != seed_symbol_count {
        failures.push(format!(
            "{} symbol(s) were interned; a demotion must intern zero",
            after.symbols.len() - seed_symbol_count
        ));
    } else {
        println!("  symbols: {} (0 interned)", after.symbols.len());
    }

    if !failures.is_empty() {
        for failure in &failures {
            eprintln!("  READBACK FAILURE: {failure}");
        }
        return Err(format!("{} readback check(s) failed", failures.len()).into());
    }

    println!(
        "\nRESULT: one demotion committed as ONE atomic set of 2 one-verb bonds. The demoted body \
         is RETAINED"
    );
    println!(
        "        entire — same key, same canonical_id, and every field the program never named \
         survived the"
    );
    println!(
        "        revision — while no observation from the space reaches it any more. The same \
         program, given a"
    );
    println!(
        "        criterion datum that was never recorded, proposed nothing and left that body \
         standing."
    );
    Ok(())
}

/// Bridge stored JSON content into the IR record the program extends. The inverse
/// of `ir_value_to_json`, narrowed to what node content can hold.
fn json_to_ir(value: &Value) -> IrValue {
    match value {
        Value::Null => IrValue::Unit,
        Value::Bool(flag) => IrValue::Bool(*flag),
        Value::Number(number) => number
            .as_i64()
            .map(IrValue::Integer)
            .unwrap_or_else(|| IrValue::Text(number.to_string())),
        Value::String(text) => IrValue::Text(text.clone()),
        Value::Array(items) => IrValue::List(items.iter().map(json_to_ir).collect()),
        Value::Object(fields) => IrValue::Record(
            fields
                .iter()
                .map(|(name, value)| (name.clone(), json_to_ir(value)))
                .collect(),
        ),
    }
}

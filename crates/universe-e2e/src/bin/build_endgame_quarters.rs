//! Build the endgame quarter of Lumina Prime: the city, the forum, the five
//! endgame districts' PLACEMENT, and the ways between them — each as ONE atomic
//! construction set through the GENERIC mutation translator, on the LIVE store,
//! proven by independent readback of the RELATIONS.
//!
//! This is the placement half of the district build. `inject_construct` writes a
//! district's ANATOMY (its 12 roles + its form); this bin writes where it IS —
//! and where a thing is, is never a coordinate here. It is:
//!
//!   * `district PART_OF city`  — the primary placement statement, and
//!   * the ways (`route`) that reach it, each `PART_OF city` and `COMMUNICATES`
//!     its two endpoint NODES.
//!
//! So two districts are at DIFFERENT places because their topology differs: the
//! Question district is a crossroads (two outgoing ways), Civilization a terminus
//! (two incoming, none out), Human Valence a leaf, and the Code district has NO
//! inter-district way at all — nothing depends on it, because it is frozen. That
//! is a readable fact about the work, not a layout accident, and it survives any
//! change in how the layout authority projects it to a screen.
//!
//! What this proves:
//!   * one construction = ONE atomic set of one-verb MutationBonds
//!   * `structure_kind` and its region/connection predicates are READ FROM THE
//!     GRAPH (construction-toolkit-v0.json `kind_profiles`), never dispatched
//!   * the `city` kind's `region_predicate` is null — a root region furnishes NO
//!     PART_OF step rather than a dangling one
//!   * NO coordinate is written in any form; readback asserts the relations and
//!     asserts no structure carries a position field
//!   * only canonical predicates (PART_OF, COMMUNICATES, PRODUCES, GROUNDS); a
//!     non-canonical upstream label (CONTRIBUTES_TO) travels as route CONTENT,
//!     never as an interned predicate — 0 new symbols
//!
//! Usage: `build_endgame_quarters [store-dir]`
//!   store-dir defaults to artifacts/ontology-registry/current/store

use std::{collections::BTreeMap, error::Error, path::PathBuf};

use serde_json::{json, Value};
use universe_core::{EntityKey, RelationKey, Revision, Tick};
use universe_e2e::mutation_translate::{translate_mutation_proposal, MutationPlan};
use universe_store::{UniverseSnapshot, UniverseStore};
use universe_transactions::{UniverseCommand, UniverseTransaction, UniverseWriteSet};

/// Disjoint key block for the quarter, well clear of the seed (~0x2300), the
/// one-off blocks (beacon 0xB000, pen 0xC000) and `inject_construct`'s hashed
/// windows (0x0001_0000 .. ~0x0FFF_0000). Every key is existence-checked anyway.
const ENTITY_BASE: u128 = 0x4000_0000;
const REL_BASE: u128 = 0x4100_0000;

const AUTHOR: &str = "a.inchauspe@digitalkin.ai";

/// The five districts, in portfolio priority order.
const DISTRICTS: [(&str, &str, &str); 5] = [
    ("autonomous-improvement", "P0", "Banc d'Essai"),
    ("question", "P0", "Halle des Réponses"),
    ("civilization", "P1", "Dix Fondations"),
    ("human-valence", "P1", "Jardin Clos"),
    ("code", "P2", "Halle Gelée"),
];

/// The inter-district ways, transcribed from the Mind Protocol portfolio's own
/// project-to-project edges (with their authored justifications). `upstream` is
/// the source graph's predicate; it travels as route CONTENT because
/// CONTRIBUTES_TO is not a canonical symbol here and must never be minted.
const WAYS: [(&str, &str, &str, &str); 4] = [
    (
        "autonomous-improvement",
        "civilization",
        "CONTRIBUTES_TO",
        "Les boucles adaptatives mesurées sont une primitive proposée du substrat civilisationnel ; aucune réussite institutionnelle n'est déduite des tests runtime seuls.",
    ),
    (
        "question",
        "autonomous-improvement",
        "CONTRIBUTES_TO",
        "Un runtime de question situé et auditable peut améliorer la détection des gaps et la sélection des sous-graphes, sous réserve de validation déterministe.",
    ),
    (
        "question",
        "civilization",
        "CONTRIBUTES_TO",
        "Une infrastructure institutionnelle continue exige des réponses sourcées, contextualisées et contestables dans chaque domaine.",
    ),
    (
        "human-valence",
        "autonomous-improvement",
        "FEEDS",
        "La valence attribuable fournit une dimension terminale facultative au résultat multidimensionnel ; elle ne devient ni score global ni vérité sur la personne.",
    ),
];

const CITY_ID: &str = "space:l2:lumina-prime:city-v0";
const FORUM_ID: &str = "place:l2:lumina-prime:endgame-forum-v0";

fn main() {
    if let Err(error) = run() {
        eprintln!("BUILD-QUARTERS FAILED: {error}");
        std::process::exit(1);
    }
}

/// One construction gesture: the ordered one-verb set for a single structure.
struct Construction {
    label: String,
    /// `Some((canonical_id, content))` puts a NEW structure entity; `None` places
    /// one the store already holds (a district, injected with its anatomy).
    structure_content: Option<(String, Value)>,
    subject: u128,
    region: Option<u128>,
    region_predicate: Option<String>,
    connection_predicate: Option<String>,
    connections: Vec<(u128, u128)>,
    moment: u128,
    justification: u128,
    rel_part_of: u128,
    rel_produces: u128,
    rel_grounds: u128,
    statement: String,
}

fn run() -> Result<(), Box<dyn Error>> {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let store_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join("artifacts/ontology-registry/current/store"));
    let toolkit_path = repo.join("fixtures/ontology/construction-toolkit-v0.json");
    println!("store dir: {}", store_dir.display());

    // The toolkit fixture is the GRAPH DATA that furnishes the shape: a kind's
    // region and connection predicates are read from here, never dispatched.
    let toolkit: Value = serde_json::from_slice(&std::fs::read(&toolkit_path)?)?;
    let profile = |kind: &str, field: &str| -> Option<String> {
        toolkit
            .pointer(&format!("/content/modularity/kind_profiles/{kind}/{field}"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let known_kind = |kind: &str| -> Result<(), Box<dyn Error>> {
        toolkit
            .pointer(&format!("/content/modularity/kind_profiles/{kind}"))
            .map(|_| ())
            .ok_or_else(|| format!("structure_kind `{kind}` has no kind_profile in the toolkit").into())
    };
    for kind in ["city", "place", "district", "route"] {
        known_kind(kind)?;
    }
    println!(
        "kind_profiles read from the graph: city region_predicate={:?} (root region), \
         place={:?}, district={:?}, route={:?} + connection {:?}",
        profile("city", "region_predicate"),
        profile("place", "region_predicate"),
        profile("district", "region_predicate"),
        profile("route", "region_predicate"),
        profile("route", "connection_predicate"),
    );

    // Open the LIVE store and replay to the authoritative snapshot.
    let store = UniverseStore::open(&store_dir)?;
    let mut snapshot = store.replay(store.load_snapshot()?)?;
    let start_revision = snapshot.revision;
    let start_symbols = snapshot.symbols.len();
    println!(
        "base revision: {} | entities: {} | relations: {} | symbols: {}",
        start_revision.0,
        snapshot.entities.len(),
        snapshot.relations.len(),
        start_symbols
    );

    // Resolve the already-injected district roots by canonical id.
    let by_canonical = index_by_canonical_id(&store, &snapshot)?;
    let mut district_key: BTreeMap<&str, u128> = BTreeMap::new();
    for (slug, _, _) in DISTRICTS {
        let id = format!("space:l2:lumina-prime:district-{slug}-v0");
        let key = *by_canonical.get(id.as_str()).ok_or_else(|| {
            format!("district `{id}` is not in the store — inject it before placing it")
        })?;
        district_key.insert(slug, key);
    }
    println!("resolved {} injected district roots", district_key.len());

    // ---- Furnish the constructions, in dependency order. -------------------
    let city = ENTITY_BASE;
    let forum = ENTITY_BASE + 1;
    let mut e = ENTITY_BASE + 2; // rolling entity cursor (moments, justifications, routes)
    let mut r = REL_BASE; // rolling relation cursor
    let mut next_e = || {
        e += 1;
        e
    };
    let mut next_r = || {
        r += 1;
        r
    };
    let mut plan: Vec<Construction> = Vec::new();

    // 1. The city — the ROOT region. Its kind_profile's region_predicate is null,
    //    so it furnishes NO PART_OF step: it is PART_OF nothing because nothing
    //    above it is built. known_absent, not a missing parent.
    plan.push(Construction {
        label: "city Lumina Prime".into(),
        structure_content: Some((CITY_ID.to_string(), json!({
            "kind": "built_structure",
            "structure_kind": "city",
            "provenance": "built",
            "name": "Lumina Prime",
            "root_region": true,
            "note": "La ville est la région racine : rien n'est bâti au-dessus d'elle, donc elle n'est PART_OF rien. Sa taille n'est pas authored — c'est ce qui est bâti dedans."
        }))),
        subject: city,
        region: None,
        region_predicate: profile("city", "region_predicate"),
        connection_predicate: None,
        connections: Vec::new(),
        moment: next_e(),
        justification: next_e(),
        rel_part_of: 0, // unused: no region
        rel_produces: next_r(),
        rel_grounds: next_r(),
        statement: "Lumina Prime est fondée comme région racine pour que les quartiers aient un endroit auquel appartenir. Aucune coordonnée n'est écrite : la ville est ce qui la contient.".into(),
    });

    // 2. The forum — the civic anchor every district's approach way meets.
    plan.push(Construction {
        label: "place Forum du Portefeuille".into(),
        structure_content: Some((FORUM_ID.to_string(), json!({
            "kind": "built_structure",
            "structure_kind": "place",
            "provenance": "built",
            "name": "Forum du Portefeuille Endgame",
            "governs": "project:mind:endgame-portfolio:v0",
            "note": "La place centrale ne tient aucun travail : c'est ce depuis quoi les cinq quartiers sont atteignables. Le portefeuille y gouverne l'ordre d'apprentissage (P0 prouver l'autonomie et la question ; P1 choisir le wedge et concevoir le pilote de valence ; P2 préparer seulement les preuves du Code)."
        }))),
        subject: forum,
        region: Some(city),
        region_predicate: profile("place", "region_predicate"),
        connection_predicate: None,
        connections: Vec::new(),
        moment: next_e(),
        justification: next_e(),
        rel_part_of: next_r(),
        rel_produces: next_r(),
        rel_grounds: next_r(),
        statement: "Le forum existe parce qu'un portefeuille qui gouverne cinq quartiers a besoin d'un endroit d'où ils sont tous atteignables, et d'où leur ordre de priorité se lit.".into(),
    });

    // 3. Place each district: the PART_OF city edge IS where it is. The district
    //    entity already exists (anatomy injected), so this set puts no structure.
    for (slug, priority, nickname) in DISTRICTS {
        let key = district_key[slug];
        plan.push(Construction {
            label: format!("district {slug} ({nickname}, {priority})"),
            structure_content: None,
            subject: key,
            region: Some(city),
            region_predicate: profile("district", "region_predicate"),
            connection_predicate: None,
            connections: Vec::new(),
            moment: next_e(),
            justification: next_e(),
            rel_part_of: next_r(),
            rel_produces: next_r(),
            rel_grounds: next_r(),
            statement: format!(
                "Le quartier « {nickname} » ({priority}) appartient à Lumina Prime. Cette appartenance EST son emplacement : aucune coordonnée n'est écrite, et sa place parmi les autres est portée par les voies qui l'atteignent."
            ),
        });
    }

    // 4. The approach ways: forum <-> district, one per district. Their existence
    //    is what makes every district reachable from the centre.
    let route_region_predicate = profile("route", "region_predicate");
    let route_connection_predicate = profile("route", "connection_predicate");
    for (slug, priority, nickname) in DISTRICTS {
        let key = district_key[slug];
        let route = next_e();
        plan.push(Construction {
            label: format!("approach way forum -> {slug}"),
            structure_content: Some((format!("route:l2:lumina-prime:forum-to-{slug}-v0"), json!({
                "kind": "built_structure",
                "structure_kind": "route",
                "provenance": "built",
                "name": format!("Voie du Forum vers le {nickname}"),
                "way_kind": "approach",
                "portfolio_priority": priority,
                "from": "place:l2:lumina-prime:endgame-forum-v0",
                "to": format!("space:l2:lumina-prime:district-{slug}-v0"),
                "note": "Le rang de priorité est une PROPRIÉTÉ de la voie, pas une position : il dit dans quel ordre le portefeuille veut apprendre, pas où le quartier se dessine."
            }))),
            subject: route,
            region: Some(city),
            region_predicate: route_region_predicate.clone(),
            connection_predicate: route_connection_predicate.clone(),
            connections: vec![(next_r(), forum), (next_r(), key)],
            moment: next_e(),
            justification: next_e(),
            rel_part_of: next_r(),
            rel_produces: next_r(),
            rel_grounds: next_r(),
            statement: format!(
                "Cette voie rend le quartier « {nickname} » atteignable depuis le forum. Elle porte son rang {priority} comme donnée, et ses deux extrémités sont des NŒUDS, jamais des points."
            ),
        });
    }

    // 5. The dependency ways: district <-> district, transcribed from the
    //    portfolio's own project-to-project edges. The Code district gets none —
    //    nothing depends on it, and that silence is the honest rendering of a
    //    frozen endgame.
    for (from, to, upstream, why) in WAYS {
        let route = next_e();
        plan.push(Construction {
            label: format!("dependency way {from} -> {to} ({upstream})"),
            structure_content: Some((format!("route:l2:lumina-prime:{from}-to-{to}-v0"), json!({
                "kind": "built_structure",
                "structure_kind": "route",
                "provenance": "built",
                "name": format!("Voie de dépendance {from} → {to}"),
                "way_kind": "dependency",
                "upstream_predicate": upstream,
                "upstream_predicate_note": "Étiquette du graphe source, portée comme DONNÉE. CONTRIBUTES_TO n'est pas un symbole canonique ici et n'est jamais interné ; l'arête écrite est COMMUNICATES.",
                "from": format!("space:l2:lumina-prime:district-{from}-v0"),
                "to": format!("space:l2:lumina-prime:district-{to}-v0"),
                "justification_upstream": why
            }))),
            subject: route,
            region: Some(city),
            region_predicate: route_region_predicate.clone(),
            connection_predicate: route_connection_predicate.clone(),
            connections: vec![(next_r(), district_key[from]), (next_r(), district_key[to])],
            moment: next_e(),
            justification: next_e(),
            rel_part_of: next_r(),
            rel_produces: next_r(),
            rel_grounds: next_r(),
            statement: why.into(),
        });
    }

    // ---- Commit each construction as ONE atomic set. -----------------------
    println!("\n{} constructions to commit\n", plan.len());
    for construction in &plan {
        commit_construction(&store, &mut snapshot, construction)?;
    }

    // ---- INDEPENDENT readback from a fresh reopen. -------------------------
    let fresh = UniverseStore::open(&store_dir)?;
    let after = fresh.replay(fresh.load_snapshot()?)?;
    println!("\n-- independent readback (fresh reopen) --");
    println!(
        "revision advanced: {} -> {} | entities: {} | relations: {}",
        start_revision.0,
        after.revision.0,
        after.entities.len(),
        after.relations.len()
    );

    let pred = |name: &str| -> Result<u32, Box<dyn Error>> {
        after
            .symbol_id(name)
            .ok_or_else(|| format!("predicate `{name}` absent from the symbol table").into())
    };
    let part_of = pred("PART_OF")?;
    let communicates = pred("COMMUNICATES")?;
    let produces = pred("PRODUCES")?;
    let grounds = pred("GROUNDS")?;
    let has = |s: u128, p: u32, t: u128| {
        after
            .relations
            .iter()
            .any(|x| x.source == EntityKey(s) && x.target == EntityKey(t) && x.predicate == p)
    };

    // The city is a ROOT region: it must be PART_OF nothing.
    if after
        .relations
        .iter()
        .any(|x| x.source == EntityKey(city) && x.predicate == part_of)
    {
        return Err("the city carries a PART_OF edge — a root region is PART_OF nothing".into());
    }
    println!("city {city:#x}: present, PART_OF nothing (root region, region_kind known_absent)");

    if !has(forum, part_of, city) {
        return Err("forum PART_OF city missing on readback".into());
    }
    println!("forum {forum:#x}: PART_OF city");

    for (slug, priority, nickname) in DISTRICTS {
        let key = district_key[slug];
        if !has(key, part_of, city) {
            return Err(format!("district {slug} PART_OF city missing on readback").into());
        }
        // Degree in inter-district ways = the district's readable neighbourhood.
        let ways: Vec<&str> = WAYS
            .iter()
            .filter(|(f, t, _, _)| *f == slug || *t == slug)
            .map(|(f, t, _, _)| if *f == slug { *t } else { *f })
            .collect();
        println!(
            "district {slug:<24} {priority}  PART_OF city | {nickname:<16} | ways to: {}",
            if ways.is_empty() {
                "(none — at the edge)".to_string()
            } else {
                ways.join(", ")
            }
        );
    }

    // Every built structure and every placement edge, and NO coordinate anywhere.
    let mut structures = 0usize;
    for construction in &plan {
        if !has(construction.moment, produces, construction.subject) {
            return Err(format!("{}: Moment PRODUCES subject missing", construction.label).into());
        }
        if !has(construction.justification, grounds, construction.subject) {
            return Err(format!("{}: justification GROUNDS subject missing", construction.label).into());
        }
        if construction.region.is_some() && !has(construction.subject, part_of, construction.region.unwrap()) {
            return Err(format!("{}: PART_OF region missing", construction.label).into());
        }
        for (_, node) in &construction.connections {
            if !has(construction.subject, communicates, *node) {
                return Err(format!("{}: COMMUNICATES anchor missing", construction.label).into());
            }
        }
        if construction.structure_content.is_some() {
            structures += 1;
            let entity = after
                .entities
                .iter()
                .find(|x| x.key == EntityKey(construction.subject))
                .ok_or_else(|| format!("{}: structure absent on readback", construction.label))?;
            let content = fresh.read_content(
                entity
                    .content
                    .as_ref()
                    .ok_or_else(|| format!("{}: structure has no content", construction.label))?,
            )?;
            for coord in ["x", "y", "z", "built_position", "path", "boundary", "position"] {
                if content.pointer(&format!("/{coord}")).is_some()
                    || content.pointer(&format!("/content/{coord}")).is_some()
                {
                    return Err(format!(
                        "{}: structure carries a position field `{coord}` — WHERE is a projection, not a datum",
                        construction.label
                    )
                    .into());
                }
            }
        }
    }
    println!(
        "\nall {} constructions read back: {structures} new Built structures, every Moment PRODUCES its subject, \n\
         every justification GROUNDS it, every placement edge present, and NO structure carries a coordinate field.",
        plan.len()
    );

    if after.symbols.len() != start_symbols {
        return Err(format!(
            "symbol table grew {} -> {} — a non-canonical symbol was interned",
            start_symbols,
            after.symbols.len()
        )
        .into());
    }
    println!("ZERO new symbols interned (symbol table stayed at {start_symbols}).");
    println!(
        "\nRESULT: the endgame quarter of Lumina Prime is BUILT — 1 root city, 1 forum, 5 placed districts,\n\
         {} approach ways and {} dependency ways, each committed as one atomic set through the generic\n\
         translator, placement carried ENTIRELY by relations, proven by independent readback.",
        DISTRICTS.len(),
        WAYS.len()
    );
    Ok(())
}

/// Index the snapshot's entities by their stored `canonical_id`.
fn index_by_canonical_id(
    store: &UniverseStore,
    snapshot: &UniverseSnapshot,
) -> Result<BTreeMap<String, u128>, Box<dyn Error>> {
    let mut index = BTreeMap::new();
    for entity in &snapshot.entities {
        let Some(reference) = entity.content.as_ref() else {
            continue;
        };
        let Ok(content) = store.read_content(reference) else {
            continue;
        };
        if let Some(id) = content.get("canonical_id").and_then(Value::as_str) {
            index.insert(id.to_string(), entity.key.0);
        }
    }
    Ok(index)
}

fn resolve(snapshot: &UniverseSnapshot, symbol: &str) -> Result<u32, Box<dyn Error>> {
    snapshot
        .symbol_id(symbol)
        .ok_or_else(|| format!("symbol `{symbol}` is not canonical (would need interning)").into())
}

/// Translate one one-verb step through the GENERIC translator and collect its
/// single command.
fn translate_step(
    store: &UniverseStore,
    base_revision: Revision,
    plan: &MutationPlan,
    proposal: &Value,
    commands: &mut Vec<UniverseCommand>,
) -> Result<(), Box<dyn Error>> {
    let write_set = translate_mutation_proposal(
        plan,
        proposal,
        store,
        base_revision,
        "construction:quarter-step:v0".into(),
    )?;
    if write_set.commands.len() != 1 {
        return Err(format!(
            "translator produced {} commands, expected 1",
            write_set.commands.len()
        )
        .into());
    }
    commands.extend(write_set.commands);
    Ok(())
}

/// Furnish + prepare + commit ONE construction as a single atomic write-set.
fn commit_construction(
    store: &UniverseStore,
    snapshot: &mut UniverseSnapshot,
    c: &Construction,
) -> Result<(), Box<dyn Error>> {
    let canonical = ["PART_OF", "COMMUNICATES", "PRODUCES", "GROUNDS"];
    if let Some(predicate) = &c.region_predicate {
        if !canonical.contains(&predicate.as_str()) {
            return Err(format!("non-canonical region predicate `{predicate}` refused").into());
        }
    }
    if let Some(predicate) = &c.connection_predicate {
        if !canonical.contains(&predicate.as_str()) {
            return Err(format!("non-canonical connection predicate `{predicate}` refused").into());
        }
    }
    // A kind whose region_predicate is null furnishes NO PART_OF step; it never
    // defaults to a dangling one.
    match (&c.region_predicate, c.region) {
        (None, Some(_)) => return Err("region given for a kind with no region_predicate".into()),
        (Some(_), None) => return Err("kind has a region_predicate but no region was given".into()),
        _ => {}
    }
    if !c.connections.is_empty() && c.connection_predicate.is_none() {
        return Err("connections furnished but the kind has no connection_predicate".into());
    }
    if let Some((_, content)) = &c.structure_content {
        for coord in ["x", "y", "z", "built_position", "path", "boundary", "position"] {
            if content.get(coord).is_some() {
                return Err(format!(
                    "structure carries a forbidden position field `{coord}` — placement is relational"
                )
                .into());
            }
        }
    }

    let base_revision = snapshot.revision;
    let sym_space = resolve(snapshot, "space")?;
    let sym_moment = resolve(snapshot, "moment")?;
    let sym_rationale = resolve(snapshot, "design_rationale")?;
    let pred_produces = resolve(snapshot, "PRODUCES")?;
    let pred_grounds = resolve(snapshot, "GROUNDS")?;

    // No key may overwrite an existing entity.
    let mut new_keys = vec![c.moment, c.justification];
    if c.structure_content.is_some() {
        new_keys.push(c.subject);
    }
    for key in &new_keys {
        if snapshot.entities.iter().any(|x| x.key == EntityKey(*key)) {
            return Err(format!("entity key {key:#x} already exists — refusing to overwrite").into());
        }
    }

    let mut commands = Vec::new();

    // 1. put_entity structure (only when this construction BUILDS one).
    if let Some((canonical_id, content)) = &c.structure_content {
        translate_step(
            store,
            base_revision,
            &MutationPlan::PutEntity {
                key: EntityKey(c.subject),
                generation: 0,
                symbol: sym_space,
                content_field: Some("content".into()),
            },
            &json!({ "content": {
                "canonical_id": canonical_id,
                "node_type": "space",
                "subtype": content.get("structure_kind").cloned().unwrap_or(Value::Null),
                "content": content
            }}),
            &mut commands,
        )?;
    }

    // 2. put_relation subject PART_OF region — the primary placement statement.
    if let (Some(predicate), Some(region)) = (&c.region_predicate, c.region) {
        let pred_part_of = resolve(snapshot, predicate)?;
        translate_step(
            store,
            base_revision,
            &MutationPlan::PutRelation {
                key: RelationKey(c.rel_part_of),
                generation: 0,
                source: EntityKey(c.subject),
                target: EntityKey(region),
                predicate: pred_part_of,
                content_field: None,
            },
            &json!({}),
            &mut commands,
        )?;
    }

    // 2b. put_relation subject COMMUNICATES anchor, one per endpoint NODE.
    if let Some(predicate) = &c.connection_predicate {
        let pred_connect = resolve(snapshot, predicate)?;
        for (rel_key, node) in &c.connections {
            translate_step(
                store,
                base_revision,
                &MutationPlan::PutRelation {
                    key: RelationKey(*rel_key),
                    generation: 0,
                    source: EntityKey(c.subject),
                    target: EntityKey(*node),
                    predicate: pred_connect,
                    content_field: None,
                },
                &json!({}),
                &mut commands,
            )?;
        }
    }

    // 3. put_entity construction Moment + 4. Moment PRODUCES subject.
    translate_step(
        store,
        base_revision,
        &MutationPlan::PutEntity {
            key: EntityKey(c.moment),
            generation: 0,
            symbol: sym_moment,
            content_field: Some("content".into()),
        },
        &json!({ "content": {
            "canonical_id": format!("moment:l2:lumina-prime:construction:{}", c.subject),
            "node_type": "moment",
            "subtype": "construction",
            "content": {
                "kind": "construction",
                "authored_by": AUTHOR,
                "requested_by": "session claude:f4fe0426-7af4-42ff-954e-e2519a16ba4c",
                "intent": format!("bâtir : {}", c.label),
                "base_revision": base_revision.0,
                "gesture": "Construction toolkit v0, set atomique, placement relationnel (aucune coordonnée)"
            }
        }}),
        &mut commands,
    )?;
    translate_step(
        store,
        base_revision,
        &MutationPlan::PutRelation {
            key: RelationKey(c.rel_produces),
            generation: 0,
            source: EntityKey(c.moment),
            target: EntityKey(c.subject),
            predicate: pred_produces,
            content_field: None,
        },
        &json!({}),
        &mut commands,
    )?;

    // 5. put_entity justification + 6. justification GROUNDS subject.
    translate_step(
        store,
        base_revision,
        &MutationPlan::PutEntity {
            key: EntityKey(c.justification),
            generation: 0,
            symbol: sym_rationale,
            content_field: Some("content".into()),
        },
        &json!({ "content": {
            "canonical_id": format!("justification:l2:lumina-prime:construction:{}", c.subject),
            "node_type": "design_rationale",
            "subtype": "justification",
            "content": { "kind": "justification", "statement": c.statement }
        }}),
        &mut commands,
    )?;
    translate_step(
        store,
        base_revision,
        &MutationPlan::PutRelation {
            key: RelationKey(c.rel_grounds),
            generation: 0,
            source: EntityKey(c.justification),
            target: EntityKey(c.subject),
            predicate: pred_grounds,
            content_field: None,
        },
        &json!({}),
        &mut commands,
    )?;

    let count = commands.len();
    let write_set = UniverseWriteSet {
        base_revision,
        idempotency_key: format!("construction:lumina-quarter:{}", c.label),
        commands,
    };
    let transaction = UniverseTransaction::prepare(snapshot, write_set)?;
    let boundary_tick = Tick(snapshot.tick.0 + 1);
    let receipt = transaction.commit(store, snapshot, boundary_tick)?;
    match receipt {
        universe_transactions::CommitReceipt::Committed { revision, .. } => println!(
            "  built {:<46} {count:>2} one-verb steps -> revision {}",
            c.label, revision.0
        ),
        other => return Err(format!("{}: unexpected receipt {other:?}", c.label).into()),
    }
    Ok(())
}

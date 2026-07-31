# Agent backlog — physicalization + « built, pas generated »

Brief de tâches pour agents (coding agents sur ce repo). Chaque tâche est **auto-portante** : contexte, fichiers/ancres, périmètre, acceptation, dépendances, parallélisable ou non. Prends une tâche, respecte les règles §0, livre avec preuve (build + test + readback).

---

## 0. Règles valables pour TOUTE tâche (non négociable)

- **Graph-first (CLAUDE.md)** : comprendre l'autorité existante → concevoir en nodes/relations → ChangeSet autorisé → valider → matérialiser → **readback indépendant**. Ne PAS éditer un fichier matérialisé comme chemin principal ; un fichier est une projection d'une autorité graphe.
- **Honnêteté épistémique** : distingue `observed / measured / built / derived / known_absent / unknown / not_measured / measurement_failed`. Ne convertis jamais une preuve absente en succès, un process qui tourne en santé, un fichier en preuve de déploiement. « Le brouillard reste brouillard ».
- **Frontière kernel** : rien ne crée un 5ᵉ verbe d'écriture. Tout se compile vers les 4 : `InternSymbols / PutEntity / PutRelation / TombstoneRelation` (`crates/universe-transactions/src/lib.rs:12`). Aucun `match` Rust ne dispatche sur une string de vocabulaire d'ontologie.
- **Preuve = readback** : après commit/matérialisation, rouvre à froid (`UniverseStore::open` + `replay`) et relis ; n'affirme jamais « vert » sans exécution observée. Colle le résumé `cargo test` / la sortie du bin dans ton rapport.
- **Périmètre borné** : ne touche que les fichiers de ta tâche. Si le workspace a une erreur de compile préexistante non liée, **signale-la** au lieu de « réparer » du code hors périmètre.

---

## 1. État courant (fait / en cours) — ne pas refaire

- **Store** : réinitialisé au kernel ontologique seul dans `artifacts/ontology-registry/current/store` ; ancien monde gelé dans `.../legacy`. `artifacts/` est **gitignore** (legacy = seule copie). Reproduire un current frais : `target/debug/canonical_ontology.exe <abs>/artifacts/ontology-registry/current`.
- ✅ **Inc 1** `crates/universe-e2e/src/bin/place_built_position.rs` — 1er fait *Built* posé sur le store vivant (position + Moment de construction + justification, set atomique), relu à froid, check anti-falsification.
- ✅ **Inc 2** `crates/universe-assets/src/layout.rs` — `override_with_built(&mut [PlacedNode], &BTreeMap<EntityKey,[f64;3]>)` : Built bat Derived. 2 tests lib verts.
- ✅ **Inc 2b** `crates/universe-e2e/src/bin/built_layout_demo.rs` — chaîne end-to-end sur le vrai store (lit `HAS_POSITION`→`built_position`→`{x,y,z}`, override, Built gagne).
- 🔄 **Inc 3** (agent worktree en cours) — traducteur générique : `MutationCommandKind` (enum fermé 4 verbes) + `translate_mutation_proposal` dans `crates/universe-e2e/src/mutation_translate.rs`, remplace `translate_fixture_proposal` (`crates/universe-e2e/src/lib.rs:~387`).
- 🔄 **Toolkits** (ChatGPT, externe) — 6 kits d'échelle depuis 3 briefs (toolkits-brief, construct-contract, atom-vocabulary).

---

## 2. Write-path (MutationBond)

### T-W1 — Mirror complet projection + materialize (GROS)
- **But** : le MutationBond graph-natif complet, miroir de `materialize_behavior_bond`.
- **Fichiers** : `crates/universe-ir/src/lib.rs` (types `Mutation*` mirror de `Behavior*` ~:548-732), `crates/universe-compiler/src/lib.rs` (`materialize_mutation_bond`, `compile_mutation_bond`, `RuntimeMutationArtifact::verify` — mirror ~:1263), lecture dans `crates/universe-e2e`.
- **Périmètre** : projection gatée sur `Epistemic::Measured + complete`, 3 hashes content-addressed (projection/mutation/artifact), reuse `Epistemic`/`BehaviorAuthority`/`BehaviorBudgets`.
- **Acceptation** : `cargo build`+`cargo test` verts ; un artifact matérialisé se `verify()` ; un cas négatif (read non-mesuré → Rejected).
- **Dépend de** : Inc 3 mergé. **Parallélisable** : non (touche ir+compiler, base de tout le reste write-path).

### T-W2 — Fixture d'autorité MutationBond + activation
- **But** : bootstrapper le 1er MutationBond via le chemin d'autorité générique (chicken-egg).
- **Fichiers** : nouveau `fixtures/ontology/mutation-bond-authority.json` (mirror de `fixtures/ontology/behavior-bond-authority.json`, clés disjointes ~`0x4000/0x7000/0x7100`) ; test/bin utilisant `install_authority_fixture` + `open_authority_store` (`crates/universe-testkit/src/lib.rs:299,471`).
- **Note clé (vérifiée)** : ça active via le loader `OntologyRegistry::load` générique **sans une ligne de code registry** (générique sur `definition_kind`). L'entité qui active = une entité `content.kind:"ontology_changeset"`.
- **Acceptation** : `activate_authority` (bin) accepte la fixture ; readback indépendant montre `active_change_sets` non vide + les nouveaux semantic_types/predicates chargés.
- **Dépend de** : rien. **Parallélisable** : oui (fixture + activation, disjoint du code Rust write-path).

### T-W3 — Geste wieldable dynamique (Propose → translate)
- **But** : un programme IR pose une position au *runtime* — `MakeRecord`→`Propose` émet la proposition, `translate_mutation_proposal` (Inc 3) produit le write-set.
- **Fichiers** : un bin/test dans `crates/universe-e2e` ; réf `crates/universe-supervisor/src/lib.rs:~775` (Propose/WriteProposal), `crates/universe-ir/src/lib.rs` (Operator).
- **Acceptation** : un programme IR + proposition {x,y,z} → 1 `PutEntity` commité ; readback = position Built posée par le geste (pas un write-set hand-built).
- **Dépend de** : Inc 3. **Parallélisable** : après Inc 3.

### T-W4 — Formaliser `built_position` comme semanticType
- **But** : `built_position` (+ `HAS_POSITION`, `CONSTRUCTED_BY`, `JUSTIFIED_BY`) déclarés comme vrais semantic_types/predicates dans un `ontology_changeset`, pas de simples symboles internés.
- **Fichiers** : fixture d'extension d'ontologie (`fixtures/ontology/...`) + readback via `OntologyRegistry::load`.
- **Acceptation** : `built_position` apparaît comme semantic_type enregistré (plus un « type non-enregistré ») ; readback confirme.
- **Dépend de** : idéalement T-W2 (même mécanique). **Parallélisable** : oui.

---

## 3. Pivot layout-built

### T-L1 — Ville qui honore le Built (materializer)
- **But** : le desktop rend la position Built (aujourd'hui il régénère).
- **Fichiers** : `apps/mind-desktop/scripts/materialize-ontology-registry.mjs` (lit le store `content-0.jsonl`) — lire `HAS_POSITION`→`built_position`→`{x,y,z}` et écraser la position dérivée dans `ontology-registry.viz.json` (miroir JS de `override_with_built`).
- **Acceptation** : re-run du materializer → le node placé apparaît à sa position Built dans le viz.json ; les autres restent dérivés.
- **Dépend de** : rien (Inc 1+2b posent la donnée). **Parallélisable** : oui (JS, disjoint du Rust).

### T-L2 — Pin-during-solve (landmark)
- **But** : un node Built est *fixe pendant* le force-directed (les voisins s'arrangent autour), pas seulement écrasé après.
- **Fichiers** : `crates/universe-assets/src/layout.rs` (`layout_graph`/`compute_city` — exclure les nodes Built de la relaxation, les ancrer).
- **Acceptation** : test montrant qu'un voisin d'un node Built se positionne différemment avec vs sans pin.
- **Dépend de** : Inc 2. **Parallélisable** : oui, mais **même fichier que Inc 2** → sérialiser avec toute autre tâche layout.rs.

---

## 4. Toolkits (kits de représentation)

### T-K1 — Toolkit **validator loop** (construct)
- **But** : un construct qui **valide un catalogue de kit** (produit par ChatGPT) contre le schéma `visual-embodiment/1` + le contrat des primitives, et produit `validation_run` + `health_assessment`. C'est l'**observer** des toolkits.
- **Contrat des primitives** (`crates/universe-assets/src/visual.rs`) : `ALLOWED_PRIMITIVES = [icosphere, sphere, capsule, points, fresnel_shell]`, tuple **arité 8** `[kind, part, palette_role, offset, rotation, scale, particle_count, particle_size]`, `primitive_budget ≤ 12`, `particle_budget`, `lod_states` couvre dormant/aggregated/sleeping/hot, `dynamics` bornées `[inMin,inMax,outMin,outMax]`.
- **Loop** : voir `construct-contract.md` — objective/behavior(GIVEN/WHEN/THEN + interdits)/observer/metric/health/maintenance. Résultats interdits à détecter : primitive hors set, tuple non-arité-8, budget dépassé, énergie non-mesurée affichée, position inventée pour signal absent.
- **Fichiers** : nouveau module (Rust `universe-assets` a déjà la validation `visual.rs` — réutiliser/étendre) ; ou un validateur graph-natif.
- **Check de TOTALITÉ / couverture (prioritaire)** : un kit doit être une **fonction totale sur son rayon** — pour CHAQUE configuration de son domaine déclaré, il émet **exactement une instruction définie** (rendu ET affordance), zéro trou. Le rayon est un produit énumérable : `roleAxis(5) × semanticTypes déclarés × lod_state(4) × état-signal{energy,weight,embedding}∈{measured,absent,not_measured} × familles-de-relation{énergisante,inhibitrice,fork,answer,structurelle,neutral}` [× bandes d'état atom{sous-seuil,au-seuil,firé,inhibé,starved} pour le kit énergétique]. Énumérer ce produit, exécuter le mapping du kit, exiger une instruction définie ; `coverage = définis/|rayon|` ; tout trou = échec nommé. Le kit DOIT déclarer son rayon (sinon « toutes les configs » est indéfini) ; les configs hors-rayon sont explicitement out-of-scope, jamais droppées. Totalité ≠ invention : la config `not_measured` a une instruction définie = **identité/brouillard** (un trou est aussi fautif qu'une valeur fabriquée).
- **Acceptation** : (a) rejette au moins 5 catalogues structurellement invalides (un par règle) ; (b) rejette un kit avec un **trou de couverture délibéré** (un semanticType sans mapping) en nommant la config manquante ; (c) accepte le catalogue réel `fixtures/assets/visual-embodiment-catalog.json` (structure) ; l'observer ne convertit jamais un check manquant en succès.
- **Dépend de** : rien. **Parallélisable** : oui. *(NB : l'utilisateur conçoit cette loop — attendre son go pour la forme graphe. v0 pressenti : validator structurel + totalité partagé, bâti sur `visual.rs` ; render-honesty et per-kit-observer en cran 2.)*

### T-K2 — Matérialiser les catalogues de kits ChatGPT
- **But** : intégrer chaque catalogue de kit produit par ChatGPT et le valider par le moteur.
- **Fichiers** : `fixtures/assets/` (nouveaux catalogues) ; `crates/universe-assets/src/visual.rs` (`load`/validation) ; route de preview `apps/mind-desktop`.
- **Acceptation** : chaque catalogue passe la validation `visual.rs` ; une route `?fixture=<kit>` le rend sans erreur console.
- **Dépend de** : ChatGPT livre les catalogues + T-K1 (validator). **Parallélisable** : oui.

### T-K3 — Lifter le sélecteur (post-its ‖ cubes)
- **But** : plusieurs catalogues de représentation coexistent par store (aujourd'hui `materialize()` hardcode UN catalogue + atom-ids `0x7000/0x7010/0x7011`).
- **Fichiers** : `crates/universe-assets/src/visual.rs:48,359`, `apps/mind-desktop/src/entity-dynamics.ts`.
- **Acceptation** : deux catalogues distincts sélectionnables pour le même graphe ; test montrant le même node rendu par deux kits.
- **Dépend de** : rien. **Parallélisable** : oui (mais même fichier `visual.rs` que T-K2 → coordonner).

---

## 5. Questions de design — PAS pour agents autonomes (nécessitent l'humain)

- **Opérateur de transition entre crans d'échelle** : agrégation (mille micro-circuits → un post-it) vs re-kit (mêmes nodes, grammaire plus grossière) ? Non tranché.
- **Décorateur de physicalisation** : forme de `node → corps + affordance` (avec justification). Design en cours avec l'utilisateur.
- Ces items = session de design, pas exécution autonome.

---

## 6. Carte de parallélisation (qui peut tourner ensemble sans conflit)

| Groupe | Tâches | Crates/fichiers | Conflit ? |
|---|---|---|---|
| A | T-W2, T-W4 | `fixtures/ontology/` + testkit (lecture) | non entre eux |
| B | T-L1 | `apps/mind-desktop/scripts/*.mjs` (JS) | disjoint du Rust |
| C | T-K1 | validation (nouveau) | disjoint |
| D | Inc 3 → T-W1 → T-W3 | `universe-ir`, `universe-compiler`, `universe-e2e` | **sérialiser** (chaîne write-path) |
| E | T-L2, T-K2, T-K3 | `universe-assets/src/{layout,visual}.rs` | **sérialiser** les tâches d'un même fichier |

Règle worktree : un agent qui mute des fichiers tourne en **worktree isolé** ; merge par fichiers disjoints. Deux tâches du même fichier (`layout.rs`, `visual.rs`) ne partent pas en parallèle.

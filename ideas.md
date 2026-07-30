# ideas.md — le monde du Cloître, en vrac

> Scratchpad d'idéation, **non-autoritatif**. Ce fichier n'est pas le graphe et ne
> gouverne rien. Il capture la vision pour qu'on puisse ensuite **se recentrer**.
> Tags : **[RÉEL]** = code qui tourne dans le repo · **[SUBSTRAT]** = primitif
> existant, pas encore branché · **[VISION]** = rêvé, zéro ligne derrière.

---

## 0. Le théorème (ce qui range tout)

Tout ce qu'on a listé — board, portal gun, board shop, mall, centre-ville, usine —
se réduit aux **5 rôles du roleAxis** (canonical-ontology, l.2254-2289), définis
par ce qu'un node fait de l'énergie :

| rôle | nodeType | fonction énergie | ce que ça devient dans le monde |
|---|---|---|---|
| **pompe** | `actor` | injecte | toi (le rider), le Citizen |
| **passage** | `moment` | transmet/absorbe | les messages, les positions |
| **attracteur** | `narrative` | retient | questions, décisions, croyances |
| **conteneur** | `space` | **borne la diffusion** | shop, mall, centre-ville, **usine** |
| **routeur** | `thing` | **laisse passer en transformant** | board, portal gun, **tous les tools** |

Conséquence : **un objet magique = un `thing`. Un lieu = un `space`.** Objet et
outil MCP sont le même node. On n'a pas dix systèmes, on a **un patron** (le
`magic_object` blueprint) itéré. Le décorateur qu'on a codé fabrique déjà ça.

---

## 1. Ce qui est déjà debout (vérifié)

- **[RÉEL]** App 3D Tauri + React + Three.js : `apps/mind-desktop` (`World.tsx`,
  `EnergyEmbodiment.tsx`, `EnergyTransfer.tsx`, `ObserverControls.tsx`, `src-tauri`).
- **[RÉEL]** Spreading-activation sur atoms : `universe-physics::AtomDynamics`.
- **[RÉEL]** Relations → joints Rapier (l'« OntologyPhysicsMapper ») :
  `universe-physics::map_relation_physical_delta` → `SpringJointBuilder`.
- **[SUBSTRAT]** `TopologicalFold` (le pli = le portal gun) : `universe-fields`.
- **[SUBSTRAT]** Tools = capabilities : `CapabilityCall`, `required_capabilities`.
- **[RÉEL]** Le blueprint + décorateur + board : `crates/universe-e2e/src/magic_object.rs`,
  `board.rs`, `fixtures/atoms/board-descent-v0.json`.
- **[RÉEL]** Ontologie canonique : 231 entités / 784 relations, `validated_with_explicit_gaps`.
- **[RÉEL] L'USINE existe déjà** : `crates/universe-postgres-import` — loop curseur
  Postgres résumable (batches bornés, watermark, prepare→readback→finalize, conflits),
  imports **inertes** (Assets + relations), + pilotes ontologie/code. **NE PAS réinventer.**

**[VISION]** (roadmap dans TODO/README/AGENTS, pas exécutable) : 6 FlowActors,
`continuous_inference_runtime`, 10 états mémoire / PROTECTING, scores
entropie/tension, `ForceLaw`, la couche audio liturgique.

---

## 2. Catalogue des OBJETS (`thing` — routeurs)

Chaque objet = quels prédicats il **énergise** (support) / **inhibe**, le geste,
le feel. Prédicats pris du store réel.

| objet | énergise / inhibe | geste | feel | statut |
|---|---|---|---|---|
| **Hoverboard** (Intensité) | `LEADS_TO`·`CAUSES`·`salience` | carver, freiner | plonge vers les pics chauds | **[RÉEL]** |
| **Board Causalité** | `DEPENDS_ON`·`CAUSES` | — | pente propre, linéaire, métronome | **[SUBSTRAT]** |
| **Board Tension** | attire vers `CONTRADICTS`·gaps | — | aspiré par le vide, vibrations max | **[SUBSTRAT]** |
| **Board Affinité** | proximité d'embedding | — | glisse doux vers le semblable | **[VISION]** (embeddings) |
| **Lanterne** | rien — *révèle* `observed`/`speculative`/Fog | lever | éclaire le statut épistémique, sans bouger | **[SUBSTRAT]** ← le moins cher |
| **Boussole** | gradient vers `open_question`/but | pointer | l'aiguille tire vers le non-résolu | **[SUBSTRAT]** |
| **Portal Gun** | crée un `TopologicalFold` bidirectionnel | tirer 2 portails | rapproche 2 Spaces, irrigue le froid | **[SUBSTRAT]** |
| **Grappin** | ressort `DEPENDS_ON` sur max-embedding | tirer | t'arrache au sol, survol | **[VISION]** |
| **Grenade à Gaps** | force d'attraction sphérique (`ForceLaw`) | lancer | lentille gravitationnelle, force un cluster | **[VISION]** |
| **Aegis / Bouclier** | damping géant (→ PROTECTING) | activer | dôme, silence de cathédrale, calme | **[VISION]** |
| **Prisme** | `split` de contexte co-actif | traverser | divise la trajectoire, apaise un souvenir | **[VISION]** |
| **Tombstone / Ancre** | fige le tick, brise la boucle | larguer | stoppe une rumination | **[VISION]** |

### Nouveaux (go nuts)
- **Diapason** — résonance d'affinité : fait *venir à toi* les souvenirs parents (`REINFORCES`).
- **Clé** — ouvre un `decision` gate ; rend une branche verrouillée franchissable.
- **Cloche** — `MAKES_SALIENT`·`RECRUITS` : sonne une région, recrute l'attention.
- **Filet** — capture un cluster local et le referme en **nouveau `space`** (crée une pièce).
- **Aimant** — `INCREASES_PROPENSITY` sur un mécanisme : renforce ce qui a marché.
- **Burin** — épingle un node et ouvre un ChangeSet (annoter, éditer) — l'outil d'auteur.
- **Fil d'Ariane** — dépose une trace re-traçable ; le télésiège vers l'origine d'une idée.
- **Graine** — plante une `narrative` (but) qui *creuse une pente vers elle* en grandissant.
- **Miroir** — superpose la trace-fantôme du Citizen à la tienne.
- **Balai / GC** — compacte les `superseded`/`refuted` (maintenance, la « guillotine des chœurs »).
- **Métronome** — règle la cadence des ticks d'inférence (le kick à 40Hz).
- **Gomme** — marque `refuted` (⚠ épistémique : jamais silencieux, toujours attribué).

---

## 3. Catalogue des LIEUX (`space` — conteneurs)

Un lieu = un `space` qui diffère par son **contenu** et sa **policy**.

- **Le Cloître / L1** — le Space-maison : le graphe cognitif personnel entier.
- **Atelier du Shaper (Board Shop)** — Space qui **contient des `thing`** (les blueprints
  de boards) ; changer de planche = changer la question posée au souvenir. **← Space-de-things.**
- **Centre commercial (Mall)** — Space d'affordances : des `thing`/tools cueillables,
  sensors de proximité, capacités temporaires. « Planification = shopping cognitif. »
- **Centre-ville (Core Hub)** — Space dense, haute stabilité : identité, croyances
  sédimentées, gravité Φ colossale et stable. Chœurs en accords résolus.
- **Zone industrielle (l'Usine)** — voir §5. Sous-Spaces :
  - Fonderie physique (SceneCompiler → RigidBodies/Joints Rapier). **[SUBSTRAT]**
  - Raffinerie Graph IR (compiler + VM → bytecode par hash). **[RÉEL]**
  - Ligne d'assemblage ChangeSets (transactions atomiques, CAS). **[RÉEL/SUBSTRAT]**
  - Gare de fret / Docks (membrane import/export, `talk`). **[VISION]**

### Nouveaux (go nuts)
- **Nécropole** — les `superseded`/`refuted` : on les visite, on ne les efface pas.
- **Observatoire** — les `metric`/health : instruments qui lisent l'état du système.
- **Serre** — les `open_question` qui poussent ; on va y jardiner les forks.
- **Tribunal** — où les `CONTRADICTS` et `decision` se résolvent (le rollback = le larsen).
- **Bibliothèque** — les `source_document` : la provenance reconstructive.

---

## 4. Tools = Things (l'unification MCP)

- Un tool = un `thing` (routeur) portant une `CapabilityCall`. **[SUBSTRAT]**
- Manier l'objet en 3D **==** appeler la capability. Même geste pour toi et le Citizen.
- Chaîne : `EffectIntent` → Capability Host (Rust) → PhysicsCommand (Rapier) →
  EffectReceipt (JSONL immuable). L'IA **lit le reçu**, ne devine pas.
- Symétrie : tu peux jeter l'Ancre sur le node de rumination du Citizen ; il peut
  tirer une Grenade pour réorganiser l'espace sous tes yeux.
- Exemples de signatures : `open_topological_fold(a,b)`, `trigger_emergency_damping(k)`,
  `split_memory_context(node)`, `inject_gravitational_singularity(locus, strength)`.

---

## 5. L'USINE — elle EXISTE : `crates/universe-postgres-import`

**Ne rien réinventer.** L'absorption des 500 conversations est déjà codée et testée :

- **Loop curseur** (`cursor.rs`) : source Postgres read-only, ordonnée `(updated_at, source_id)`,
  lue en **batches bornés** avec **watermark**, chaîne `ADVANCES_TO`. Deux phases par batch :
  `prepare` (inerte) → **readback indépendant** → `finalize` (publie le curseur + reçu).
  Résumable depuis le dernier watermark relu, idempotente, **conflits** (drift schéma/mapping/row,
  position) enregistrés **sans avancer**.
- **Pilote identité + relations** (`lib.rs`) : rows → **Assets inertes** + maps d'identité ;
  relations sources → **résolues inertes** OU **quarantaine** (endpoint inconnu). Cross-graph tracé.
- **Pilotes ontologie / code** : les **seuls** endroits qui *activent*, via ChangeSet approuvé,
  scoping source, révision épinglée.

**L'invariant de fer : l'absorption n'active RIEN.** Tout entre `executable:false`,
`ontology_activated:false`, `payload_imported:false`, `physical_mapping_activated:false`.
`source_status_activates_target:false`. Les conversations **sédimentent** en matière graphe
dormante ; elles ne tournent pas.

**Où se branche la board (corrigé, honnête).** Le pipeline est plus long que je l'avais dit :
1. **Absorption** (curseur/identité) : rows → Assets + relations **inertes**. Rien activé.
2. **Adaptation** (`ontology_pilot`) : le **vocabulaire** (node types, prédicats) reçoit un mapping
   approuvé, scopé, révision-épinglée. Reçu : `ontology_activated:true` mais **`physics_activated:false`**
   — il active le *mapping*, PAS la donnée, PAS la physique.
3. **[MANQUANT]** appliquer ce mapping aux Assets/relations réels + leur attacher un `physical_profile`
   → **là seulement** une relation devient physique (→ SpringJoint). `physical_mapping_activated` est
   **`false` partout** dans l'import : gap **déclaré et gardé**, pas un oubli.

Donc l'import Postgres **n'est pas encore ridable** et `ontology_pilot` n'est **pas** le commutateur
physique. Ce qui EST ridable aujourd'hui : le **store ontologie canonique** (231/784) — il porte déjà
des `physical_profile` (positions), lus par le vertical slice e2e. Le board est le **consommateur en aval**.

---

## 6. Le Feel (la console vivante) [VISION côté audio/rendu]

6 potards, réglés au playtest, **sans toucher au moteur** :
`k_slope` (raideur), `k_turn` (nervosité des forks), `k_brake` (friction d'arrêt),
`k_lod` (vitesse d'ouverture d'un moment), `ghost_cold` (translucidité des « et-si »),
`gate_doppler` (signature sonore des portes).

Audio (mapping rêvé) : kick = tick du daemon ; snare = joint Rapier à sa contrainte
limite ; chœur glitché = compaction mémoire (`superseded`) ; larsen 2s = rollback ;
saturation basses = friction de contradiction. **Critère de réussite** : un
utilisateur non briefé **freine spontanément devant une contradiction**, parce que
la pente + la vibration + le cri du chœur le lui demandent.

---

## 7. Météo = état système (go nuts)
- Soleil zénithal = cohérence saine. · Orage / wake storms = surcharge physique.
- Brouillard = régions non-mesurées (Fog). · Nuit = dormant. · Givre (Aegis) = PROTECTING.
- Saisons = sédimentation lente : le centre-ville « vieillit » et durcit.

## 8. Multijoueur / symétrie (go nuts)
- Toi et le Citizen partagez le plan ; les deux maniez des tools.
- Replays fantômes des traversées passées. · Co-op de régulation (tu poses l'Aegis).
- Un jour : deux Citizens de deux humains se rencontrent en L3 sur une piste partagée.

---

## 9. Pour se recentrer — les plus petites tranches RÉELLES

Rangées par (valeur × proximité du substrat). On en choisit **une**.

- **A. Board sur données réelles = bloqué par un GAP DE MODÉLISATION (vérifié).** Deux
  physiques distinctes, non connectées : (1) le `physical_profile` canonique = descripteurs
  **spatiaux** (`polarity`/`hierarchy`/`permanence`), `prototype_not_calibrated` — pour le
  layout/spring, **pas** l'énergie d'atom ; (2) `BehaviorPhysicalProfileContent`
  (threshold/seed/transfer_energy entiers) = la spreading-activation que la board ride, présente
  **uniquement dans des fixtures écrites à la main**. Aucun corpus réel (import inerte, ou
  conversations) ne porte l'énergie d'atom. Rider le réel demande soit une **dérivation
  heuristique** (canonique spatial → énergie, à marquer *dérivée/non-mesurée*), soit une **phase
  de modélisation/calibration** des profils d'énergie. Gros, hors substrat existant.
- **B. 2ᵉ objet = Portal Gun** sur le `TopologicalFold` existant. Prouve
  « tool = thing = objet magique » et branche un `thing` sur un primitif réel.
- **C. Lanterne** — révèle Fog/vécu/hypothèse *sans bouger*. Le moins cher ;
  prouve que le blueprint tient sur un objet au feel opposé (statique).
- **D. Board Shop** = un `space` qui contient plusieurs blueprints de boards.
  Prouve le « Space-de-things » et le geste « changer de question ».

Recommandation : **A** (le squelette qui donne un sens à tout le reste), ou **C**
si on veut une victoire rapide avant de creuser l'usine.

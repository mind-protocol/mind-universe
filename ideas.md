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
  - **⚠ La dérivation heuristique existe DÉJÀ mais ne suffit pas au FEEL (vérifié 2026-07-30).**
    `crates/universe-e2e/src/canonical_ride.rs` ride un vrai voisinage canonique, le carve
    redirige, déterministe, tagué `derived_uncalibrated / not_measured`, test qui passe. **MAIS**
    la membrane refuse de la streamer : `universe-protocol/src/lib.rs:218`
    (`EnergyTransferMessage::validate`) rejette tout transfert dont `epistemic != Measured`
    (« only measured transfers may be published »), verrouillé par le test
    `energy_transfer_fails_closed_without_measured_provenance_or_bounds`. Donc un carve **dérivé
    ne peut pas devenir une glisse *ressentie* sur le wire** — par design. L'adapter TS accepte
    `not_measured` et `desktop_world_snapshot` écrit des frames-fichier hors `ProtocolStream`,
    donc on *pourrait* faire passer la glisse dérivée au renderer par un fichier — **NE PAS le
    faire** : ça contourne délibérément l'invariant de la membrane (interdit par CLAUDE.md).
    Conséquence : rendre le carve *ressenti* exige de l'énergie **réellement mesurée** (calibration,
    option A « modélisation »), pas un raccourci dérivé. Le seul chemin qui streame une vraie
    glisse mesurée aujourd'hui = `desktop_energy_stream` sur un run behavior fixture, **pas** les
    conversations réelles.
  - **✅ SLICE CONSTRUITE + VÉRIFIÉE (2026-07-30) : `crates/universe-e2e/src/canonical_seed_energy.rs`.**
    Port L1 du pattern de seeding du design (`l2:physics:developer-needs-initial-seeding`) : pour
    chaque relation canonique, construit une phrase source/relation/cible, l'**embed**
    (`universe-embeddings`), calcule `raw = cos(link,+ancre) − cos(link,−ancre)`, `tanh(raw/temp)`
    → propensity ∈ (−1,1), puis **conversion d'unité** magnitude→énergie, signe→Support/Inhibit.
    Tagué `measured:semantic_v0`, **overlay séparé du canonique** (comme l'exige
    `l1-online-physical-plasticity`). Test déterministe offline (embedder de hachage) qui PASSE.
    La **seule** décision de modélisation = la paire d'ancres `activation-propensity-v0`, nommée
    et versionnée (pas smugglée).
  - **🔎 CE QUE LA MESURE RÉVÈLE (observé, pas supposé) :** sur le store canonique réel, **784
    relations → seulement ~14 phrases distinctes** (6 prédicats), ex. `source_document --GROUNDS-->
    protocol` répété à l'identique. L'embedder est fidèle (distinct_propensity == distinct_sentences) :
    **ce n'est PAS la méthode qui collapse, c'est le graphe canonique qui est *type-level*** — il ne
    porte pas de texte distinctif par instance. La résolution du terrain ridable est bornée par la
    distinctivité textuelle du canonique. **La différenciation riche (784 descentes distinctes) vit
    dans les 500 CONVERSATIONS** (contenu réel des mémoires), pas dans l'ontologie de types. →
    Prochaine étape pour un vrai relief : seeder sur le *contenu des conversations*, pas seulement
    les symboles canoniques.
  - **📈 PROUVÉ PAR MESURE (2026-07-30) : le contenu des conversations lève le plafond.** Mesuré sur
    l'export ChatGPT réel (`Downloads/…-2026-07-23-…zip`) : **1342 conversations, 17 098 messages,
    16 812 textes distincts (99,5 % uniques), 11 690 paires-de-réponse distinctes** (parent→child),
    ~1419 chars/msg. Soit **14 → 11 690 descentes ridables distinctes (×835)** vs le graphe canonique.
    Le relief riche existe bel et bien — dans le *texte vécu*, pas dans l'ontologie de types. La
    lecture est locale, en investigation (pas de wiring runtime → respecte `python:0`). Prochain
    slice Rust : `conversation_seed_energy` sur les paires-de-réponse (même pipeline embed→cosine→
    tanh→AtomBond, tagué `measured:semantic_v0`).
  - **✅ SLICE 2 CONSTRUITE + VÉRIFIÉE EN RUST (2026-07-30) :
    `crates/universe-e2e/src/conversation_seed_energy.rs` + bin `conversation_seed_energy`.** Même
    pipeline via `SeedContext` partagé (une seule définition d'ancres/maths, refactor de
    `canonical_seed_energy`). Relations = paires-de-réponse (parent→child) des conversations. 3 tests
    hermétiques (fixture synthétique, zéro texte privé committé) PASSENT + le canonique reste vert.
    **Run in-engine sur le vrai corpus** (bin, counts-only, aucun texte persisté) :
    **1342 conversations, 11 734 paires, 11 496 propensités DISTINCTES** (vs 14 canonique). Le relief
    riche est matérialisé, mesuré, déterministe. ⚠ CAVEAT honnête : le split support/inhibit
    (~50/50) reste **arbitraire au hash** — la DIFFÉRENCIATION (11 496) est réelle et indépendante de
    l'embedder, mais la POLARITÉ n'aura de sens qu'avec le vrai modèle sentence-transformers
    (`NodeTransformersProvider`). Reste : brancher le vrai embedder ; faire consommer l'overlay par
    `measured_ride`.
- **B. 2ᵉ objet = Portal Gun** sur le `TopologicalFold` existant. Prouve
  « tool = thing = objet magique » et branche un `thing` sur un primitif réel.
- **C. Lanterne** — révèle Fog/vécu/hypothèse *sans bouger*. Le moins cher ;
  prouve que le blueprint tient sur un objet au feel opposé (statique).
- **D. Board Shop** = un `space` qui contient plusieurs blueprints de boards.
  Prouve le « Space-de-things » et le geste « changer de question ».

Recommandation : **A** (le squelette qui donne un sens à tout le reste), ou **C**
si on veut une victoire rapide avant de creuser l'usine.

---

## 10. Triage qualité — 🟢 good / 🟡 average / 🔴 meh

> **Autre axe que §1.** Les tags `[RÉEL]/[SUBSTRAT]/[VISION]` mesurent la *maturité*
> (est-ce que ça tourne ?). Ici on note la **valeur d'ingénierie** de l'idée
> (est-ce que ça mérite d'être construit ?), indépendamment de si c'est codé.
> Une idée peut être `[VISION]` **et** 🟢, ou `[RÉEL]` **et** 🔴.

**La thèse en une ligne :** il y a **deux projets emmêlés**. La *couche discipline*
(graphe-source, loops, honnêteté épistémique, observers, budget de fuel) est un
système fort et constructible. La *couche feel* est un **pari UX de première classe,
à égalité** avec la discipline — mais son levier est la **focalisation**, pas
l'étendue.

### 10.1 Correctif « feel / juice » (la leçon Angry Birds)
> Angry Birds n'a aucune nouveauté architecturale et a mangé un milliard d'heures :
> **un seul verbe** (le lancer), réglé au millimètre, avec du **juice** (le cri du
> cochon, l'effondrement des blocs). Le feel n'est pas de la « saveur » secondaire —
> c'est le produit. Correction de mon triage initial, qui sous-notait l'engagement :

- **Le geste board/carve** → **🟢** (pas 🟡). C'est le lance-pierre. Si *une*
  interaction doit valoir des heures, c'est celle-là. Pari assumé, non validé.
- **Le juice** (audio, effondrement, vibration sur contradiction) → **🟡→🟢** s'il est
  câblé à un état réel. Chez Rovio la retention *est* le cri du cochon. Pas « meh ».
- **Les potards de feel** (`k_slope`, `k_turn`, `k_brake`) → Angry Birds *est*
  l'argument pour les régler obsessionnellement. Bon instinct (reste 🟡 tant que rien
  ne tourne, mais c'est la surface de tuning n°1).

**Ce que la leçon condamne au contraire (donc restent 🔴) :**
- **Les neuf autres objets magiques.** Rovio a livré *le lancer*, pas lancer + portal
  gun + grenade + prisme. Dix verbes à moitié réglés = l'inverse de la leçon.
- **La physique pour la cognition du Citizen.** Distinction clé : chez Angry Birds la
  physique simule le *jouet que le joueur lance* (légitime, on investit). Le 60 Hz
  pitché ici modélisait la *rumination/angoisse du Citizen* — la physique comme
  substrat de l'état mental d'autrui. C'est ça la sur-ingénierie, pas le board-sur-terrain.

**Thèse corrigée :** le feel est une discipline **🟢 de première classe**, à égalité
avec la couche épistémique — mais **un geste, réglé obsessionnellement, câblé à un
état honnête**, pas dix gadgets.

### 🟢 Good — le vrai projet
| idée | pourquoi |
|---|---|
| **Graphe = source, fichiers = matérialisations** | discipline réelle ; intent/comportement/preuve restent reliés. |
| **États d'honnêteté épistémique** (`observed`/`measured`/`known_absent`/`unknown`/`not_measured`/`measurement_failed`) | le joyau. Refuser d'assimiler « absent » à « zéro » ou « tourne » à « sain ». |
| **Loop = unité auto-vérifiante** (objective → observer → health → maintenance) | force chaque capacité à dire comment elle échoue et comment on l'observe. |
| **Observer indépendant** qui ne croit pas les claims de l'implémentation | attaque le mode d'échec n°1 des agents (succès auto-rapporté). |
| **1 humain ↔ 1 Citizen AI** | thèse produit claire et assumée (anti-swarm). |
| **Séparation d'autorité graphe Human/Citizen** | valeur sécurité + design ; le reframe « interface immunitaire » tient. |
| **Mémoire spatiale — rider le graphe comme un terrain** | pari UX réel pour naviguer un contexte énorme. |
| **Budget fuel first-class + out-of-fuel fail-loud** | discipline runtime : coût visible, pas de runaway. |
| **Hystérésis anti-thrashing / anti-rumination** | vrai réflexe de théorie du contrôle sur un vrai problème. |

### 🟡 Average — correct mais dérivatif ou non prouvé
| idée | réserve |
|---|---|
| **« Membrane » au lieu d'API** | surtout un gateway stateful mieux raconté. |
| **Couches L1–L4** | raisonnable, conventionnel. |
| **Hoverboard / carver les embeddings** | belle UI potentielle ; le *feel* est totalement non validé. |
| **Context Sandwiches** | joli nom pour du budget de fenêtre de contexte. |
| **State machine 10 états + formules de score** | plausible *si* ancré dans des preuves live ; aujourd'hui prose + maths. |
| **Potards de feel** (`k_slope`, `k_turn`, `k_brake`) | bon instinct (exposer les params tôt), prématuré sans loop qui tourne. |
| **Tombstone / Killswitch / Aegis** | les seuls « objets magiques » à sémantique réelle (mute node, freeze L1, backpressure). |

### 🔴 Meh — saveur vendue comme architecture, ou sur-ingénierie
| idée | verdict |
|---|---|
| **Rapier 60 Hz pour modéliser la cognition** | sur-ingénierie ; pas besoin d'un solveur de contraintes pour décider d'indexer un doc. |
| **Portal Gun / Grenade / Slingshot / Prisme / Sablier** | inflation métaphorique : ils *renomment* des ops sans ajouter de capacité. |
| **Tours corporate & prisons en 3D littérales** | saveur pure ; zéro charge architecturale au-delà de « quarantaine » + « pool de budget ». |
| **Deli / Incinérateur / Magasin de services comme échoppes** | vibe > fonction. |
| **Esthétique cyber-liturgique, 128 BPM, chœurs de cathédrale, avatar doré** | superbe mood board, pas le produit. |
| **Potards `ghost_cold` / `gate_doppler`** | prolifération de paramètres prématurée. |
| **Le twist « upload de conscience »** | puissant émotionnellement, mais c'est une *motivation*, pas un requirement — et c'est l'histoire qui justifie la sur-ingénierie ci-dessus. À manier avec soin. |

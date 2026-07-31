# ALIGN.md — le point de visualisation unique des liens

> Document de **réconciliation**. Il ne gouverne rien tant qu'il n'est pas
> matérialisé en autorité graphe (le `visual-mapping`, cf. §5). Sa fonction :
> supprimer la divergence — aujourd'hui **quatre** représentations concurrentes
> d'un lien coexistent — en fixant **une seule** décision, dont dérivent le
> renderer et la matérialisation. Étoile du nord partagée avec
> [cloitre-visual-grammar.md](cloitre-visual-grammar.md) : « cité-cathédrale, un
> node est ce qu'il fait de l'énergie ».

---

## 0. Le point unique (la thèse)

**Un lien est un conduit vivant, pas une ligne. Chacun de ses attributs canoniques
est projeté sur un canal perceptif orthogonal distinct. La géométrie et le style du
conduit se dérivent du `physical_profile` du prédicat et de l'état du lien universel
— jamais d'une taxonomie inventée dans le renderer.**

Corollaire dur : il n'existe **qu'une** table prédicat → rendu, et c'est la
projection du canonique. Toute autre table est une dette à supprimer, pas une
variante à maintenir.

---

## 1. Les quatre représentations à réconcilier (l'état divergent)

| # | Où | Ce qu'un lien y est | Verdict |
|---|---|---|---|
| A | **Canonique** `fixtures/ontology/canonical-ontology.json` | lien universel unique ; le prédicat est un `physical_profile` : `polarity [p_ab,p_ba]∈[-1,1]`, `hierarchy` signé, `permanence`, + état `weight/energy/recency/stability/gate`. **20 familles**, 65 profils. | **AUTORITÉ.** Source de vérité. |
| B | **Kernel layout** `crates/universe-assets/src/layout.rs` | consomme `polarity` (moyenne), `hierarchy`, `similarity` — mais **seulement pour placer les nœuds**, pas pour rendre le lien. | Correct mais **partiel** : lit 3 attributs, en jette la moitié, ne produit aucune géométrie de lien. |
| C | **Résolveur TS** `apps/mind-desktop/src/relation-infrastructure.ts` | 11 familles « rues » ad-hoc (`causality`, `flow`, `containment`…), style = forme urbaine ; `drapedRoute` **aplatit à y=0**. | **DETTE — désormais rétrogradée au rang de _fallback_.** Le `Bond` (`World.tsx`) dérive maintenant du canonique via `relation-infrastructure.ts::projectPhysicalProfile` — `polarity`→lumière, `hierarchy`→pente (positions `[x,y,z]` réelles, **plus d'aplatissement**), `permanence`→épaisseur, `calibrated`→opacité honnête. `infrastructureStyle`/`drapedRoute` ne servent plus que de _fallback_ quand un lien n'a **aucun** `physics` (source hors ontologie). À supprimer une fois toutes les sources profilées. |
| D | **Grammaire cloître** `cloitre-visual-grammar.md` §1 | « constellation de liens lumineux », style par `relation_family` (~10 familles). | Bonne intention, **non-autoritative**, famille-seulement (ne lit pas polarity/hierarchy/permanence). Absorbée par §2. |

Divergences nommées : **B** ignore le rendu du lien ; **C** invente une 2ᵉ taxonomie
et détruit la hauteur ; **D** s'arrête à la famille. Aucune ne lit l'attribut riche
que **A** porte déjà.

---

## 2. La table canonique unique : attribut → canal

Chaque attribut a **son** canal. Six canaux statiques + cinq dynamiques, indépendants
(ils ne se disputent pas le même pixel).

### Statique — le type (lisible même éteint)
| attribut canonique | canal | règle |
|---|---|---|
| `family` (20) | teinte de base | identité de grande intention |
| `permanence` [0,1] | **matière** : fil → câble → poutre → arche de pierre | 4 barreaux, l'échelle déjà choisie par l'ontologie (`linkQuantification.admissionNote`) |
| `hierarchy` [−1,1] | **pente** du conduit | +1 : source sous la cible (partie→tout) ; −1 : au-dessus ; 0 : horizontal. **Le lien suit la pente que le layout impose déjà — il ne l'aplatit pas.** |
| `polarity` signe | **couleur de lumière** | + excitation (cyan/or) · − inhibition (rouge/violet) |
| `polarity` asymétrie `\|p_ab\|−\|p_ba\|` | **chevrons / sens** | symétrique = double-sens ; asymétrique = quasi-unidirectionnel |
| `mode` (`axis`/`composite`/`semantic_required`) | présence de la **synthèse** « {a}…{b} » | `semantic_required` ⇒ la phrase reste affichée |

### Dynamique — l'état (la vie)
| attribut | canal | règle |
|---|---|---|
| `energy` (**mesurée**) | paquets de lumière glissant | flux ressenti ; **bloqué si epistemic ≠ Measured** (membrane) |
| `weight` | épaisseur accumulée | usage cumulé |
| `recency` | patine | frais = net · ancien = terne (jamais effacé) |
| `stability` | régularité vs scintillement | instable ⇒ scintille |
| `gate` | vanne visible | ouvert / goulot (latence) / éteint (expiré) — l'unique interface comportement→physique, donc explicite |

### Honnêteté (superposée à tout)
- Attribut `unknown`/`not_measured` ⇒ **le canal passe en brume/pointillé**, jamais
  une valeur par défaut confiante. L'absence se dit.
- Prédicat `runtime_compatibility_only` (12) ⇒ conduit **fantôme translucide**.
- Le lien hérite de l'épistémie de ses extrémités : solide entre deux mesurés,
  translucide vers une hypothèse.

---

## 3. Contraintes de fer (héritées, non-négociables)

1. **Pas de ligne générique** (`ontology3d` contract `visual-truth`).
2. **Flux ressenti ⇒ énergie mesurée** : `EnergyTransferMessage::validate` refuse tout
   transfert `epistemic != Measured`. Ne pas contourner via fichier.
3. **Une seule table** prédicat→rendu, dérivée de A. Supprimer C, pas la dupliquer.
4. **Le renderer dérive, n'invente pas.** Toute signification vient du graphe.

---

## 4. Ce qui change concrètement (dette → cible)

- **Supprimer** la taxonomie 11-familles de `relation-infrastructure.ts` ; la
  remplacer par la projection des **20 familles** canoniques + les 3 scalaires du
  `physical_profile`.
- **Cesser d'aplatir** : la hauteur est détruite **deux fois** — `World.tsx:200-201`
  force chaque endpoint à `y=0` (`sourceFootprint/targetFootprint`), puis
  `drapedRoute` (`relation-infrastructure.ts:121-137`) redrape sur le terrain. Cible :
  le conduit relie les **positions réelles** `[x,y,z]` et suit la pente `hierarchy`.
- **Étendre le wire** `desktop_world_snapshot.rs` : chaque `relation_materialized`
  doit porter `family`, `polarity`, `hierarchy`, `permanence`, `mode`, `gate`,
  `energy`, `recency`, `stability`.
  - ✅ **FAIT (slice §6)** : `polarity_micro` + `hierarchy_micro`, lus depuis le
    `physical_profile` que le kernel lisait déjà (une lecture, deux usages). Absents
    quand le prédicat n'a pas de profil (jamais un `0` par défaut). L'adaptateur
    (`protocol-adapter.ts:253-284`) les consomme déjà. **Preuve** : store ontologie →
    1001/1021 relations portent la physique, `BLOCKS` en polarité négative
    (inhibition), les 4 prédicats sans profil restant neutres, `readback_ok`.
  - ⬜ Reste : `family`, `permanence`, `mode`, `gate`, `energy`, `recency`, `stability`
    (+ même traitement dans `desktop_world_delta.rs`).
- **Compléter B** : le kernel lit déjà polarity/hierarchy pour les positions ; les
  mêmes valeurs alimentent le rendu du conduit (une lecture, deux usages).

---

## 5. Comment ça devient autorité (graph-first, pas hardcode)

La table §2 se matérialise dans le **`visual-mapping`** (`fixtures/assets/visual-*`,
`universe-assets::visual`), étendu du niveau *entité* (embodiment citoyen, déjà fait)
au niveau **arête** : `family → teinte`, `physical_profile → matière/pente/lumière`,
`état du lien → paquets/patine/vanne`, `epistemic → brume`. Le
`desktop_world_snapshot` porte alors ces canaux par edge ; le renderer les consomme
sans réinventer de style. La légende de référence de cette table :
[bond-grammar](scratchpad/bond-grammar.html) (design, non-autoritative).

**✅ Matérialisée** : la table §2 est désormais un Asset content-addressed —
`crates/universe-assets/src/bond_channel.rs` + fixture `fixtures/assets/bond-channel-grammar.json`,
relue byte-à-byte (parité) avec les deux invariants gardés (`unknown`⇒fog ; `energy`⇒measured),
même pattern que `visual.rs`/`layout_authority.rs`. Reçu `artifacts/assets/bond-channel-20260730-001`.
C'est **la** table unique ; le renderer la dérive, il ne la réinvente pas.

---

## 5b. L'autre moitié du point unique : les nœuds (rôle → forme)

Le point unique vaut aussi pour **les nœuds** — même principe : la forme se dérive du
**rôle** (`roleAxis` = ce qu'un node fait de l'énergie), **pas** du `content_kind`.
Cette section **absorbe** `cloitre-visual-grammar.md` (désormais un pointeur vers ici),
supprimant la dernière divergence côté nœuds.

| rôle | forme | interdit nommé |
|---|---|---|
| **`space`** (conteneur) | **plateau/dais circulaire** ; taille+lumière = Φ (Core Hub, Nécropole, Bibliothèque, Observatoire, Serre, Tribunal, Usine) | pas une tour générique |
| **`actor`** (pompe) | **avatar-source** (toi / le Citizen) | pas un bâtiment |
| **`narrative`** (attracteur) | **arbre cristallin** qui pousse sur un plateau | — |
| **`moment`** (passage) | **impulsion / flux** transitoire | pas statique |
| **`thing`** (routeur) | **objet maniable** (board, Lanterne, tool MCP) | — |

Le reste (137 définitions, 65 profils…) = **matière et lumière**, pas de l'architecture.
Épistémique : identique à §2 — un node `unknown`/`not_measured` passe en **Fog**, jamais
faussé. **Interdits** (mes deux ratés, nommés) : ❌ tours corporate/SimCity (🔴 `ideas.md`
§10.3) · ❌ mobile de bébé (breloques pastel flottantes).

---

## 6. La plus petite tranche vraie

Avant tout flux : rendre **statique** ce qui ne coûte aucune physique.

- **✅ Côté liens — FAIT (pente + couleur, renderer déterministe)** : un bond dérive
  sa **couleur** du signe de `polarity` (excitation `#46e0d0` / inhibition `#d8607a`)
  et sa **pente** de `hierarchy` (le point de contrôle de l'arc penche vers le tout),
  chaque canal strictement opt-in. Preuve : `apps/mind-desktop/src/scene-svg.ts` +
  4 assertions dans `scene-svg.regression.test.ts` (couleur ± , pente orthogonale à la
  couleur, et **absence ⇒ bond neutre byte-identique** — les goldens existants ne
  bougent pas). Contrat étendu (`RelationVisualDescriptor.hierarchy?/.polarity?`) et
  adaptateur prêt (`hierarchy_micro`/`polarity_micro`). **Pas encore live** : le bin
  `desktop_world_snapshot.rs` ne matérialise pas encore ces champs sur le wire (il les
  lit déjà pour le layout, `:404`) — c'est le prochain pas §4, « une lecture deux
  usages ». Ordre restant : matière/gate → paquets **mesurés**.
- **✅ Côté nœuds — FAIT (la Lanterne)** : révéler le statut épistémique en **lumière /
  brume** sans bouger — application directe du canal épistémique §2. Preuve
  déterministe : `apps/mind-desktop/src/scene-svg.ts` mode `lantern` + golden
  `src/__snapshots__/scene-lantern.svg` (`scene-svg.regression.test.ts`) : un `measured`
  reste clair, un `unknown` est noyé de Fog et marqué « ? », jamais faussé. C'est le
  joyau (honnêteté épistémique) prouvé à l'écran avant tout ride.
- **✅ Côté liens — FAIT (tranche statique)** : la table canonique unique existe —
  `relation-infrastructure.ts::projectPhysicalProfile` : `polarity`-signe → couleur de
  lumière, `hierarchy` → **pente** (fin de l'aplatissement `y=0` dans `World.tsx` `Bond`),
  `permanence` → matière, asymétrie polarité → sens. La cité ontology
  (`?fixture=ontology`) résout **149/149** liens vers leur `physical_profile` canonique,
  matérialisé depuis le store par `scripts/materialize-ontology-registry.mjs`. Preuve :
  `relation-infrastructure.test.ts` (projection) + `ontology-registry-fixture.test.ts`.
  Reste (dette nommée, non supprimée) : brancher le wire `desktop_world_snapshot.rs`
  pour les autres sources, retirer la table 11-familles de secours, et les canaux
  `energy/gate` (exigent la mesure). Profils encore `prototype_not_calibrated` ⇒ rendus
  plus estompés (canal honnêteté §2), jamais présentés comme calibrés.

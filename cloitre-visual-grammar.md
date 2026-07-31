# Loop — Cloître Visual Grammar

> **⚠ Absorbé dans [ALIGN.md](ALIGN.md) §5b** (le point viz unique). La *décision*
> côté nœuds vit là-bas ; **ce fichier = la forme Loop détaillée** de cette décision
> (Objective → Observer → Health → Maintenance), non-autoritative, pour la matérialisation.
> En cas de divergence, ALIGN.md tranche.
>
> Exprimé comme une **boucle** (CLAUDE.md), matérialisable dans le `visual-mapping`.
> Étoile du nord : la cité-cathédrale cyber-liturgique vivante. Socle : les 5 rôles du
> `roleAxis` (`ideas.md` §0).
> Racine : `Space` = `l2:mind-desktop:rendering` (rendu du monde), relié à `l1:cloître`.

---

## Objective
Toute scène affichée est **dérivée du graphe** — la forme d'un node vient de son
**rôle**, l'état de son **statut épistémique**, l'ambiance de la **santé** — et
**jamais un `unknown` n'est rendu comme un `measured`**. Condition de succès
observable : un observateur indépendant relit la scène rendue et retrouve, pour
chaque node, le rôle et le statut du graphe ; zéro forme inventée ; zéro Fog manquant.

## Pattern
**Projection calculée, pas copie.** Le renderer résout `role → forme`,
`relation_family → style de lien`, `epistemic → lumière/brume`, `health → météo`.
Un seul thème committé (monde de nuit sous lumière zénithale). La signification
vit dans le graphe ; le renderer la lit, il ne l'invente pas.

## Vocabulary
- **plateau** : rendu d'un `space` (LIEU). **objet** : rendu d'un `thing` (routeur).
- **arbre** : rendu d'un `narrative` (attracteur qui pousse). **flux/impulsion** : `moment` (passage).
- **pompe** : `actor` (toi/le Citizen). **constellation** : les relations en liens lumineux.
- **Fog** : brume = `unknown`/`not_measured`. **fantôme** : hypothèse translucide.
- **god-rays** : santé (cohérence). **cascade** : énergie **mesurée** qui descend un gradient.
- **ruban** : route carve-able (le ride). Anti-formes interdites : **tour corporate**, **breloque flottante**.

## Behavior (GIVEN / WHEN / THEN)
- **GIVEN** un `space` avec Φ, **WHEN** on rend, **THEN** un plateau circulaire dont
  la taille/lumière suit Φ ; **jamais** un immeuble gris générique.
- **GIVEN** un node `unknown`/`not_measured`, **THEN** il est **noyé de Fog** (désaturé,
  émission 0) ; **FORBIDDEN** de le rendre solide/éclairé comme un `measured`.
- **GIVEN** une hypothèse (`speculative`), **THEN** fantôme translucide, distinct du mesuré.
- **GIVEN** une relation, **THEN** un lien-constellation dont le style vient de sa
  `relation_family` ; **jamais** une rue bitume.
- **GIVEN** un flux d'énergie `epistemic != measured`, **THEN** il **n'est pas** rendu
  comme une cascade ressentie (la membrane le refuse — voir Justification).

## Algorithm
1. Résoudre le **rôle** du node (`roleAxis`) → forme de base.
2. Résoudre le **statut épistémique** → modulation lumière/brume (mesuré→éclairé,
   unknown→Fog, hypothèse→fantôme, measurement_failed→scintillement rouge).
3. Résoudre la **`relation_family`** de chaque edge → style de lien-constellation.
4. Dériver la **santé** → météo/god-rays. Composer flux **mesurés** en cascades.
5. Préserver **unknown / not_measured / measurement_failed** — jamais de défaut inventé.

## CodeDefinition
Extension graph-native du `visual-mapping` (cf. `universe-assets::visual` +
`fixtures/assets/visual-*`) du niveau *entité* (embodiment citoyen, déjà fait) au
niveau **rôle** : tables `role→forme`, `relation_family→lien`, `epistemic→modulation`,
`health→météo`, matérialisées comme Assets content-addressed, dérivées du graphe.

## Implementation
`planned` pour la grammaire complète. Déjà `materialized/verified` : l'invariant
épistémique au niveau matériau (`universe-assets::visual` refuse une policy qui
émettrait un `unknown` comme confiant) et sa consommation par le renderer
(`validateEmbodimentMapping`). `substrate` : positions (`physical_profile`), énergie
sémantique mesurée (`canonical_seed_energy`), app 3D (`World.tsx`).

## Justification
Le rôle (énergie) est le bon axe de forme : il rend le monde **lisible**
(space=lieu, thing=objet) sans dupliquer chaque `content_kind`. Alternatives
rejetées : (a) `content_kind → immeuble` = tours corporate 🔴 (`ideas.md` §10.3) ;
(b) breloques flottantes = mobile de bébé. **Contrainte de fer** : la membrane
(`EnergyTransferMessage::validate`, `universe-protocol/src/lib.rs:218`) refuse tout
transfert `epistemic != Measured` — donc une glisse **dérivée** ne peut pas devenir
un flux ressenti sur le wire, par design ; ne pas contourner via fichier (CLAUDE.md).

## Validation
Fixtures + cas négatifs : (i) un `space` ne rend jamais une tour générique ;
(ii) un node `unknown` rend émission 0 + Fog (cas négatif : une policy qui l'éclaire
est **refusée**) ; (iii) une hypothèse ≠ un mesuré à l'œil ; (iv) parité : le mapping
rendu passe le validateur du renderer. Régression déterministe : les golden SVG
(`scene-svg.regression.test.ts`) + les tests de modulation (`visual.rs`).

## Observer
Procédure **indépendante** qui relit la scène rendue (pas les claims du renderer) :
pour chaque node, extrait rôle + statut de la scène et les **compare au graphe** ;
compte les nodes rendus solides dont le graphe dit `unknown` (doit être **0**) ;
compte les Fog manquants ; vérifie que chaque edge a un lien. N'assimile jamais un
Fog absent à « tout mesuré ».

## Observer validation
Tests prouvant que l'observer **détecte de vrais échecs** : injecter un node
`unknown` rendu à tort comme `measured` → l'observer doit lever ; retirer un Fog →
lever ; ne pas convertir une scène vide en succès.

## Metric
Vecteur (dimensions **non** fusionnées) : `roles_rendered / total`,
`fog_correct / unknown_total`, `unknown_rendered_confident` (doit être 0),
`edges_with_link / edges`, `epistemic_states_distinct`, `health_source` (mesuré / dérivé / not_measured).

## Health
État vivant dérivé des métriques + fraîcheur : `healthy` (couverture pleine, 0
malhonnêteté, preuve fraîche) · `degraded` (Fog manquants) · `stale` (scène vieille) ·
`unknown` / `not_measured` (pas encore mesuré) · `measurement_failed`. Jamais « sain »
par absence d'erreur.

## Maintenance
Affordances de réparation : **re-dériver** la scène depuis le graphe ; **recalibrer**
la palette/Φ→taille ; **relier** un rôle sans mapping (reste `unknown`, pas de défaut) ;
**rematérialiser** le `visual-mapping` ; **suspendre** le rendu d'une région non-mesurée
(Fog plutôt que faux) ; **demander à un humain** si un rôle n'a pas de forme déclarée.

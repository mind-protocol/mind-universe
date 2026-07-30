# mind-universe

`mind-universe` est le noyau bootstrap d’un univers graph-native. Le graphe est
la source de vérité à long terme ; ce dépôt contient uniquement le code natif
nécessaire pour charger, valider, exécuter, persister, observer et réparer cet
univers.

Le premier parcours vertical headless est déjà exécutable :

```text
Actor graph_read
→ ReadField
→ résonance locale d’un Space
→ TopologicalFold
→ exécution d’une CodeDefinition Graph IR
→ commit d’un Moment
→ relecture indépendante
→ libération du fold
```

## Prérequis

- Rust stable avec `cargo`
- Windows, Linux ou macOS
- Git, uniquement pour cloner le dépôt

Vérifier l’installation :

```powershell
rustc --version
cargo --version
```

## Lancement rapide

Depuis la racine du dépôt :

```powershell
cargo run -p universe-e2e --bin universe-e2e -- artifacts/verification
```

Cette commande lance le parcours vertical complet à partir des fixtures
Genesis et Graph IR. En cas de succès, elle affiche un identifiant de
corrélation, par exemple :

```text
e2e-10732-1785405571972966200
```

Les preuves de l’exécution sont écrites dans :

```text
artifacts/verification/<correlation-id>/
```

On y trouve notamment :

- `manifest.json` : résultat et identifiants de l’exécution ;
- `vm-trace.jsonl` : trace des instructions Graph IR ;
- `phases.json` : ordre des phases du superviseur ;
- `runtime-inventory.json` : mécanismes réellement activés.

L’état persistant utilisé par ce lancement est placé dans
`artifacts/verification/store/`. Ces fichiers sont des artefacts locaux et ne
remplacent pas l’autorité du Universe Snapshot et de son journal valide.

Pour écrire les preuves dans un autre dossier :

```powershell
cargo run -p universe-e2e --bin universe-e2e -- C:\temp\mind-universe-proof
```

## Lancer seulement le serveur bootstrap

Le serveur ouvre un store et charge une Genesis, puis affiche son état initial.
Il se trouve dans un workspace Cargo séparé :

```powershell
cargo run --manifest-path apps/universe-server/Cargo.toml -- `
  artifacts/server-store `
  fixtures/genesis/minimal-genesis.json
```

La sortie attendue commence par :

```text
ready universe_revision=0 tick=0
```

Ce serveur est encore un bootstrap minimal : il ne fournit pas à lui seul le
parcours E2E, le Desktop ou un service réseau complet. Pour vérifier le système
intégré, utiliser `universe-e2e`.

## Vérification

Formater et tester tout le workspace principal :

```powershell
cargo fmt --all -- --check
cargo test --workspace
```

Puis exécuter la preuve intégrée avec un dossier propre :

```powershell
cargo run -p universe-e2e --bin universe-e2e -- artifacts/verification/manual
```

Un processus qui démarre ou des tests unitaires verts ne suffisent pas à
prouver le comportement : la preuve E2E doit produire ses reçus, puis relire le
Moment commité par un chemin indépendant.

## Organisation

```text
crates/universe-core/          identités et contrats partagés
crates/universe-store/         snapshot, journal et contenu
crates/universe-physics/       résidence physique et Rapier
crates/universe-query/         requêtes locales bornées
crates/universe-ir/            représentation Graph IR
crates/universe-compiler/      compilation déterministe
crates/universe-vm/            exécution bornée
crates/universe-transactions/  write sets et commits
crates/universe-supervisor/    boot et ordonnancement
crates/universe-protocol/      messages et relecture
crates/universe-e2e/           parcours vertical headless
apps/universe-server/          serveur bootstrap minimal
fixtures/                      Genesis et programmes de test
artifacts/verification/        preuves locales générées
```

## État actuel

Le premier parcours vertical headless fonctionne, mais le bootstrap v0 complet
n’est pas encore atteint. Parmi les limites connues : transactions
multi-commandes atomiques, triggers complets, reprise protocolaire, signature
cryptographique de Genesis, effets externes avec reçus, preuve d’échelle
10M/10M et Mind Desktop.

Le détail des contrats, responsabilités, travaux terminés et blocages se trouve
dans [AGENTS.md](AGENTS.md) et [TODO.md](TODO.md).

## Règle d’autorité

Le comportement variable appartient au graphe : nodes, relations,
CodeDefinitions, loops, policies et ChangeSets. Les fichiers natifs de ce dépôt
ne doivent contenir que les primitives indispensables au bootstrap. Toute
évolution fonctionnelle doit modifier l’autorité graph-native avant de
matérialiser ou compiler un artefact dérivé.

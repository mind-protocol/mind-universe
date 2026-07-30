#!/usr/bin/env node

// Recovery/export tool only. The generated GraphSeed becomes authoritative
// after UniverseStore installs it; these source files are provenance inputs.

import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

function argumentsFrom(argv) {
  const values = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (!flag?.startsWith("--") || value === undefined) {
      throw new Error(`invalid argument near ${flag ?? "<end>"}`);
    }
    values.set(flag.slice(2), value);
  }
  return values;
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function sortedValue(value) {
  if (Array.isArray(value)) {
    return value.map(sortedValue);
  }
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, sortedValue(value[key])]),
    );
  }
  return value;
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function canonicalJsonHash(value) {
  return sha256(JSON.stringify(sortedValue(value)));
}

function stableKey(value) {
  return value.toString(16).padStart(32, "0");
}

function difference(left, right) {
  return [...left].filter((value) => !right.has(value)).sort();
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

const args = argumentsFrom(process.argv.slice(2));
const repository = process.cwd();
const sourceRoot = resolve(repository, "..", "body-suit", "data");
const ontologyPath = resolve(
  args.get("ontology") ?? resolve(sourceRoot, "graph-ontology.json"),
);
const vocabularyPath = resolve(
  args.get("vocabulary") ?? resolve(sourceRoot, "ontology-vocabulary.json"),
);
const mappingPath = resolve(
  args.get("mapping") ?? resolve(sourceRoot, "l4-ontology-mapping.json"),
);
const outputPath = resolve(
  args.get("output") ??
    resolve(repository, "fixtures", "ontology", "canonical-ontology.json"),
);

const ontology = readJson(ontologyPath);
const vocabulary = readJson(vocabularyPath);
const mapping = readJson(mappingPath);

assert(ontology.schemaVersion === "1.17.0", "unexpected ontology version");
assert(mapping.mappingVersion === "0.4.0", "unexpected mapping version");
assert(ontology.nodeTypes.length === 5, "stored node type count drifted");
assert(ontology.semanticTypes.length === 34, "semantic type count drifted");
assert(ontology.relationFamilies.length === 20, "relation family count drifted");
assert(ontology.relationTypes.length === 55, "predicate count drifted");
assert(ontology.epistemicStatuses.length === 11, "status count drifted");
assert(vocabulary.nodes.length === 120, "doctrine node count drifted");
assert(vocabulary.links.length === 70, "doctrine relation count drifted");

const vocabularyNodes = new Map(vocabulary.nodes.map((node) => [node.id, node]));
assert(
  vocabularyNodes.size === vocabulary.nodes.length,
  "duplicate doctrine node ID",
);
for (const link of vocabulary.links) {
  assert(vocabularyNodes.has(link.source), `missing doctrine source ${link.source}`);
  assert(vocabularyNodes.has(link.target), `missing doctrine target ${link.target}`);
  assert(link.justification?.trim(), "doctrine relation lacks justification");
}

const typeMappingNode = mapping.nodes.find(
  (node) => node.id === "l4-node-type-mapping",
);
const predicateMappingNode = mapping.nodes.find(
  (node) => node.id === "l4-predicate-translation-dictionary",
);
assert(typeMappingNode?.mappings?.length === 34, "node type mapping is incomplete");
assert(
  predicateMappingNode?.profiles?.length === 65,
  "predicate profile mapping is incomplete",
);

const relationIds = new Set(ontology.relationTypes.map((item) => item.id));
const constraintIds = new Set(Object.keys(ontology.relationConstraints));
const profileByPredicate = new Map(
  predicateMappingNode.profiles.map((profile) => [profile.source, profile]),
);
const profileIds = new Set(profileByPredicate.keys());
const constraintGaps = difference(relationIds, constraintIds);
const physicalGaps = difference(relationIds, profileIds);
assert(
  JSON.stringify(constraintGaps) ===
    JSON.stringify(["BASED_ON", "PROPOSES_CHANGE_TO"]),
  `unexpected endpoint-constraint gaps: ${constraintGaps.join(", ")}`,
);
assert(
  JSON.stringify(physicalGaps) === JSON.stringify(constraintGaps),
  "constraint and physical gaps no longer coincide",
);

const compatibilityPredicates = difference(
  new Set([...constraintIds, ...profileIds]),
  relationIds,
);
assert(
  compatibilityPredicates.length === 12,
  "compatibility predicate count drifted",
);
for (const predicate of compatibilityPredicates) {
  assert(
    ontology.relationConstraints[predicate] !== undefined,
    `compatibility predicate ${predicate} lacks a constraint`,
  );
  assert(
    profileByPredicate.has(predicate),
    `compatibility predicate ${predicate} lacks a profile`,
  );
}

const storedRoleBySemanticType = new Map(
  typeMappingNode.mappings.map((entry) => [entry.source, entry]),
);
const storedNodeTypeIds = new Set(ontology.nodeTypes.map((item) => item.id));
for (const semanticType of ontology.semanticTypes) {
  const storageMapping = storedRoleBySemanticType.get(semanticType.id);
  assert(storageMapping, `semantic type ${semanticType.id} lacks storage mapping`);
  assert(
    storedNodeTypeIds.has(storageMapping.l4),
    `semantic type ${semanticType.id} maps to unknown stored type ${storageMapping.l4}`,
  );
}

const entities = [];
const relations = [];
const entityKeyByIdentity = new Map();
const doctrineKeyById = new Map();
const provenance = new Map();

function addEntity(key, symbol, content, identity) {
  assert(!entityKeyByIdentity.has(identity), `duplicate identity ${identity}`);
  const entity = {
    key: stableKey(key),
    generation: 0,
    symbol,
    content: sortedValue(content),
  };
  entities.push(entity);
  entityKeyByIdentity.set(identity, entity.key);
  return entity.key;
}

let nextRelationKey = 0x2000;
function addRelation(source, target, predicate, justification, role, extra = {}) {
  assert(source && target, `missing endpoint for ${predicate}`);
  assert(justification?.trim(), `${predicate} relation lacks justification`);
  relations.push({
    key: stableKey(nextRelationKey),
    generation: 0,
    source,
    target,
    predicate,
    content: sortedValue({
      ...extra,
      justification,
      kind: "ontology_relation",
      role,
    }),
  });
  nextRelationKey += 1;
}

function rememberProvenance(entityKey, ...sourceKeys) {
  provenance.set(entityKey, sourceKeys);
}

const sourceHashes = {
  executable_schema: canonicalJsonHash(ontology),
  l4_mapping: canonicalJsonHash(mapping),
  vocabulary_doctrine: canonicalJsonHash(vocabulary),
};

const manifestContent = {
  authority_statement:
    "Après installation, ce cluster du UniverseStore est l'autorité canonique. Les documents embarqués ne sont que sa provenance reconstructive.",
  compatibility_predicates: compatibilityPredicates,
  declared_counts: {},
  kind: "ontology_manifest",
  language: ontology.language,
  mapping_version: mapping.mappingVersion,
  ontology_id: "mind-canonical-ontology",
  known_gaps: constraintGaps,
  schema_version: ontology.schemaVersion,
  source_hashes: sourceHashes,
  status: "reconstructed_with_explicit_gaps",
};
const manifestKey = addEntity(
  0x1000,
  "protocol",
  manifestContent,
  "manifest:mind-canonical-ontology",
);

const executableSourceKey = addEntity(
  0x1001,
  "source_document",
  {
    authority: "reconstruction_input_executable_schema",
    canonical_json_sha256: sourceHashes.executable_schema,
    document: ontology,
    kind: "ontology_source",
    source_id: "graph-ontology.json@1.17.0",
    source_role: "executable_schema",
  },
  "source:executable",
);
const doctrineSourceKey = addEntity(
  0x1002,
  "source_document",
  {
    authority: "reconstruction_input_doctrine",
    canonical_json_sha256: sourceHashes.vocabulary_doctrine,
    document: vocabulary,
    kind: "ontology_source",
    source_id: "ontology-vocabulary.json@1.17.0",
    source_role: "vocabulary_doctrine",
  },
  "source:doctrine",
);
const mappingSourceKey = addEntity(
  0x1003,
  "source_document",
  {
    active_subset: [
      "l4-node-type-mapping",
      "l4-predicate-translation-dictionary",
    ],
    authority: "historical_provenance_only",
    canonical_json_sha256: sourceHashes.l4_mapping,
    document: mapping,
    kind: "ontology_source",
    source_id: "l4-ontology-mapping.json@0.4.0",
    source_role: "l4_mapping",
  },
  "source:mapping",
);

const definitionKeys = {
  stored_node_type: new Map(),
  semantic_type: new Map(),
  relation_family: new Map(),
  predicate: new Map(),
  epistemic_status: new Map(),
  compatibility_predicate: new Map(),
};

function addDefinition(base, index, definitionKind, item, options = {}) {
  const canonicalId = item.id;
  const content = {
    canonical: options.canonical ?? true,
    canonical_id: canonicalId,
    constraint_status: options.constraintStatus,
    definition_kind: definitionKind,
    doctrine: options.doctrine ?? null,
    endpoint_constraint: options.endpointConstraint ?? null,
    executable: options.executable ?? item,
    kind: "ontology_definition",
    schema_version: ontology.schemaVersion,
    status: options.status ?? "active",
    storage_mapping: options.storageMapping ?? null,
  };
  const key = addEntity(
    base + index,
    "terme",
    content,
    `definition:${definitionKind}:${canonicalId}`,
  );
  definitionKeys[definitionKind].set(canonicalId, key);
  if (options.doctrine) {
    doctrineKeyById.set(options.doctrine.id, key);
  }
  rememberProvenance(key, ...(options.sources ?? [executableSourceKey]));
  return key;
}

ontology.nodeTypes.forEach((item, index) => {
  addDefinition(0x1100, index, "stored_node_type", item);
});

ontology.semanticTypes.forEach((item, index) => {
  const doctrine = vocabularyNodes.get(`terme-type-${item.id}`);
  assert(doctrine, `missing doctrine for semantic type ${item.id}`);
  addDefinition(0x1200, index, "semantic_type", item, {
    doctrine,
    sources: [executableSourceKey, doctrineSourceKey, mappingSourceKey],
    storageMapping: storedRoleBySemanticType.get(item.id),
  });
});

ontology.relationFamilies.forEach((item, index) => {
  const doctrine = vocabularyNodes.get(`terme-famille-${item.id}`);
  assert(doctrine, `missing doctrine for relation family ${item.id}`);
  addDefinition(0x1300, index, "relation_family", item, {
    doctrine,
    sources: [executableSourceKey, doctrineSourceKey],
  });
});

ontology.relationTypes.forEach((item, index) => {
  const doctrine = vocabularyNodes.get(`terme-predicat-${item.id}`);
  assert(doctrine, `missing doctrine for predicate ${item.id}`);
  const hasConstraint = ontology.relationConstraints[item.id] !== undefined;
  addDefinition(0x1400, index, "predicate", item, {
    constraintStatus: hasConstraint ? "defined" : "missing",
    doctrine,
    endpointConstraint: ontology.relationConstraints[item.id] ?? null,
    sources: [executableSourceKey, doctrineSourceKey],
  });
});

ontology.epistemicStatuses.forEach((item, index) => {
  const doctrine = vocabularyNodes.get(`terme-statut-${item.id}`);
  assert(doctrine, `missing doctrine for epistemic status ${item.id}`);
  addDefinition(0x1500, index, "epistemic_status", item, {
    doctrine,
    sources: [executableSourceKey, doctrineSourceKey],
  });
});

compatibilityPredicates.forEach((canonicalId, index) => {
  const profile = profileByPredicate.get(canonicalId);
  addDefinition(
    0x1600,
    index,
    "compatibility_predicate",
    { id: canonicalId },
    {
      canonical: false,
      constraintStatus: "defined",
      endpointConstraint: ontology.relationConstraints[canonicalId],
      executable: null,
      sources: [executableSourceKey, mappingSourceKey],
      status: "runtime_compatibility_only",
      storageMapping: null,
      doctrine: null,
    },
  );
  assert(
    definitionKeys.relation_family.has(profile.family),
    `compatibility predicate ${canonicalId} has unknown family ${profile.family}`,
  );
});

const definitionArrayNames = new Set([
  "nodeTypes",
  "semanticTypes",
  "relationFamilies",
  "relationTypes",
  "epistemicStatuses",
]);
const metadataNames = new Set(["schemaVersion", "language"]);
const contractEntries = Object.entries(ontology).filter(
  ([name]) => !definitionArrayNames.has(name) && !metadataNames.has(name),
);
const contractKeys = new Map();
contractEntries.forEach(([contractId, value], index) => {
  const key = addEntity(
    0x1700 + index,
    "terme",
    {
      canonical_id: contractId,
      kind: "ontology_contract",
      schema_version: ontology.schemaVersion,
      value,
    },
    `contract:${contractId}`,
  );
  contractKeys.set(contractId, key);
  rememberProvenance(key, executableSourceKey);
});

const physicalProfileKeys = new Map();
predicateMappingNode.profiles.forEach((profile, index) => {
  const key = addEntity(
    0x1800 + index,
    "mechanism",
    {
      canonical_id: profile.source,
      kind: "physical_profile",
      mapping_version: mapping.mappingVersion,
      profile,
      status: "prototype_not_calibrated",
    },
    `physical_profile:${profile.source}`,
  );
  physicalProfileKeys.set(profile.source, key);
  rememberProvenance(key, mappingSourceKey);
});

const gapKeys = new Map();
constraintGaps.forEach((predicate, index) => {
  const key = addEntity(
    0x1900 + index,
    "open_question",
    {
      canonical_id: `predicate:${predicate}:constraint_and_physics`,
      evidence: {
        declared_in_relation_types: true,
        endpoint_constraint_present: constraintIds.has(predicate),
        physical_profile_present: profileIds.has(predicate),
      },
      kind: "ontology_gap",
      missing: ["endpoint_constraint", "physical_profile"],
      status: "unresolved",
      subject: predicate,
      summary:
        "Le prédicat est canonique dans le schéma 1.17.0 mais sa contrainte d'extrémités et son prototype physique n'ont jamais été définis dans les sources.",
    },
    `gap:${predicate}`,
  );
  gapKeys.set(predicate, key);
  rememberProvenance(key, executableSourceKey, mappingSourceKey);
});

manifestContent.declared_counts = {
  compatibility_predicates: compatibilityPredicates.length,
  contracts: contractKeys.size,
  doctrine_links: vocabulary.links.length,
  epistemic_statuses: ontology.epistemicStatuses.length,
  gaps: gapKeys.size,
  physical_profiles: physicalProfileKeys.size,
  predicates: ontology.relationTypes.length,
  relation_families: ontology.relationFamilies.length,
  registry_members: entities.length - 1,
  semantic_types: ontology.semanticTypes.length,
  source_documents: 3,
  stored_node_types: ontology.nodeTypes.length,
};
entities[0].content = sortedValue(manifestContent);

addRelation(
  executableSourceKey,
  manifestKey,
  "GROUNDS",
  "Le schéma exécutable 1.17.0 fournit les définitions et contraintes récupérées.",
  "reconstruction_provenance",
);
addRelation(
  doctrineSourceKey,
  manifestKey,
  "GROUNDS",
  "Le miroir doctrinal 120/70 fournit les définitions argumentées et leurs liens.",
  "reconstruction_provenance",
);
addRelation(
  mappingSourceKey,
  manifestKey,
  "GROUNDS",
  "Le mapping L4 0.4.0 fournit les rôles de stockage et prototypes physiques récupérés.",
  "reconstruction_provenance",
);

for (const entity of entities.slice(1)) {
  addRelation(
    entity.key,
    manifestKey,
    "PART_OF",
    "Cet Atome appartient au registre ontologique canonique reconstruit.",
    "registry_membership",
  );
}

for (const [entityKey, sourceKeys] of provenance) {
  for (const sourceKey of sourceKeys) {
    addRelation(
      entityKey,
      sourceKey,
      "DERIVED_FROM",
      "Le contenu de cet Atome est dérivé de ce document source embarqué et hashé.",
      "definition_provenance",
    );
  }
}

for (const link of vocabulary.links) {
  addRelation(
    doctrineKeyById.get(link.source),
    doctrineKeyById.get(link.target),
    link.type,
    link.justification,
    "doctrine_link",
    { doctrine_source: link },
  );
}

for (const [child, parent] of Object.entries(ontology.nodeTypeHierarchy)) {
  addRelation(
    definitionKeys.semantic_type.get(child),
    definitionKeys.semantic_type.get(parent),
    "SUBCASE_OF",
    `${child} est déclaré comme sous-cas de ${parent} par nodeTypeHierarchy dans le schéma 1.17.0.`,
    "semantic_type_hierarchy",
  );
}

for (const predicate of compatibilityPredicates) {
  const profile = profileByPredicate.get(predicate);
  addRelation(
    definitionKeys.compatibility_predicate.get(predicate),
    definitionKeys.relation_family.get(profile.family),
    "DEFINES",
    `${predicate} reste rattaché à ${profile.family} uniquement pour compatibilité avec le mapping et les contraintes historiques.`,
    "compatibility_family_binding",
  );
}

for (const [predicate, profileKey] of physicalProfileKeys) {
  const predicateKey =
    definitionKeys.predicate.get(predicate) ??
    definitionKeys.compatibility_predicate.get(predicate);
  assert(predicateKey, `physical profile ${predicate} has no predicate definition`);
  addRelation(
    profileKey,
    predicateKey,
    "DEFINES",
    `Ce profil L4 0.4.0 initialise la lecture physique de ${predicate}; il reste un prototype non calibré.`,
    "physical_profile_binding",
  );
}

for (const [predicate, gapKey] of gapKeys) {
  addRelation(
    gapKey,
    definitionKeys.predicate.get(predicate),
    "ADDRESSES",
    `Cette lacune nomme explicitement les définitions manquantes pour ${predicate}.`,
    "ontology_gap",
  );
}

const symbols = [
  ...new Set([
    ...ontology.nodeTypes.map((item) => item.id),
    ...ontology.semanticTypes.map((item) => item.id),
    ...ontology.relationFamilies.map((item) => item.id),
    ...ontology.epistemicStatuses.map((item) => item.id),
    ...ontology.relationTypes.map((item) => item.id),
    ...compatibilityPredicates,
  ]),
];

for (const entity of entities) {
  assert(symbols.includes(entity.symbol), `undeclared entity symbol ${entity.symbol}`);
}
for (const relation of relations) {
  assert(
    symbols.includes(relation.predicate),
    `undeclared relation predicate ${relation.predicate}`,
  );
}

const seed = {
  universe: stableKey(0x0a701091),
  symbols,
  entities,
  relations,
};

// Match serde's GraphSeed field order while recursively sorting Value maps.
const rustSeedShape = {
  universe: seed.universe,
  symbols: seed.symbols,
  entities: seed.entities.map((entity) => ({
    key: entity.key,
    generation: entity.generation,
    symbol: entity.symbol,
    content: sortedValue(entity.content),
  })),
  relations: seed.relations.map((relation) => ({
    key: relation.key,
    generation: relation.generation,
    source: relation.source,
    target: relation.target,
    predicate: relation.predicate,
    content: sortedValue(relation.content),
  })),
};
const envelope = {
  contract: "mind-universe-graph-seed",
  version: 0,
  sha256: sha256(JSON.stringify(rustSeedShape)),
  seed,
};

mkdirSync(dirname(outputPath), { recursive: true });
writeFileSync(outputPath, `${JSON.stringify(envelope, null, 2)}\n`, "utf8");

console.log(outputPath);
console.log(`sha256=${envelope.sha256}`);
console.log(`entities=${entities.length}`);
console.log(`relations=${relations.length}`);
console.log(`known_gaps=${constraintGaps.join(",")}`);
console.log(`compatibility_predicates=${compatibilityPredicates.join(",")}`);

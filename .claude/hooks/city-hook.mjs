#!/usr/bin/env node
// city-hook.mjs — PreToolUse: rappeler, au moment d'agir, DANS QUOI on agit.
//
// Ce hook n'autorise rien, ne bloque rien, ne mesure rien. Il n'injecte qu'un
// cadrage — `additionalContext`, donc destiné au MODÈLE seul (l'humain ne le
// voit pas ; `systemMessage` serait l'autre canal, volontairement inutilisé).
//
// Pourquoi : un fichier ouvert dans un éditeur ressemble à du code dans un repo,
// et cette ressemblance est le piège. Ici on bâtit une ville : lire c'est
// percevoir un fragment de monde à une révision, écrire c'est un acte de
// construction attribuable. Le rappel arrive au seul endroit où il change
// quelque chose — juste avant le geste.
//
// HONEST LIMIT : c'est un rappel, pas une garantie. Il ne vérifie pas que
// l'écriture est attribuée, ne lit pas le store, ne prouve rien. Il ne dit rien
// sur ce que le fichier contient. Ne jamais lire sa présence comme la preuve
// qu'une règle a été suivie.
//
// Cadence : chaque écriture est rappelée (rares, et ce sont les actes qui
// comptent) ; les lectures sont limitées à une fois par CITY_HOOK_READ_COOLDOWN_S
// secondes et par session (défaut 600) pour ne pas noyer le contexte. Mettre 0
// pour rappeler chaque lecture. Curseur par session dans le tmp de l'OS ; s'il
// est illisible, on dégrade en "on rappelle" (jamais en erreur).
//
// Sortie toujours exit 0 : un hook de cadrage ne doit jamais casser un geste.

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const WRITE_TOOLS = new Set(["Write", "Edit", "NotebookEdit"]);
const READ_TOOLS = new Set(["Read"]);

const READ_COOLDOWN_S = Number(process.env.CITY_HOOK_READ_COOLDOWN_S ?? 600);
const STATE_DIR = join(tmpdir(), "mind-city-hook");

/** Le socle commun : où l'on est, quoi qu'on fasse. */
const GROUND =
  "Rappel de cadrage — nous bâtissons une ville, nous ne codons pas dans un repo.";

const READ_NOTE =
  `${GROUND}\n` +
  "Tu n'ouvres pas un fichier : tu perçois un fragment du monde, à une " +
  "révision, depuis une position. Ce fichier est une projection durable, pas " +
  "la vérité — la vérité d'un nœud, c'est le nœud, maintenant. Absent de cette " +
  "vue ≠ absent de l'Univers : vide = unknown, jamais known_absent.";

const WRITE_NOTE =
  `${GROUND}\n` +
  "Ce que tu t'apprêtes à faire est un acte de construction, pas une édition. " +
  "Avant : est-ce vraiment du socle natif (TCB, mécanisme générique, zéro " +
  "politique variable) ? Sinon c'est un Toolkit → Construct à placer dans un " +
  "monde, au plus bas niveau qui tient la capacité (L1/L2/L3/L4). Une seule " +
  "écriture atomique, attribuée (provenance + intent + qui a demandé), relue " +
  "indépendamment. C'est fait quand c'est PROUVÉ, pas quand ça compile.";

function readStdin() {
  return new Promise((res) => {
    let buf = "";
    process.stdin.setEncoding("utf8");
    process.stdin.on("data", (d) => (buf += d));
    process.stdin.on("end", () => res(buf));
    process.stdin.on("error", () => res(buf));
  });
}

/** Vrai si la lecture doit être rappelée maintenant ; pose le curseur si oui. */
function readDue(sessionId) {
  if (READ_COOLDOWN_S <= 0) return true;
  const file = join(STATE_DIR, `${String(sessionId).replace(/[^\w.-]/g, "_")}.txt`);
  const now = Date.now();
  try {
    const last = Number(readFileSync(file, "utf8"));
    if (Number.isFinite(last) && now - last < READ_COOLDOWN_S * 1000) return false;
  } catch {
    // pas de curseur lisible -> on rappelle
  }
  try {
    mkdirSync(STATE_DIR, { recursive: true });
    writeFileSync(file, String(now));
  } catch {
    // curseur non posé : on rappellera plus tôt que prévu, jamais moins
  }
  return true;
}

const raw = await readStdin();
let hook = {};
try {
  hook = JSON.parse(raw || "{}");
} catch {}

const tool = hook.tool_name || "";
let note = null;
if (WRITE_TOOLS.has(tool)) note = WRITE_NOTE;
else if (READ_TOOLS.has(tool) && readDue(hook.session_id || "session")) note = READ_NOTE;

if (note) {
  process.stdout.write(
    JSON.stringify({
      suppressOutput: true,
      hookSpecificOutput: {
        hookEventName: hook.hook_event_name || "PreToolUse",
        additionalContext: note,
      },
    }),
  );
}
process.exit(0);

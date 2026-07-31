//! Actor point-of-view: a bounded, deterministic, TEMPORARY view.
//!
//! `sense` perceives *as an actor*. This module places the sensed neighbourhood
//! on a deterministic Fibonacci sphere, anchors the actor's eye, and derives a
//! first-person look orientation and a textual render of the visible spheres.
//!
//! These positions are a *reconstructible projection*, not canonical placement —
//! the honest "materialise a temporary view" effect the `sense` contract allows.
//! They are tagged `projection: "fibonacci-sphere"` so no reader mistakes a view
//! for a construction (CLAUDE.md: "procedural layout ... must not silently move
//! or rewrite canonical placement"; "Fog remains Fog"). The math mirrors the
//! desktop `first-person-look` module (yaw about world-up, pitch about local
//! right) so the textual POV agrees with the rendered one.

use serde::Serialize;

/// The actor's eye and look orientation over the sensed neighbourhood.
#[derive(Clone, Debug, Serialize)]
pub struct Pov {
    /// The perceiving actor (resolved key or the requested id).
    pub actor: String,
    /// True when no matching entity was found — an external observer, not a
    /// situated ActorInstance.
    pub generated: bool,
    pub eye: [f64; 3],
    /// Where the eye came from: `situated` (the actor's inferred position) or
    /// `external_observer`.
    pub eye_source: &'static str,
    pub look_at: [f64; 3],
    pub yaw: f64,
    pub pitch: f64,
    /// Coordinate frame provenance — the positions are solver output.
    pub projection: &'static str,
}

/// One visible object as the actor sees it: a sphere at a bearing and distance.
#[derive(Clone, Debug, Serialize)]
pub struct SphereSighting {
    pub key: String,
    pub label: String,
    /// Visual primitive. Scaffold default `"sphere"`; the per-node primitive
    /// comes from the visual profile (content hydration), a declared gap.
    pub primitive: &'static str,
    pub position: [f64; 3],
    pub distance_m: f64,
    /// 8-point bearing relative to where the actor is facing.
    pub bearing: &'static str,
}

/// Yaw/pitch that make an eye at `eye` look toward `target`, in the desktop's
/// YXZ (yaw-then-pitch) convention. Ported from `first-person-look.ts`.
pub fn orientation_from_look_at(eye: [f64; 3], target: [f64; 3]) -> (f64, f64) {
    let d = [target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if len < 1e-6 {
        return (0.0, 0.0);
    }
    let yaw = (-(d[0] / len)).atan2(-(d[2] / len));
    let pitch = (d[1] / len).clamp(-1.0, 1.0).asin();
    (yaw, pitch)
}

/// 8-point bearing of `target` relative to an actor facing `yaw`, using the
/// ground-plane forward/right basis (walking stays level, as in FPV).
pub fn bearing(eye: [f64; 3], yaw: f64, target: [f64; 3]) -> &'static str {
    let forward = [-yaw.sin(), -yaw.cos()]; // (x, z) ground forward
    let right = [yaw.cos(), -yaw.sin()]; // (x, z) ground right
    let d = [target[0] - eye[0], target[2] - eye[2]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if len < 1e-6 {
        return "here";
    }
    let dn = [d[0] / len, d[1] / len];
    let fdot = dn[0] * forward[0] + dn[1] * forward[1];
    let rdot = dn[0] * right[0] + dn[1] * right[1];
    let angle = rdot.atan2(fdot); // 0 = ahead, +pi/2 = right, +-pi = behind
    let sector = (angle / std::f64::consts::FRAC_PI_4).round() as i64;
    match sector.rem_euclid(8) {
        0 => "ahead",
        1 => "ahead-right",
        2 => "to your right",
        3 => "behind-right",
        4 => "behind you",
        5 => "behind-left",
        6 => "to your left",
        7 => "ahead-left",
        _ => "ahead",
    }
}

/// Euclidean distance in metres (the ~1m/node calibration).
pub fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// One perceived node as a line of the human render: a readable name plus its
/// per-node action phrases, humanised generically from its affordance verbs (empty
/// when the node carries none — then only the name is shown). The `observe`-builder
/// fills this from `objects` + `affordances`, deriving both name and verbs from the
/// store's own data; the text stays a projection of the structured fields, never a
/// second source of truth.
#[derive(Clone, Debug)]
pub struct NodeLine {
    pub name: String,
    pub actions: Vec<String>,
}

/// The human `text` render is filtered to fit within this many characters — the
/// bounded prompt the MCP `sense`/`act` tools and the L1 Ollama loop read. It
/// favours node breadth: as many node names (and their grouped actions) as fit,
/// the rest summarised as a trailing count. The full bounded set always remains
/// machine-available in the structured `objects`/`affordances` fields.
pub const TEXT_BUDGET: usize = 2500;

/// Renders the first-person "text of sense": a prose situation line (place = the
/// world's derived name, never a universe hex id; omitted when none can be
/// derived), then the perceived nodes by NAME — one readable name per line — with
/// each node's affordances listed beneath it as short phrases (`  · verbe`). No
/// distance, bearing, position, type, or physics-sphere wording: a readable,
/// filtered situation, not a coordinate dump. `nodes` is expected to be already
/// grouped/deduplicated (the caller collapses a construct's anatomy faces into one
/// line and unions their actions), with names and verbs already derived from the
/// store's data; this renderer only lays it out within the budget.
///
/// The budget favours breadth: every node NAME always renders, and the grouped
/// actions fill whatever budget remains (stopping once the next bullet would
/// overflow) so a long action list never crowds out node names.
pub fn render_text(
    place: Option<&str>,
    revision: u64,
    situated: bool,
    origin_name: &str,
    nodes: &[NodeLine],
) -> String {
    // The place clause is added only when a place could be derived from the data.
    let at = place.map(|p| format!(", à {p}")).unwrap_or_default();
    let prose = if situated {
        format!("Tu es près de {origin_name}{at} (rev {revision}).")
    } else {
        format!("Tu observes {origin_name}{at} (rev {revision}).")
    };
    if nodes.is_empty() {
        return format!("{prose}\n\nRien n'est à portée dans ton champ perceptif.\n");
    }

    let header = format!("{prose}\n\nAutour de toi :");
    // Reserve the node names first (breadth), then spend what is left on the
    // grouped action bullets (depth). Byte lengths, like the original budget.
    let names_cost: usize = nodes.iter().map(|n| n.name.len() + 1).sum();
    let mut action_budget = TEXT_BUDGET
        .saturating_sub(header.len())
        .saturating_sub(names_cost);

    let mut out = header;
    let mut full = false;
    for node in nodes {
        out.push('\n');
        out.push_str(&node.name);
        if full {
            continue;
        }
        for action in &node.actions {
            let line = format!("\n  · {action}");
            if line.len() > action_budget {
                full = true;
                break;
            }
            out.push_str(&line);
            action_budget -= line.len();
        }
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn look_at_straight_ahead_is_zero_yaw_zero_pitch() {
        // Ground forward is -Z, so an eye looking toward -Z has yaw 0, pitch 0.
        let (yaw, pitch) = orientation_from_look_at([0.0, 0.0, 0.0], [0.0, 0.0, -1.0]);
        assert!(yaw.abs() < 1e-9);
        assert!(pitch.abs() < 1e-9);
    }

    #[test]
    fn bearing_maps_the_four_cardinals() {
        // Facing -Z (yaw 0): -Z ahead, +Z behind, +X right, -X left.
        assert_eq!(bearing([0.0, 0.0, 0.0], 0.0, [0.0, 0.0, -1.0]), "ahead");
        assert_eq!(bearing([0.0, 0.0, 0.0], 0.0, [0.0, 0.0, 1.0]), "behind you");
        assert_eq!(bearing([0.0, 0.0, 0.0], 0.0, [1.0, 0.0, 0.0]), "to your right");
        assert_eq!(bearing([0.0, 0.0, 0.0], 0.0, [-1.0, 0.0, 0.0]), "to your left");
    }

    #[test]
    fn render_text_lays_out_bulleted_prose() {
        // The caller hands render_text already-grouped node lines (names + verbs
        // derived from the store, no hardcoded dictionary); it lays them out: a
        // prose situation line, then each node NAME on its own line with its
        // actions as `  · verbe` bullets.
        let nodes = vec![
            NodeLine {
                name: "Sky toolkit v0".into(),
                actions: vec!["observe".into(), "add star".into()],
            },
            NodeLine { name: "Underground toolkit".into(), actions: Vec::new() },
        ];
        let text = render_text(Some("Mind Universe"), 42, true, "Sky toolkit v0", &nodes);
        assert!(
            text.starts_with("Tu es près de Sky toolkit v0, à Mind Universe (rev 42)."),
            "situated prose: {text}"
        );
        assert!(text.contains("\n\nAutour de toi :"), "grouping header: {text}");
        assert!(text.contains("\nSky toolkit v0"), "named node line: {text}");
        assert!(text.contains("\n  · observe"), "action bullet: {text}");
        assert!(text.contains("\n  · add star"), "multi-word action bullet: {text}");
        // A node with no actions renders as a bare name line (no dangling ` : `).
        assert!(text.contains("\nUnderground toolkit"), "bare name line: {text}");
        assert!(!text.contains(" : "), "no inline action separator: {text}");
        assert!(!text.to_lowercase().contains("sphere"), "no sphere wording: {text}");
    }

    #[test]
    fn render_text_omits_place_when_none_and_reads_external() {
        // No derivable place -> the place clause is omitted entirely (never a
        // hardcoded default), and the external vantage reads as observing.
        let text = render_text(None, 3, false, "Root", &[]);
        assert!(text.starts_with("Tu observes Root (rev 3)."), "no place clause: {text}");
        assert!(!text.contains(", à "), "place clause omitted: {text}");
        assert!(text.contains("Rien n'est à portée dans ton champ perceptif."));
    }
}

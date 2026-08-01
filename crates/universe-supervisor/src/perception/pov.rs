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

/// One line of the human render: a readable name plus its action phrases,
/// humanised generically from its affordance verbs (empty when the line carries
/// none — then only the name is shown). The `observe`-builder fills this from
/// `objects` + `affordances`, deriving both name and verbs from the store's own
/// data; the text stays a projection of the structured fields, never a second
/// source of truth.
///
/// A line is not always ONE entity. The builder collapses siblings — a construct's
/// anatomy faces, a moment stream, the accumulated session bodies — into a single
/// line and unions their actions, and a line standing for several carries its own
/// COUNT in its `name` (the builder's job, never this renderer's). So this
/// renderer must never phrase a line as a singular thing, and never count lines
/// as if they were entities: the un-collapsed set lives in `objects`/`affordances`.
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
/// `origin_is_perceiver` says whether the named origin IS the embodied actor —
/// the caller's fact, since only it resolved both. It decides between being
/// somewhere and being NEAR something: the renderer cannot infer it from the name.
///
/// The budget favours breadth: every node NAME always renders, and the grouped
/// actions fill whatever budget remains (stopping once the next bullet would
/// overflow) so a long action list never crowds out node names.
pub fn render_text(
    place: Option<&str>,
    revision: u64,
    situated: bool,
    origin_is_perceiver: bool,
    origin_name: &str,
    nodes: &[NodeLine],
) -> String {
    // The place clause is added only when a place could be derived from the data.
    let at = place.map(|p| format!(", à {p}")).unwrap_or_default();
    // The origin is what the prose NAMES; the perceiver is who reads it. They
    // coincide on the default `sense` path (no `where` -> the origin falls back to
    // the embodied actor), and there "près de" would place the reader beside
    // itself. Proximity is only sayable about someone ELSE.
    let prose = match (situated, origin_is_perceiver) {
        (true, true) => format!("Tu es {origin_name}{at} (rev {revision})."),
        (true, false) => format!("Tu es près de {origin_name}{at} (rev {revision})."),
        (false, _) => format!("Tu observes {origin_name}{at} (rev {revision})."),
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
        let text = render_text(Some("Mind Universe"), 42, true, false, "Sky toolkit v0", &nodes);
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
    fn collapsing_sibling_lines_frees_breadth_budget_for_real_actions() {
        // The renderer reserves every NAME first and spends what is left on action
        // bullets, so a wall of sibling name lines starves the real constructs.
        // This is the layout half of the fix: hand it the SAME world once as 30
        // sibling lines and once as the caller's single collapsed line, and the
        // freed bytes must land on action bullets. The renderer does no grouping —
        // it only shows that grouping pays.
        let construct = |i: usize| NodeLine {
            name: format!("Toolkit v{i} (a real construct with a long authored display name)"),
            actions: (0..8).map(|a| format!("action verbe numero {a} de ce toolkit")).collect(),
        };
        let constructs: Vec<NodeLine> = (0..4).map(construct).collect();

        let mut sprawled = vec![NodeLine {
            name: "Balise".into(),
            actions: vec!["resolve spatial fix".into()],
        }];
        for i in 0..30 {
            sprawled.push(NodeLine {
                name: format!("Claude 0761f8bb e96d 418a b324 2cc135cb9a{i:02}"),
                actions: Vec::new(),
            });
        }
        sprawled.extend(constructs.iter().cloned());

        let mut collapsed = vec![sprawled[0].clone()];
        collapsed.push(NodeLine {
            name: "30 corps de session (repliés en une ligne — chacun reste listé dans objects)"
                .into(),
            actions: Vec::new(),
        });
        collapsed.extend(constructs.iter().cloned());

        let before = render_text(Some("Lumina Prime"), 291, true, false, "Balise", &sprawled);
        let after = render_text(Some("Lumina Prime"), 291, true, false, "Balise", &collapsed);

        // Both renders honour the same budget — collapsing frees room, it never
        // buys extra room. (The budget bounds the ACTION spend; names always
        // render, so a world of nothing but names could still exceed it.)
        assert!(before.len() <= TEXT_BUDGET, "before within budget: {}", before.len());
        assert!(after.len() <= TEXT_BUDGET, "after within budget: {}", after.len());
        assert!(after.len() < before.len(), "the collapsed render is the shorter one");

        let bullets = |t: &str| t.matches("\n  · ").count();
        assert!(
            bullets(&after) > bullets(&before),
            "the freed name budget must land on action bullets: {} -> {}",
            bullets(&before),
            bullets(&after)
        );
        // Concretely: the LAST construct's actions were starved before and render
        // after — the whole point of freeing the budget.
        let last = "Toolkit v3";
        assert!(after.contains(last) && before.contains(last), "names always render");
        let tail = |t: &str| t.split(last).nth(1).unwrap_or("").to_owned();
        assert_eq!(bullets(&tail(&before)), 0, "starved before: {}", tail(&before));
        assert!(bullets(&tail(&after)) > 0, "fed after: {}", tail(&after));
    }

    #[test]
    fn a_collapsed_line_renders_its_count_verbatim_and_never_as_one_thing() {
        // The renderer lays out what it is handed and does not re-word it: a line
        // standing for a crowd keeps its COUNT, so the reader cannot take it for a
        // single inhabitant. The renderer must not silently singularise it, and
        // must not add a bullet the caller did not union onto it.
        let nodes = vec![
            NodeLine {
                name: "31 corps de session (repliés en une ligne — chacun reste listé dans objects)"
                    .into(),
                actions: Vec::new(),
            },
            NodeLine { name: "Energy pen v0".into(), actions: vec!["capture gesture".into()] },
        ];
        let text = render_text(Some("Lumina Prime"), 291, true, false, "Balise", &nodes);
        assert!(
            text.contains(
                "\n31 corps de session (repliés en une ligne — chacun reste listé dans objects)"
            ),
            "the count renders verbatim on its own line: {text}"
        );
        // One line, not 31 — and the crowd never grows a bullet of its own.
        assert_eq!(text.matches("corps de session").count(), 1, "exactly one line: {text}");
        assert!(text.contains("\nEnergy pen v0\n  · capture gesture"), "real construct: {text}");
    }

    #[test]
    fn render_text_omits_place_when_none_and_reads_external() {
        // No derivable place -> the place clause is omitted entirely (never a
        // hardcoded default), and the external vantage reads as observing.
        let text = render_text(None, 3, false, false, "Root", &[]);
        assert!(text.starts_with("Tu observes Root (rev 3)."), "no place clause: {text}");
        assert!(!text.contains(", à "), "place clause omitted: {text}");
        assert!(text.contains("Rien n'est à portée dans ton champ perceptif."));
    }

    #[test]
    fn the_perceiver_is_never_rendered_as_standing_near_itself() {
        // The DEFAULT `sense` call names no `where`, so the origin falls back to
        // the embodied actor and the prose names the reader. "Près de" asserts a
        // proximity relation, and nothing is near itself: being somewhere is the
        // only thing sayable there. Same origin, same world — only the predicate
        // the CALLER resolved differs.
        let nodes = vec![NodeLine {
            name: "Energy pen v0".into(),
            actions: vec!["capture gesture".into()],
        }];
        let me = "Claude 68a7630b a6dd 4846 bbe0 5a285238011e";

        let own = render_text(Some("Mind"), 302, true, true, me, &nodes);
        assert!(own.starts_with(&format!("Tu es {me}, à Mind (rev 302).")), "self prose: {own}");
        assert!(!own.contains("près de"), "nothing is near itself: {own}");

        // A NAMED `where` still reads as proximity: the fix must not flatten the
        // distinction, only stop misapplying it.
        let other = render_text(Some("Mind"), 302, true, false, "Energy pen v0", &nodes);
        assert!(other.starts_with("Tu es près de Energy pen v0, à Mind (rev 302)."), "{other}");

        // An unplaced perceiver observes from outside; that vantage wins over the
        // identity, because an external observer is beside nothing at all.
        let outside = render_text(Some("Mind"), 302, false, true, me, &nodes);
        assert!(outside.starts_with(&format!("Tu observes {me}, à Mind")), "{outside}");
        assert!(!outside.contains("près de"), "{outside}");

        // The rest of the render is untouched by the predicate.
        assert!(own.contains("\n\nAutour de toi :"), "{own}");
        assert!(own.contains("\nEnergy pen v0\n  · capture gesture"), "{own}");
    }
}

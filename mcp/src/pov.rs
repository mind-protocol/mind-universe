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

/// The text of sense lists at most this many nearest spheres; the rest stay in
/// the structured `objects` field. Keeps the human render readable while the
/// bounded full set remains machine-available.
pub const TEXT_SPHERE_CAP: usize = 12;

/// Renders the first-person "text of sense": the actor and the nearest spheres
/// around it, by bearing and distance. `sightings` must be sorted by distance.
pub fn render_text(
    actor_label: &str,
    pov: &Pov,
    universe: &str,
    revision: u64,
    tick: u64,
    sightings: &[SphereSighting],
    uncertainty: &str,
) -> String {
    let mut out = String::new();
    let situated = match pov.eye_source {
        "external_observer" => "an external observer of this neighbourhood",
        _ => "situated",
    };
    out.push_str(&format!(
        "You are {actor_label} ({situated}) in universe {universe} at revision {revision}, tick {tick}.\n"
    ));
    if sightings.is_empty() {
        out.push_str("Nothing is within your bounded field of view.\n");
    } else {
        let shown = sightings.len().min(TEXT_SPHERE_CAP);
        out.push_str(&format!(
            "Around you, {} spheres (nearest {shown}):\n",
            sightings.len()
        ));
        for s in &sightings[..shown] {
            out.push_str(&format!(
                "  - {} ({}) — ~{:.1}m {}\n",
                s.label, s.primitive, s.distance_m, s.bearing
            ));
        }
        if sightings.len() > shown {
            out.push_str(&format!("  … and {} more further out.\n", sightings.len() - shown));
        }
    }
    out.push_str(&format!("Uncertainty: {uncertainty}.\n"));
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
}

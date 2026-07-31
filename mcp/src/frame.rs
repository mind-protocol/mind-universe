//! A deterministic first-person FRAME of what the actor sees.
//!
//! Perceiving is not a perfect mirror of reality — it is a bounded, inferred
//! projection, and that is exactly what an image is here. This module takes the
//! actor's POV camera (`eye`, `yaw`, `pitch`) and the placed spheres around it,
//! projects them through a pinhole, and emits a self-contained SVG. It is a
//! *reconstructible view*, never a canonical placement: neutral circles + the
//! object's identity as a label, no privileged shape or palette. The desktop
//! renderer owns art direction; this owns "the gestalt an LLM can act from".
//!
//! Honesty: the frame is tagged `inferred` in its own caption, mirrors the POV
//! math in `pov.rs` (YXZ yaw-then-pitch), and shows only what falls in front of
//! the camera inside the field of view — the rest stays in the object manifest.

use crate::pov::{Pov, SphereSighting};

/// Image plane, in pixels. `pub` so the raster backend shares the exact frame.
pub const WIDTH: f64 = 640.0;
pub const HEIGHT: f64 = 360.0;
/// Focal length in pixels — ~78° horizontal field of view over WIDTH.
const FOCAL: f64 = 400.0;
/// A sphere's on-screen radius at 1 m; it shrinks with depth (perspective).
const RADIUS_AT_1M: f64 = 90.0;

/// One projected sphere on the image plane. Shared by the SVG and raster
/// backends so both draw the *same* frame from the *same* projection.
pub struct Disc {
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub depth: f64,
    /// Nearer = brighter; a monochrome-ish shade so depth reads at a glance.
    /// An inferred cue, never a measured colour.
    pub shade: u32,
    pub label: String,
}

/// Projects the sphere world positions through the pinhole, culls what is behind
/// the camera or well off-canvas, and returns the discs sorted farthest-first so
/// nearer spheres paint last (on top). The single source of frame geometry.
pub fn project(pov: &Pov, sightings: &[SphereSighting]) -> Vec<Disc> {
    let (forward, right, up) = basis(pov.yaw, pov.pitch);
    let cx = WIDTH / 2.0;
    let cy = HEIGHT / 2.0;
    let mut discs: Vec<Disc> = Vec::new();
    for s in sightings {
        let v = [
            s.position[0] - pov.eye[0],
            s.position[1] - pov.eye[1],
            s.position[2] - pov.eye[2],
        ];
        let depth = dot(v, forward);
        if depth <= 0.05 {
            continue; // behind the camera or on the lens
        }
        let sx = cx + FOCAL * dot(v, right) / depth;
        let sy = cy - FOCAL * dot(v, up) / depth; // image y grows downward
        let r = (RADIUS_AT_1M / depth).clamp(1.5, 220.0);
        if sx < -r || sx > WIDTH + r || sy < -r || sy > HEIGHT + r {
            continue;
        }
        let shade = (200.0 - (depth * 6.0)).clamp(70.0, 200.0) as u32;
        discs.push(Disc { x: sx, y: sy, r, depth, shade, label: s.label.clone() });
    }
    discs.sort_by(|a, b| b.depth.partial_cmp(&a.depth).unwrap_or(std::cmp::Ordering::Equal));
    discs
}

/// The camera basis (forward, right, up) from a yaw/pitch, in the same
/// convention as `pov::orientation_from_look_at` (yaw about world-up, pitch
/// about local right). Straight ahead (yaw 0, pitch 0) looks down -Z.
fn basis(yaw: f64, pitch: f64) -> ([f64; 3], [f64; 3], [f64; 3]) {
    let forward = [
        -yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    ];
    // right = normalize(forward x world_up); world_up = +Y.
    let right = normalize(cross(forward, [0.0, 1.0, 0.0]));
    // true up = right x forward (already orthonormal).
    let up = cross(right, forward);
    (forward, right, up)
}

/// Projects the sphere world positions through the pinhole and returns the SVG
/// document for the frame. `sightings` should be sorted nearest-first so nearer
/// spheres paint last (on top).
pub fn render_svg(pov: &Pov, sightings: &[SphereSighting], caption: &str) -> String {
    let cx = WIDTH / 2.0;
    let cy = HEIGHT / 2.0;
    let discs = project(pov, sightings);

    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{WIDTH:.0}\" height=\"{HEIGHT:.0}\" \
         viewBox=\"0 0 {WIDTH:.0} {HEIGHT:.0}\" font-family=\"monospace\">"
    ));
    // Ground/void backdrop — a neutral horizon so the frame reads as a scene.
    svg.push_str(&format!(
        "<rect width=\"{WIDTH:.0}\" height=\"{HEIGHT:.0}\" fill=\"#0b0e14\"/>"
    ));
    svg.push_str(&format!(
        "<rect y=\"{cy:.0}\" width=\"{WIDTH:.0}\" height=\"{cy:.0}\" fill=\"#11161f\"/>"
    ));
    for d in &discs {
        // Nearer = brighter, so depth reads at a glance (inferred cue, not truth).
        let shade = d.shade;
        svg.push_str(&format!(
            "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{:.1}\" fill=\"rgb({shade},{shade},{})\" \
             stroke=\"#e6edf3\" stroke-width=\"1\" fill-opacity=\"0.85\"/>",
            d.x,
            d.y,
            d.r,
            (shade + 40).min(255),
        ));
        // Label only discs big enough to read, so the frame stays legible.
        if d.r >= 8.0 {
            svg.push_str(&format!(
                "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"11\" fill=\"#e6edf3\" \
                 text-anchor=\"middle\">{}</text>",
                d.x,
                d.y - d.r - 3.0,
                escape(&d.label),
            ));
        }
    }
    // Crosshair at the look direction, and an honesty caption.
    svg.push_str(&format!(
        "<line x1=\"{:.0}\" y1=\"{:.0}\" x2=\"{:.0}\" y2=\"{:.0}\" stroke=\"#7d8590\"/>",
        cx - 6.0, cy, cx + 6.0, cy
    ));
    svg.push_str(&format!(
        "<line x1=\"{:.0}\" y1=\"{:.0}\" x2=\"{:.0}\" y2=\"{:.0}\" stroke=\"#7d8590\"/>",
        cx, cy - 6.0, cx, cy + 6.0
    ));
    svg.push_str(&format!(
        "<text x=\"8\" y=\"18\" font-size=\"11\" fill=\"#7d8590\">{}</text>",
        escape(caption)
    ));
    svg.push_str("</svg>");
    svg
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f64; 3]) -> [f64; 3] {
    let len = dot(v, v).sqrt();
    if len < 1e-9 {
        [0.0, 0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

/// Minimal XML text escaping for labels (canonical ids are `:`/`-` safe, but
/// `&`/`<`/`>` must never break the document).
fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pov(yaw: f64, pitch: f64) -> Pov {
        Pov {
            actor: "root".into(),
            generated: false,
            eye: [0.0, 0.0, 0.0],
            eye_source: "situated",
            look_at: [0.0, 0.0, -1.0],
            yaw,
            pitch,
            projection: "physics_sphere",
        }
    }

    fn sighting(label: &str, position: [f64; 3]) -> SphereSighting {
        SphereSighting {
            key: "k".into(),
            label: label.into(),
            primitive: "sphere",
            position,
            distance_m: super::super::pov::distance([0.0, 0.0, 0.0], position),
            bearing: "ahead",
        }
    }

    #[test]
    fn a_sphere_dead_ahead_projects_to_the_frame_centre() {
        // Facing -Z, a sphere at -Z lands on the crosshair.
        let svg = render_svg(&pov(0.0, 0.0), &[sighting("balise", [0.0, 0.0, -5.0])], "test");
        assert!(svg.contains("<circle"));
        // Its centre x is the image centre (320).
        assert!(svg.contains("cx=\"320.0\""));
        assert!(svg.contains("balise"));
    }

    #[test]
    fn a_sphere_behind_the_camera_is_not_drawn() {
        // Facing -Z, a sphere at +Z is behind the actor: no circle.
        let svg = render_svg(&pov(0.0, 0.0), &[sighting("behind", [0.0, 0.0, 5.0])], "test");
        assert!(!svg.contains("<circle"));
    }

    #[test]
    fn labels_are_xml_escaped() {
        let svg = render_svg(&pov(0.0, 0.0), &[sighting("a<b&c", [0.0, 0.0, -3.0])], "test");
        assert!(svg.contains("a&lt;b&amp;c"));
        assert!(!svg.contains("a<b&c"));
    }
}

use acadrust::{
    entities::{hatch::BoundaryEdge, Entity, EntityType, Hatch},
    types::{Matrix3, Vector2, Vector3},
};
use std::f64::consts::{PI, TAU};

pub(super) fn sheet_x_bounds(entity: &EntityType) -> (f64, f64) {
    if let EntityType::Hatch(hatch) = entity {
        if let Some(bounds) = hatch_arc_x_bounds(hatch) {
            return bounds;
        }
    }
    let bounds = entity.as_entity().bounding_box();
    (bounds.min.x, bounds.max.x)
}

/// Use the X extrema of each circular or elliptic hatch sweep in world coordinates.
fn hatch_arc_x_bounds(hatch: &Hatch) -> Option<(f64, f64)> {
    let arcs: Vec<_> = hatch
        .paths
        .iter()
        .flat_map(|path| &path.edges)
        .filter_map(|edge| match edge {
            BoundaryEdge::CircularArc(arc) => Some((
                arc.center,
                Vector2::new(arc.radius, 0.0),
                1.0,
                arc.start_angle,
                arc.end_angle,
                arc.counter_clockwise,
            )),
            BoundaryEdge::EllipticArc(arc) => {
                // Ellipse parameters use the same axes in both directions.
                // Reversing a CW interval lets us evaluate it as a CCW sweep.
                let (start, end) = if arc.counter_clockwise {
                    (arc.start_angle, arc.end_angle)
                } else {
                    (arc.end_angle, arc.start_angle)
                };
                Some((
                    arc.center,
                    arc.major_axis_endpoint,
                    arc.minor_axis_ratio,
                    start,
                    end,
                    true,
                ))
            }
            _ => None,
        })
        .collect();
    if arcs.is_empty() {
        return None;
    }
    let mut remainder = hatch.clone();
    for path in &mut remainder.paths {
        path.edges.retain(|e| {
            !matches!(
                e,
                BoundaryEdge::CircularArc(_) | BoundaryEdge::EllipticArc(_)
            )
        });
    }
    let (mut min, mut max) = if remainder.paths.iter().any(|path| !path.edges.is_empty()) {
        let bounds = remainder.bounding_box();
        (bounds.min.x, bounds.max.x)
    } else {
        (f64::INFINITY, f64::NEG_INFINITY)
    };
    let ocs = Matrix3::arbitrary_axis(hatch.normal);
    let x_weight = ocs.transform_point(Vector3::new(1.0, 0.0, 0.0)).x;
    let y_weight = ocs.transform_point(Vector3::new(0.0, 1.0, 0.0)).x;

    for (center, major, minor_ratio, start, end, standard_axes) in arcs {
        let direction = if standard_axes { 1.0 } else { -1.0 };
        let delta = end - start;
        let sweep = if delta.abs() >= TAU {
            TAU
        } else {
            delta.rem_euclid(TAU)
        };
        // Only clockwise circular edges have mirrored stored angles.
        let cosine_weight = x_weight * major.x + y_weight * major.y;
        let sine_weight = direction * minor_ratio * (y_weight * major.x - x_weight * major.y);
        let extremum = sine_weight.atan2(cosine_weight);
        let center = ocs
            .transform_point(Vector3::new(center.x, center.y, hatch.elevation))
            .x;
        let x_at = |angle: f64| center + cosine_weight * angle.cos() + sine_weight * angle.sin();
        for angle in [start, end] {
            let x = x_at(angle);
            min = min.min(x);
            max = max.max(x);
        }
        for angle in [extremum, extremum + PI] {
            if (angle - start).rem_euclid(TAU) <= sweep {
                let x = x_at(angle);
                min = min.min(x);
                max = max.max(x);
            }
        }
    }
    Some((min, max))
}

#[cfg(test)]
mod tests {
    use super::*;
    use acadrust::entities::hatch::{BoundaryPath, EllipticArcEdge};

    #[test]
    fn reversing_elliptic_sweep_preserves_x_bounds() {
        let bounds = |start: f64, end: f64, counter_clockwise| {
            let mut hatch = Hatch::solid();
            let mut path = BoundaryPath::new();
            path.edges.push(BoundaryEdge::EllipticArc(EllipticArcEdge {
                center: Vector2::new(10.4, 5.0),
                major_axis_endpoint: Vector2::new(1.0, 0.6),
                minor_axis_ratio: 0.45,
                start_angle: start.to_radians(),
                end_angle: end.to_radians(),
                counter_clockwise,
            }));
            hatch.add_path(path);
            hatch_arc_x_bounds(&hatch).unwrap()
        };
        let forward = bounds(150.0, 210.0, true);
        assert_eq!(forward, bounds(210.0, 150.0, false));
        assert!((forward.0 - (10.4 - 1.0_f64.hypot(0.27))).abs() < 1e-12);
        assert!(forward.1 < 10.0);
    }
}

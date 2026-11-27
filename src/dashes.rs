use acadrust::{
    entities::EntityType,
    tables::{LineType, LineTypeElement},
    types::{Handle, Vector3},
    CadDocument,
};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use vsoft::universal_layout::SheetLayout;

const PT_TO_MM: f64 = 25.4 / 72.0;
type Point = [i64; 2];
type Segment = [Point; 2];

#[derive(Deserialize)]
pub struct BezierStroke {
    pub controls: [Point; 4],
    pub pattern: Vec<f64>,
}

pub fn restore_beziers(
    doc: &mut CadDocument,
    sheet: &SheetLayout,
    members: &[Handle],
    strokes: &[BezierStroke],
) -> acadrust::Result<usize> {
    if strokes.is_empty() {
        return Ok(0);
    }
    let canonical = |points: [Point; 4]| {
        let mut reversed = points;
        reversed.reverse();
        points.min(reversed)
    };
    let mut by_key = HashMap::<_, Vec<_>>::new();
    for &handle in members {
        let Some(EntityType::Spline(spline)) = doc.get_entity(handle) else {
            continue;
        };
        if spline.degree != 3 || spline.control_points.len() != 4 {
            continue;
        }
        let points = std::array::from_fn(|i| {
            let p = spline.control_points[i];
            point([
                (p.x - sheet.model_offset_mm) / PT_TO_MM,
                (sheet.size_mm[1] - p.y) / PT_TO_MM,
            ])
        });
        by_key.entry(canonical(points)).or_default().push(handle);
    }
    let mut seen = HashSet::new();
    let duplicates: HashSet<_> = strokes
        .iter()
        .map(|s| canonical(s.controls))
        .filter(|k| !seen.insert(*k))
        .collect();
    let mut restored = 0;
    for stroke in strokes {
        let key = canonical(stroke.controls);
        if duplicates.contains(&key)
            || stroke.pattern.is_empty()
            || stroke.pattern.len() % 2 != 0
            || stroke.pattern.iter().any(|v| !v.is_finite() || *v <= 0.)
        {
            continue;
        }
        let Some([handle]) = by_key.get(&key).map(Vec::as_slice) else {
            continue;
        };
        let Some(EntityType::Spline(spline)) = doc.get_entity(*handle) else {
            continue;
        };
        let [a, b, c, d, e, f, g, h] = spline.knots.as_slice() else {
            continue;
        };
        let length: f64 = spline
            .control_points
            .windows(2)
            .map(|p| {
                let x = p[1].x - p[0].x;
                let y = p[1].y - p[0].y;
                x.hypot(y) / PT_TO_MM
            })
            .sum();
        if a != b
            || b != c
            || c != d
            || e != f
            || f != g
            || g != h
            || d >= e
            || spline.flags.closed
            || spline.flags.periodic
            || !spline.weights.is_empty()
            || !spline.fit_points.is_empty()
            || spline.control_points.iter().any(|p| p.z.abs() > 1e-8)
            || !length.is_finite()
            || length + 0.01 > stroke.pattern[0]
        {
            continue;
        }
        let linetype = dash_type(doc, &stroke.pattern)?;
        let common = doc.get_entity_mut(*handle).unwrap().common_mut();
        common.linetype = linetype;
        common.linetype_handle = None;
        common.linetype_scale = 1.0;
        common.raw_record = None;
        restored += 1;
    }
    Ok(restored)
}

#[derive(Deserialize)]
pub struct Stroke {
    pub endpoints: Segment,
    pub solid_overlap: bool,
    pub patterns: Vec<Option<(Vec<f64>, Point)>>,
}

fn point(coords: [f64; 2]) -> Point {
    coords.map(|value| {
        let scaled = value * 100.0;
        let half = (scaled * 2.0).round_ties_even() / 2.0;
        // Match Python's tie handling after PDF extraction and CAD unit conversion.
        let snapped = if (scaled - half).abs() <= 1e-5 {
            half
        } else {
            scaled
        };
        snapped.round_ties_even() as i64
    })
}

fn segment(start: Point, end: Point) -> Segment {
    let mut ends = [start, end];
    ends.sort_unstable();
    ends
}

fn dash_type(doc: &mut CadDocument, pattern: &[f64]) -> acadrust::Result<String> {
    let name = format!(
        "VSOFT_DASH_{}",
        pattern
            .iter()
            .map(f64::to_string)
            .collect::<Vec<_>>()
            .join("_")
    );
    if doc.line_types.get(&name).is_none() {
        let mut style = LineType::new(&name);
        style.handle = doc.allocate_handle();
        style.description = "PDF dash pattern".into();
        style.pattern_length = pattern.iter().sum::<f64>() * PT_TO_MM;
        for (index, length) in pattern.iter().enumerate() {
            let mm = length * PT_TO_MM;
            style.add_element(if index % 2 == 0 {
                LineTypeElement::dash(mm)
            } else {
                LineTypeElement::space(mm)
            });
        }
        doc.line_types.add(style)?;
    }
    Ok(name)
}

pub fn restore(
    doc: &mut CadDocument,
    sheet: &SheetLayout,
    members: &mut Vec<Handle>,
    strokes: &[Stroke],
) -> acadrust::Result<usize> {
    let candidates: HashMap<_, _> = strokes
        .iter()
        .filter_map(|stroke| {
            let pattern = stroke.patterns.first()?.as_ref()?;
            if stroke.solid_overlap || !stroke.patterns.iter().all(|p| p.as_ref() == Some(pattern))
            {
                return None;
            }
            Some((segment(stroke.endpoints[0], stroke.endpoints[1]), pattern))
        })
        .collect();
    if candidates.is_empty() {
        return Ok(0);
    }

    let from_cad = |p: Vector3| {
        point([
            (p.x - sheet.model_offset_mm) / PT_TO_MM,
            (sheet.size_mm[1] - p.y) / PT_TO_MM,
        ])
    };
    let mut matched = HashSet::new();
    let mut output = Vec::with_capacity(members.len());
    for &handle in members.iter() {
        let entity = doc.get_entity(handle).expect("sheet member must exist");
        let is_line = matches!(entity, EntityType::Line(_));
        let mut parts = match entity {
            EntityType::Line(_) => vec![entity.clone()],
            EntityType::LwPolyline(poly)
                if poly.constant_width == 0.0
                    && poly
                        .vertices
                        .iter()
                        .all(|v| v.start_width == 0.0 && v.end_width == 0.0) =>
            {
                entity.explode()
            }
            _ => Vec::new(),
        };
        let mut changed = false;
        for part in &mut parts {
            let EntityType::Line(line) = part else {
                continue;
            };
            let start = from_cad(line.start);
            let key = segment(start, from_cad(line.end));
            if let Some((pattern, pdf_start)) = candidates.get(&key) {
                if start != *pdf_start {
                    std::mem::swap(&mut line.start, &mut line.end);
                }
                line.common.linetype = dash_type(doc, pattern)?;
                line.common.linetype_handle = None;
                line.common.linetype_scale = 1.0;
                line.common.raw_record = None;
                matched.insert(key);
                changed = true;
            }
        }
        if !changed {
            output.push(handle);
        } else if is_line {
            *doc.get_entity_mut(handle).expect("sheet member must exist") = parts.remove(0);
            output.push(handle);
        } else {
            doc.remove_entity(handle);
            for part in parts {
                output.push(doc.add_entity(part)?);
            }
        }
    }
    *members = output;
    Ok(matched.len())
}

#[cfg(test)]
mod bezier_tests {
    use super::*;
    use acadrust::entities::Spline;
    use vsoft::universal_layout::{plan_sheets, SheetInput};

    #[test]
    fn restores_only_a_unique_full_spline_that_fits_the_first_dash() {
        let plan = plan_sheets(&[SheetInput {
            number: 1,
            size_pt: [100., 100.],
        }])
        .unwrap();
        let sheet = &plan[0];
        let controls = [[1000, 1000], [1010, 1000], [1020, 1010], [1030, 1010]];
        let mut spline = Spline::new();
        spline.degree = 3;
        spline.knots = vec![0., 0., 0., 0., 1., 1., 1., 1.];
        spline.control_points = controls
            .iter()
            .map(|&[x, y]| {
                Vector3::new(
                    x as f64 * PT_TO_MM / 100.,
                    sheet.size_mm[1] - y as f64 * PT_TO_MM / 100.,
                    0.,
                )
            })
            .collect();
        let mut doc = CadDocument::new();
        let handle = doc.add_entity(EntityType::Spline(spline.clone())).unwrap();
        let duplicate = doc.add_entity(EntityType::Spline(spline.clone())).unwrap();
        let short = BezierStroke {
            controls,
            pattern: vec![0.1, 0.1],
        };
        assert_eq!(
            restore_beziers(&mut doc, sheet, &[handle], &[short]).unwrap(),
            0
        );
        let long = || BezierStroke {
            controls,
            pattern: vec![1., 1.],
        };
        assert_eq!(
            restore_beziers(&mut doc, sheet, &[handle, duplicate], &[long()]).unwrap(),
            0
        );
        assert_eq!(
            restore_beziers(&mut doc, sheet, &[handle], &[long()]).unwrap(),
            1
        );
        let EntityType::Spline(result) = doc.get_entity(handle).unwrap() else {
            panic!()
        };
        assert_eq!(result.control_points, spline.control_points);
        assert!(result.common.linetype.starts_with("VSOFT_DASH_"));
    }
}

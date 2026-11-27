use super::{sheet_layers, text::Span};
mod outlines;
use acadrust::{entities::EntityType, types::Handle, CadDocument};
use std::error::Error;
use vsoft::universal_layout::SheetLayout;

pub(super) const LAYER: &str = "VSOFT_STAMP";
const EPS: f64 = 0.35;

#[derive(Clone, Copy)]
pub(super) struct Rect(pub [f64; 4]);

impl Rect {
    pub(super) fn contains(self, other: Self, margin: f64) -> bool {
        let [x0, y0, x1, y1] = self.0;
        let [a, b, c, d] = other.0;
        a >= x0 + margin && b >= y0 + margin && c <= x1 - margin && d <= y1 - margin
    }
}

pub(super) fn bounds(entity: &EntityType) -> Rect {
    let b = entity.as_entity().bounding_box();
    Rect([b.min.x, b.min.y, b.max.x, b.max.y])
}

fn field_kind(value: &str) -> Option<&'static str> {
    let label = value.trim().trim_end_matches(':').trim().to_lowercase();
    let normalized: String = label
        .chars()
        .map(|c| match c {
            'a' => 'а',
            'c' => 'с',
            'e' => 'е',
            'k' => 'к',
            'o' => 'о',
            'p' => 'р',
            other => other,
        })
        .collect();
    match (label.as_str(), normalized.as_str()) {
        ("client" | "customer", _) | (_, "заказчик" | "заказчик проекта") => {
            Some("client")
        }
        ("project" | "project name", _) | (_, "проект" | "проект имя" | "название проекта") => {
            Some("project")
        }
        _ => None,
    }
}

fn in_separate_rows(first: Rect, second: Rect, region: Rect, lines: &[Rect]) -> bool {
    let [a, b, c, d] = first.0;
    let [e, f, g, h] = second.0;
    (a - e).abs() <= 1.0
        && lines.iter().any(|line| {
            let [x0, y0, x1, y1] = line.0;
            region.contains(*line, -EPS)
                && y1 - y0 <= EPS
                && x0 <= a.min(e) + EPS
                && x1 >= c.max(g) - EPS
                && y0 >= d.min(h) - EPS
                && y1 <= b.max(f) + EPS
        })
}

pub(super) fn assign(
    doc: &mut CadDocument,
    sheet: &SheetLayout,
    members: &[Handle],
    spans: &[Span],
) -> Result<Option<Rect>, Box<dyn Error>> {
    let Some(frame) = members
        .iter()
        .filter_map(|id| doc.get_entity(*id))
        .find(|entity| entity.common().layer == "VSOFT_FRAME")
    else {
        return Ok(None);
    };
    let Rect([left, bottom, right, _]) = bounds(frame);
    let mut lines = Vec::new();
    let mut contours = Vec::new();
    for entity in members.iter().filter_map(|id| doc.get_entity(*id)) {
        match entity {
            EntityType::Line(_) => lines.push(bounds(entity)),
            EntityType::LwPolyline(polyline)
                if !polyline.is_closed && polyline.normal == acadrust::types::Vector3::UNIT_Z =>
            {
                if (2..=32).contains(&polyline.vertices.len()) {
                    contours.push((bounds(entity), polyline));
                }
                for pair in polyline.vertices.windows(2) {
                    let a = pair[0].location;
                    let b = pair[1].location;
                    if pair[0].bulge == 0.0 {
                        lines.push(Rect([
                            a.x.min(b.x),
                            a.y.min(b.y),
                            a.x.max(b.x),
                            a.y.max(b.y),
                        ]));
                    }
                }
            }
            _ => {}
        }
    }
    let mut candidates = Vec::new();
    for Rect([x0, y0, x1, y1]) in lines.iter().copied() {
        let height = y0 - bottom;
        if y1 - y0 > EPS
            || !(12.0..=65.0).contains(&height)
            || x1 - x0 < 65.0
            || ((x0 - left).abs() > EPS && (x1 - right).abs() > EPS)
        {
            continue;
        }
        let region = Rect([x0, bottom, x1, y1]);
        let mut columns = Vec::new();
        let mut rows = Vec::new();
        for line in &lines {
            if !region.contains(*line, -EPS) {
                continue;
            }
            let [a, b, c, d] = line.0;
            if c - a <= EPS
                && d - b >= height * 0.7
                && a > x0 + EPS
                && c < x1 - EPS
                && !columns.iter().any(|x: &f64| (*x - a).abs() <= EPS)
            {
                columns.push(a);
            }
            if d - b <= EPS
                && c - a >= 8.0
                && b > bottom + EPS
                && d < y0 - EPS
                && !rows.iter().any(|y: &f64| (*y - b).abs() <= EPS)
            {
                rows.push(b);
            }
        }
        // Title blocks have differently sized subdivision rows and several
        // editable labels. A regular schedule at the sheet edge is insufficient.
        let uneven = lines.iter().any(|line| {
            let [a, b, c, d] = line.0;
            region.contains(*line, -EPS)
                && d - b <= EPS
                && b > bottom + EPS
                && d < y0 - EPS
                && (8.0..(x1 - x0 - EPS)).contains(&(c - a))
        });
        let labels: Vec<_> = spans
            .iter()
            .filter_map(|span| {
                let bounds = span.bounds(sheet);
                region
                    .contains(bounds, -EPS)
                    .then_some((field_kind(&span.value)?, bounds))
            })
            .collect();
        let has_fields = labels.iter().enumerate().any(|(index, (kind, first))| {
            labels[index + 1..].iter().any(|(other_kind, second)| {
                kind != other_kind && in_separate_rows(*first, *second, region, &lines)
            })
        });
        let has_side = |x: f64| {
            (x - left).abs() <= EPS
                || (x - right).abs() <= EPS
                || lines.iter().any(|line| {
                    let [a, b, c, d] = line.0;
                    (a - x).abs() <= EPS
                        && (c - x).abs() <= EPS
                        && (b - bottom).abs() <= EPS
                        && (d - y0).abs() <= EPS
                })
        };
        if columns.len() >= 3
            && rows.len() >= 3
            && uneven
            && has_side(x0)
            && has_side(x1)
            && (has_fields || outlines::has_sheet_headers(region, &lines, &contours))
            && !candidates.iter().any(|other: &Rect| {
                other
                    .0
                    .iter()
                    .zip(region.0)
                    .all(|(a, b)| (a - b).abs() <= EPS)
            })
        {
            candidates.push(region);
        }
    }
    let [region] = candidates.as_slice() else {
        return Ok(None);
    };
    let handles: Vec<_> = members
        .iter()
        .copied()
        .filter(|id| {
            doc.get_entity(*id).is_some_and(|entity| {
                entity.common().layer != "VSOFT_FRAME"
                    && matches!(
                        entity,
                        EntityType::Line(_)
                            | EntityType::LwPolyline(_)
                            | EntityType::Solid(_)
                            | EntityType::Hatch(_)
                    )
                    && region.contains(bounds(entity), -EPS)
            })
        })
        .collect();
    let mut moved = false;
    for handle in handles {
        moved |= sheet_layers::move_to_layer(doc, handle, LAYER)?;
    }
    Ok(moved.then_some(*region))
}

#[cfg(test)]
mod tests {
    use super::*;
    use acadrust::{entities::LwPolyline, tables::Layer, types::Vector2};
    use vsoft::universal_layout::{plan_sheets, SheetInput};

    #[test]
    fn table_with_merged_cells_touching_frame_remains_on_import_layer() {
        let sheets = plan_sheets(&[SheetInput {
            number: 1,
            size_pt: [842., 595.],
        }])
        .unwrap();
        let mut doc = CadDocument::new();
        let mut frame_layer = Layer::new("VSOFT_FRAME");
        frame_layer.handle = doc.allocate_handle();
        doc.layers.add(frame_layer).unwrap();
        let mut frame = LwPolyline::from_points(vec![
            Vector2::new(5., 5.),
            Vector2::new(290., 5.),
            Vector2::new(290., 205.),
            Vector2::new(5., 205.),
        ]);
        frame.is_closed = true;
        frame.common.layer = "VSOFT_FRAME".into();
        let mut members = vec![doc.add_entity(EntityType::LwPolyline(frame)).unwrap()];
        let mut segment = |a, b| {
            members.push(
                doc.add_entity(EntityType::LwPolyline(LwPolyline::from_points(vec![a, b])))
                    .unwrap(),
            );
        };
        for x in (170..=290).step_by(30) {
            segment(Vector2::new(x as f64, 5.), Vector2::new(x as f64, 53.));
        }
        for y in (5..=53).step_by(12) {
            let end = if y == 29 { 230. } else { 290. };
            segment(Vector2::new(170., y as f64), Vector2::new(end, y as f64));
        }
        let spans = (0..4)
            .map(|index| {
                let x = 175. + f64::from(index) * 30.;
                let value = ["Проект", "Лист", "item", "item"][index as usize];
                serde_json::from_value(serde_json::json!({
                    "value":value,
                    "origin":[x, 165.], "bbox":[x,163.,x+8.,166.],
                    "cap_height":2., "width_factor":1., "rotation":0.
                }))
                .unwrap()
            })
            .collect::<Vec<Span>>();
        assert!(assign(&mut doc, &sheets[0], &members, &spans)
            .unwrap()
            .is_none());
        assert!(doc.layers.get(LAYER).is_none());
        assert!(members[1..]
            .iter()
            .all(|handle| doc.get_entity(*handle).unwrap().common().layer == "0"));
    }

    #[test]
    fn title_fields_need_distinct_meanings_and_a_cell_divider() {
        assert_eq!(field_kind("пpoeкт имя:"), Some("project"));
        assert_eq!(field_kind("Заказчик:"), Some("client"));
        assert_eq!(field_kind("№"), None);
        assert_eq!(field_kind("Лист"), None);
        let first = Rect([10., 4., 15., 7.]);
        let second = Rect([10., 10., 15., 13.]);
        let region = Rect([0., 0., 40., 20.]);
        assert!(!in_separate_rows(
            first,
            second,
            region,
            &[Rect([0., 8., 8., 8.])]
        ));
        assert!(in_separate_rows(
            first,
            second,
            region,
            &[Rect([0., 8., 20., 8.])]
        ));
        let neighbor = Rect([20., 4., 25., 7.]);
        assert!(!in_separate_rows(
            first,
            neighbor,
            region,
            &[Rect([17., 0., 17., 20.])]
        ));
    }
}

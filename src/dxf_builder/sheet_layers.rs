use acadrust::{
    entities::EntityType,
    tables::Layer,
    types::{Color, Handle, LineWeight, Transparency, Vector3},
    CadDocument,
};
use std::error::Error;
use vsoft::universal_layout::SheetLayout;

const FRAME: &str = "VSOFT_FRAME";
const TOLERANCE_MM: f64 = 0.2;
const PAPER_EDGE_TOLERANCE_MM: f64 = 25.4 / 72.0;

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOLERANCE_MM
}

fn frame_candidate(
    entity: &EntityType,
    sheet: &SheetLayout,
    doc: &CadDocument,
    members: &[Handle],
) -> bool {
    let EntityType::LwPolyline(polyline) = entity else {
        return false;
    };
    if polyline.vertices.len() != 4
        || polyline.normal != Vector3::UNIT_Z
        || polyline.vertices.iter().any(|vertex| vertex.bulge != 0.0)
    {
        return false;
    }
    let [a, b, c, d] = std::array::from_fn(|index| polyline.vertices[index].location);
    let [width, height] = sheet.size_mm;
    let axis_aligned =
        |a: acadrust::types::Vector2, b: acadrust::types::Vector2| near(a.x, b.x) ^ near(a.y, b.y);
    if ![(a, b), (b, c), (c, d), (d, a)]
        .into_iter()
        .all(|(a, b)| axis_aligned(a, b))
    {
        return false;
    }
    let left = a.x.min(c.x) - sheet.model_offset_mm;
    let right = width - (a.x.max(c.x) - sheet.model_offset_mm);
    let bottom = a.y.min(c.y);
    let top = height - a.y.max(c.y);
    [left, right, bottom, top]
        .into_iter()
        .all(|margin| (-PAPER_EDGE_TOLERANCE_MM..=25.0).contains(&margin))
        && (a.x - c.x).abs() >= width * 0.85
        && (a.y - c.y).abs() >= height * 0.85
        && (polyline.is_closed || {
            // A clipped top is credible when matching crossbars bound the
            // drawing area above and below, both joining the same side rails.
            if !near(a.y, d.y) || top.abs() > PAPER_EDGE_TOLERANCE_MM || a.y <= b.y {
                return false;
            }
            let mut upper = false;
            let mut lower = false;
            for member in members {
                let Some(EntityType::LwPolyline(bar)) = doc.get_entity(*member) else {
                    continue;
                };
                if bar.is_closed
                    || bar.common.layer != polyline.common.layer
                    || bar.normal != polyline.normal
                {
                    continue;
                }
                let [start, end] = bar.vertices.as_slice() else {
                    continue;
                };
                let (p, q) = (start.location, end.location);
                if start.bulge != 0.0
                    || !near(p.y, q.y)
                    || !near(p.x.min(q.x), a.x.min(d.x))
                    || !near(p.x.max(q.x), a.x.max(d.x))
                {
                    continue;
                }
                upper |= (1.0..=10.0).contains(&(a.y - p.y));
                lower |= (10.0..=30.0).contains(&(p.y - b.y));
            }
            upper && lower
        })
}

pub(super) fn assign_frame(
    doc: &mut CadDocument,
    sheet: &SheetLayout,
    members: &[Handle],
) -> Result<(), Box<dyn Error>> {
    let mut candidates = members.iter().copied().filter(|handle| {
        doc.get_entity(*handle)
            .is_some_and(|entity| frame_candidate(entity, sheet, doc, members))
    });
    let Some(handle) = candidates.next() else {
        return Ok(());
    };
    if candidates.next().is_some() {
        return Ok(());
    }
    move_to_layer(doc, handle, FRAME)?;
    Ok(())
}

pub(super) fn move_to_layer(
    doc: &mut CadDocument,
    handle: Handle,
    name: &str,
) -> Result<bool, Box<dyn Error>> {
    let source = &doc.get_entity(handle).unwrap().common().layer;
    let Some(source) = doc.layers.get(source).cloned() else {
        return Ok(false);
    };
    let target = doc
        .layers
        .get(name)
        .cloned()
        .unwrap_or_else(|| Layer::new(name));
    if source.is_off()
        || source.is_frozen()
        || target.is_off()
        || target.is_frozen()
        || source.is_plottable != target.is_plottable
        || source.plotstyle_handle != target.plotstyle_handle
        || source.plot_style != target.plot_style
        || source.material != target.material
    {
        return Ok(false);
    }
    if doc.layers.get(name).is_none() {
        let mut target = target;
        target.handle = doc.allocate_handle();
        doc.layers.add(target)?;
    }
    let common = doc.get_entity_mut(handle).unwrap().common_mut();
    if common.color == Color::ByLayer {
        common.color = source.color;
        common.color_name = source.color_name;
    }
    if common.linetype.is_empty() || common.linetype.eq_ignore_ascii_case("ByLayer") {
        common.linetype = source.line_type;
        common.linetype_handle = None;
    }
    if common.line_weight == LineWeight::ByLayer {
        common.line_weight = source.line_weight;
    }
    if common.transparency == Transparency::ByLayer {
        common.transparency = source.transparency;
    }
    common.layer = name.into();
    common.layer_handle = None;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use acadrust::{entities::LwPolyline, types::Vector2};
    use vsoft::universal_layout::{plan_sheets, SheetInput};

    fn contour(sheet: &SheetLayout, layer: &str, closed: bool) -> EntityType {
        let [width, height] = sheet.size_mm;
        let left = sheet.model_offset_mm + 5.0;
        let right = sheet.model_offset_mm + width - 5.0;
        let mut polyline = LwPolyline::from_points(vec![
            Vector2::new(left, height),
            Vector2::new(left, 5.0),
            Vector2::new(right, 5.0),
            Vector2::new(right, height),
        ]);
        polyline.is_closed = closed;
        polyline.common.layer = layer.into();
        EntityType::LwPolyline(polyline)
    }

    #[test]
    fn frame_style_is_independent_of_sheet_order() {
        for numbers in [[1, 2], [2, 1]] {
            let plan = plan_sheets(&numbers.map(|number| SheetInput {
                number,
                size_pt: [842., 595.],
            }))
            .unwrap();
            let mut doc = CadDocument::new();
            for sheet in &plan {
                let name = format!("IMPORT_{}", sheet.number);
                let mut layer = Layer::new(&name);
                layer.color = Color::from_index(sheet.number as i16);
                layer.line_type = if sheet.number == 1 {
                    "DASHED"
                } else {
                    "Continuous"
                }
                .into();
                layer.line_weight = LineWeight::Value(sheet.number as i16 * 10);
                layer.handle = doc.allocate_handle();
                doc.layers.add(layer.clone()).unwrap();
                let mut frame = contour(sheet, &name, true);
                frame.common_mut().linetype = "ByLayer".into();
                let handle = doc.add_entity(frame).unwrap();
                assign_frame(&mut doc, sheet, &[handle]).unwrap();
                let common = doc.get_entity(handle).unwrap().common();
                assert_eq!(common.layer, FRAME);
                assert_eq!(common.color, layer.color);
                assert_eq!(common.linetype, layer.line_type);
                assert_eq!(common.line_weight, layer.line_weight);
            }
        }
    }

    #[test]
    fn open_contour_at_paper_edge_stays_on_import_layer() {
        let plan = plan_sheets(&[SheetInput {
            number: 1,
            size_pt: [842., 595.],
        }])
        .unwrap();
        let mut doc = CadDocument::new();
        let handle = doc.add_entity(contour(&plan[0], "0", false)).unwrap();
        assign_frame(&mut doc, &plan[0], &[handle]).unwrap();
        assert_eq!(doc.get_entity(handle).unwrap().common().layer, "0");
        assert!(doc.layers.get(FRAME).is_none());
    }

    #[test]
    fn open_frame_with_crossbars_moves_without_moving_the_content() {
        let plan = plan_sheets(&[SheetInput {
            number: 85,
            size_pt: [1127., 762.],
        }])
        .unwrap();
        let sheet = &plan[0];
        let [width, height] = sheet.size_mm;
        let mut doc = CadDocument::new();
        let outline = doc.add_entity(contour(sheet, "0", false)).unwrap();
        let mut members = vec![outline];
        for (left, right, y) in [
            (5.0, width - 5.0, height - 4.5),
            (5.0, width - 5.0, 26.0),
            (100.0, 150.0, 100.0),
        ] {
            members.push(
                doc.add_entity(EntityType::LwPolyline(LwPolyline::from_points(vec![
                    Vector2::new(left, y),
                    Vector2::new(right, y),
                ])))
                .unwrap(),
            );
        }
        assign_frame(&mut doc, sheet, &members).unwrap();
        assert_eq!(doc.get_entity(outline).unwrap().common().layer, FRAME);
        assert!(members[1..]
            .iter()
            .all(|handle| doc.get_entity(*handle).unwrap().common().layer == "0"));
    }
}

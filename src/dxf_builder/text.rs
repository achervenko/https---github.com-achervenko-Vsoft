use super::stamp::{self, Rect};
mod cells;
use acadrust::{
    entities::{EntityType, LineSpacingStyle, MText, Text},
    types::{Handle, Vector3},
    CadDocument,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::error::Error;
use vsoft::universal_layout::SheetLayout;

#[derive(Deserialize)]
pub(super) struct Span {
    pub(super) value: String,
    origin: [f64; 2],
    cap_height: f64,
    width_factor: f64,
    rotation: f64,
    bbox: Option<[f64; 4]>,
    block: Option<usize>,
    line: Option<usize>,
}

#[derive(Default, Serialize)]
pub(super) struct TrialReport {
    pub(super) merged_groups: usize,
    merged_rows: usize,
    separate_rows: BTreeMap<&'static str, usize>,
}

impl Span {
    fn point(&self, sheet: &SheetLayout) -> Vector3 {
        Vector3::new(
            sheet.model_offset_mm + self.origin[0],
            sheet.size_mm[1] - self.origin[1],
            0.,
        )
    }

    pub(super) fn bounds(&self, sheet: &SheetLayout) -> Rect {
        let [x0, y0, x1, y1] = self.bbox.unwrap_or([
            self.origin[0],
            self.origin[1],
            self.origin[0],
            self.origin[1],
        ]);
        Rect([
            x0 + sheet.model_offset_mm,
            sheet.size_mm[1] - y1,
            x1 + sheet.model_offset_mm,
            sheet.size_mm[1] - y0,
        ])
    }
}

pub(super) fn add(
    doc: &mut CadDocument,
    sheet: &SheetLayout,
    members: &mut Vec<Handle>,
    spans: &[Span],
    stamp: Option<Rect>,
    trial: bool,
) -> Result<TrialReport, Box<dyn Error>> {
    let layer = |span: &Span| {
        if stamp.is_some_and(|region| region.contains(span.bounds(sheet), -0.35)) {
            stamp::LAYER
        } else {
            "PDF_TEXT"
        }
    };
    let mut replacements = std::collections::BTreeMap::new();
    let mut suppressed = vec![false; spans.len()];
    let mut report = TrialReport::default();
    if trial {
        let grid = cells::Grid::read(doc, members);
        let mut groups: Vec<(Rect, Vec<usize>)> = Vec::new();
        for (index, span) in spans
            .iter()
            .enumerate()
            .filter(|(_, span)| span.bbox.is_some())
        {
            if let Some(cell) = grid.cell(span.bounds(sheet)) {
                if let Some((_, rows)) =
                    groups.iter_mut().find(|(existing, _)| existing.0 == cell.0)
                {
                    rows.push(index);
                } else {
                    groups.push((cell, vec![index]));
                }
            }
        }
        let with_cell: usize = groups.iter().map(|(_, rows)| rows.len()).sum();
        report
            .separate_rows
            .insert("no_confirmed_cell", spans.len() - with_cell);
        for (cell, rows) in groups {
            let [first_index, second_index, ..] = rows.as_slice() else {
                *report
                    .separate_rows
                    .entry("single_row_in_cell")
                    .or_default() += rows.len();
                continue;
            };
            let first = &spans[*first_index];
            let height = first.cap_height;
            let gap = spans[*second_index].origin[1] - first.origin[1];
            if first.block.is_none() || !(0.95..2.5).contains(&(gap / height)) {
                *report.separate_rows.entry("incompatible_rows").or_default() += rows.len();
                continue;
            }
            let [a, b, c, d] = cell.0;
            let overlaps = |span: &Span| {
                let [x0, y0, x1, y1] = span.bounds(sheet).0;
                x0 < c && x1 > a && y0 < d && y1 > b
            };
            if spans
                .iter()
                .enumerate()
                .any(|(i, span)| overlaps(span) && !rows.contains(&i))
            {
                *report.separate_rows.entry("overlapping_text").or_default() += rows.len();
                continue;
            }
            if !rows.iter().enumerate().all(|(i, index)| {
                let row = &spans[*index];
                let expected_line = first.line.and_then(|line| line.checked_add(i));
                row.block == first.block
                    && row.line.is_some()
                    && row.line == expected_line
                    && row.rotation.abs() <= 1e-6
                    && (row.cap_height - height).abs() <= 0.03
                    && (row.origin[0] - first.origin[0]).abs() <= 0.15
                    && (row.origin[1] - first.origin[1] - i as f64 * gap).abs() <= 0.12
                    && cell.contains(row.bounds(sheet), 0.36)
                    && layer(row) == layer(first)
            }) {
                *report.separate_rows.entry("incompatible_rows").or_default() += rows.len();
                continue;
            }
            let mut text = MText::new();
            text.value = rows
                .iter()
                .map(|index| {
                    let row = &spans[*index];
                    let value = row
                        .value
                        .replace('\\', "\\\\")
                        .replace('{', "\\{")
                        .replace('}', "\\}");
                    format!("{{\\W{:.8};{value}}}", row.width_factor)
                })
                .collect::<Vec<_>>()
                .join("\\P");
            text.insertion_point = first.point(sheet) + Vector3::new(0., height, 0.);
            text.height = height;
            text.rectangle_width = 0.;
            text.line_spacing_style = LineSpacingStyle::Exactly;
            text.line_spacing_factor = gap / (height * 5. / 3.);
            text.style = "VSOFT_PDF".into();
            text.common.layer = layer(first).into();
            for index in &rows {
                suppressed[*index] = true;
            }
            report.merged_rows += rows.len();
            replacements.insert(*first_index, text);
        }
    }
    report.merged_groups = replacements.len();
    for (index, span) in spans.iter().enumerate() {
        if let Some(text) = replacements.remove(&index) {
            members.push(doc.add_entity(EntityType::MText(Box::new(text)))?);
            continue;
        }
        if suppressed[index] {
            continue;
        }
        let mut text = Text::new();
        text.value = span.value.clone();
        text.insertion_point = span.point(sheet);
        text.rotation = span.rotation;
        text.height = span.cap_height;
        text.width_factor = span.width_factor;
        text.style = "VSOFT_PDF".into();
        text.common.layer = layer(span).into();
        members.push(doc.add_entity(EntityType::Text(text))?);
    }
    Ok(report)
}

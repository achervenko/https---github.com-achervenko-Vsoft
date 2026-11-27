//! Paper geometry for the universal profile. PDF dimensions are points,
//! native AutoCAD imports use inches, and the assembled drawing uses millimetres.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

const POINTS_PER_INCH: f64 = 72.0;
const MM_PER_INCH: f64 = 25.4;
const PT_TO_MM: f64 = MM_PER_INCH / POINTS_PER_INCH;
const IMPORT_GAP_IN: f64 = 2.0;
const MODEL_GAP_MM: f64 = 20.0;
const PLOT_TOLERANCE_PT: f64 = 2.0;
const ISO_MEDIA_MM: [(u16, u16); 5] = [(841, 1189), (594, 841), (420, 594), (297, 420), (210, 297)];

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct SheetInput {
    pub number: usize,
    pub size_pt: [f64; 2],
}

#[derive(Debug, PartialEq, Serialize)]
pub struct SheetLayout {
    pub number: usize,
    pub size_mm: [f64; 2],
    pub import_offset_in: f64,
    pub import_right_boundary_in: f64,
    pub model_offset_mm: f64,
    pub paper_center_mm: [f64; 2],
    pub view_center_mm: [f64; 2],
    pub plot_media: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum LayoutError {
    Empty,
    InvalidNumber(usize),
    InvalidSize(usize),
    OffsetOverflow(usize),
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "at least one sheet is required"),
            Self::InvalidNumber(number) => write!(f, "invalid or duplicate sheet number {number}"),
            Self::InvalidSize(number) => write!(f, "sheet {number} has invalid PDF dimensions"),
            Self::OffsetOverflow(number) => write!(f, "offset for sheet {number} exceeds range"),
        }
    }
}

impl std::error::Error for LayoutError {}

fn plot_media(size_pt: [f64; 2]) -> Option<String> {
    for (index, (short, long)) in ISO_MEDIA_MM.into_iter().enumerate() {
        for (width, height) in [(short, long), (long, short)] {
            let nominal_pt = [width, height].map(|mm| f64::from(mm) / PT_TO_MM);
            if size_pt
                .into_iter()
                .zip(nominal_pt)
                .all(|(actual, nominal)| (actual - nominal).abs() <= PLOT_TOLERANCE_PT)
            {
                return Some(format!(
                    "ISO_full_bleed_A{index}_({width}.00_x_{height}.00_MM)"
                ));
            }
        }
    }
    None
}

/// Plan sheets in input order. Sizes describe the visible page, with PDF
/// rotation already reflected in its width and height.
pub fn plan_sheets(sheets: &[SheetInput]) -> Result<Vec<SheetLayout>, LayoutError> {
    if sheets.is_empty() {
        return Err(LayoutError::Empty);
    }
    let mut seen = HashSet::with_capacity(sheets.len());
    let mut layouts = Vec::with_capacity(sheets.len());
    let mut import_offset_in = 0.0;
    let mut model_offset_mm = 0.0;
    for sheet in sheets {
        let SheetInput { number, size_pt } = *sheet;
        if number == 0 || !seen.insert(number) {
            return Err(LayoutError::InvalidNumber(number));
        }
        if !size_pt.iter().all(|size| size.is_finite() && *size > 0.0) {
            return Err(LayoutError::InvalidSize(number));
        }
        let size_mm = size_pt.map(|size| size * PT_TO_MM);
        let paper_center_mm = size_mm.map(|size| size / 2.0);
        let width_in = size_pt[0] / POINTS_PER_INCH;
        let next_import_offset_in = import_offset_in + (width_in + IMPORT_GAP_IN);
        let next_model_offset_mm = model_offset_mm + (size_mm[0] + MODEL_GAP_MM);
        if !next_import_offset_in.is_finite() || !next_model_offset_mm.is_finite() {
            return Err(LayoutError::OffsetOverflow(number));
        }
        layouts.push(SheetLayout {
            number,
            size_mm,
            import_offset_in,
            import_right_boundary_in: import_offset_in + width_in + IMPORT_GAP_IN / 2.0,
            model_offset_mm,
            paper_center_mm,
            view_center_mm: [model_offset_mm + paper_center_mm[0], paper_center_mm[1]],
            plot_media: plot_media(size_pt),
        });
        import_offset_in = next_import_offset_in;
        model_offset_mm = next_model_offset_mm;
    }
    Ok(layouts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(number: usize, size_pt: [f64; 2]) -> SheetInput {
        SheetInput { number, size_pt }
    }

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-8, "{actual} != {expected}");
    }

    #[test]
    fn mixed_pages_keep_source_numbers_sizes_and_spacing() {
        let plan = plan_sheets(&[
            sheet(85, [1684.0, 1191.0]),
            sheet(2, [841.89, 1190.55]),
            sheet(4, [600.0, 1000.0]),
        ])
        .unwrap();
        assert_eq!(
            plan.iter().map(|page| page.number).collect::<Vec<_>>(),
            [85, 2, 4]
        );
        assert_eq!(
            plan[0].plot_media.as_deref(),
            Some("ISO_full_bleed_A2_(594.00_x_420.00_MM)")
        );
        assert_eq!(
            plan[1].plot_media.as_deref(),
            Some("ISO_full_bleed_A3_(297.00_x_420.00_MM)")
        );
        assert_eq!(plan[2].plot_media, None);
        close(plan[0].size_mm[0], 594.0777777778);
        close(plan[0].size_mm[1], 420.1583333333);
        close(plan[0].import_right_boundary_in, 24.3888888889);
        close(plan[1].import_offset_in, 25.3888888889);
        close(plan[1].model_offset_mm, 614.0777777778);
        close(plan[2].model_offset_mm, 931.0778611111);
        close(plan[2].paper_center_mm[0], 105.8333333333);
        close(plan[2].view_center_mm[0], 1036.9111944444);
        close(plan[2].view_center_mm[1], 176.3888888889);
    }

    #[test]
    fn media_requires_both_dimensions_in_tolerance() {
        let [width, height] = [420.0, 297.0].map(|mm| mm / PT_TO_MM);
        assert!(plot_media([width + 2.0, height - 2.0]).is_some());
        assert!(plot_media([width + 2.01, height]).is_none());
        assert!(plot_media([width, height - 2.01]).is_none());
    }

    #[test]
    fn rejects_invalid_inputs() {
        assert_eq!(plan_sheets(&[]), Err(LayoutError::Empty));
        let a2 = [1684.0, 1191.0];
        assert_eq!(
            plan_sheets(&[sheet(0, a2)]),
            Err(LayoutError::InvalidNumber(0))
        );
        assert_eq!(
            plan_sheets(&[sheet(8, a2), sheet(8, a2)]),
            Err(LayoutError::InvalidNumber(8))
        );
        for width in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                plan_sheets(&[sheet(7, [width, 1191.0])]),
                Err(LayoutError::InvalidSize(7))
            );
        }
    }
}

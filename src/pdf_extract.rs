use mupdf::{
    drawing::{Drawing, DrawingItem, DrawingType},
    Document, Point, TextCharFlags, TextPageFlags,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    path::PathBuf,
};

const MM: f64 = 25.4 / 72.;
type Key = [[i64; 2]; 2];
type Pattern = Option<(Vec<f64>, [i64; 2])>;

fn point(p: Point) -> [i64; 2] {
    [p.x, p.y].map(|v| {
        let v = f64::from(v) * 100.;
        let half = (v * 2.).round_ties_even() / 2.;
        (if (v - half).abs() <= 1e-5 { half } else { v }).round_ties_even() as i64
    })
}
fn key(a: Point, b: Point) -> Key {
    let mut k = [point(a), point(b)];
    k.sort_unstable();
    k
}

fn short_beziers(drawings: &[Drawing]) -> Vec<Value> {
    let mut by_controls = BTreeMap::new();
    for draw in drawings
        .iter()
        .filter(|d| d.drawing_type != DrawingType::Fill)
    {
        for item in &draw.items {
            let DrawingItem::Curve(a, b, c, d) = *item else {
                continue;
            };
            let controls = [a, b, c, d].map(point);
            let mut reversed = controls;
            reversed.reverse();
            let entry = by_controls
                .entry(controls.min(reversed))
                .or_insert((0, None));
            entry.0 += 1;
            let Some(dash) = &draw.dashes else {
                continue;
            };
            let length: f64 = [a, b, c, d]
                .windows(2)
                .map(|v| {
                    (f64::from(v[1].x) - f64::from(v[0].x))
                        .hypot(f64::from(v[1].y) - f64::from(v[0].y))
                })
                .sum();
            if draw.items.len() == 1
                && !draw.close_path.unwrap_or(false)
                && draw.drawing_type == DrawingType::Stroke
                && dash.phase == 0.
                && dash
                    .dashes
                    .first()
                    .is_some_and(|v| length + 0.03 <= f64::from(*v))
                && dash.dashes.iter().all(|v| v.is_finite() && *v > 0.)
            {
                let mut pattern: Vec<f64> = dash
                    .dashes
                    .iter()
                    .map(|v| v.to_string().parse().unwrap())
                    .collect();
                if pattern.len() % 2 != 0 {
                    pattern.extend_from_within(..);
                }
                entry.1 = Some(json!({"controls":controls,"pattern":pattern}));
            }
        }
    }
    by_controls
        .into_values()
        .filter_map(|(count, value)| if count == 1 { value } else { None })
        .collect()
}

fn dashes(drawings: Vec<Drawing>) -> (Vec<Value>, usize) {
    let mut solid = BTreeSet::new();
    let mut strokes: BTreeMap<Key, Vec<Pattern>> = BTreeMap::new();
    let mut unsupported = 0;
    for draw in drawings
        .into_iter()
        .filter(|d| d.drawing_type != DrawingType::Fill)
    {
        let pattern = draw.dashes.filter(|d| !d.dashes.is_empty());
        let closed = draw.close_path.unwrap_or(false);
        if let Some(pattern) = pattern {
            let mut values: Vec<f64> = pattern
                .dashes
                .iter()
                .map(|v| v.to_string().parse().unwrap())
                .collect();
            if values.len() % 2 != 0 {
                values.extend_from_within(..);
            }
            for (i, item) in draw.items.iter().enumerate() {
                if let DrawingItem::Line(a, b) = *item {
                    let valid = i == 0
                        && !closed
                        && draw.drawing_type == DrawingType::Stroke
                        && pattern.phase == 0.
                        && values.iter().all(|v| v.is_finite() && *v > 0.);
                    strokes
                        .entry(key(a, b))
                        .or_default()
                        .push(valid.then(|| (values.clone(), point(a))));
                } else {
                    unsupported += 1;
                }
            }
            continue;
        }
        let mut first = None;
        let mut last = None;
        for item in draw.items {
            match item {
                DrawingItem::Line(a, b) | DrawingItem::Curve(a, _, _, b) => {
                    if last != Some(a) {
                        first = Some(a);
                    }
                    last = Some(b);
                    if matches!(item, DrawingItem::Line(..)) {
                        solid.insert(key(a, b));
                    }
                }
                shape => {
                    let vertices = match shape {
                        DrawingItem::Rect { rect: r, .. } => [
                            Point::new(r.x0, r.y0),
                            Point::new(r.x1, r.y0),
                            Point::new(r.x1, r.y1),
                            Point::new(r.x0, r.y1),
                        ],
                        DrawingItem::Quad(q) => [q.ul, q.ur, q.lr, q.ll],
                        _ => unreachable!(),
                    };
                    for i in 0..4 {
                        solid.insert(key(vertices[i], vertices[(i + 1) % 4]));
                    }
                    first = None;
                    last = None;
                }
            }
        }
        if let (Some(a), Some(b)) = (first, last) {
            if closed && a != b {
                solid.insert(key(b, a));
            }
        }
    }
    (
        strokes
            .into_iter()
            .map(|(ends, patterns)| {
                json!({
                    "endpoints":ends,"patterns":patterns,"solid_overlap":solid.contains(&ends)
                })
            })
            .collect(),
        unsupported,
    )
}

fn continues_in_reading_direction(previous: Point, next: Point, along: Point) -> bool {
    let length = along.x.hypot(along.y);
    length == 0.0
        || ((next.x - previous.x) * along.x + (next.y - previous.y) * along.y) / length >= -0.01
}

pub fn run(args: impl Iterator<Item = std::ffi::OsString>) -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = args.collect();
    let [input, output, font, selection, dashes_flag] = args.as_slice() else {
        return Err(
            "expected PDF, output JSON, Arial path, source page numbers and dash flag".into(),
        );
    };
    let bytes = fs::read(PathBuf::from(input))?;
    let pdf = Document::from_bytes(&bytes, "pdf")?;
    let font_bytes = fs::read(PathBuf::from(font))?;
    let font = ttf_parser::Face::parse(&font_bytes, 0)?;
    let bounds = |ch| {
        font.glyph_index(ch)
            .and_then(|id| font.glyph_bounding_box(id))
            .ok_or("missing glyph bounds")
    };
    let h = f64::from(bounds('A')?.y_max - bounds('x')?.y_min);
    let numbers: Vec<usize> = selection
        .to_str()
        .ok_or("invalid sheet numbers")?
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    if numbers.len() != pdf.page_count()? as usize {
        return Err("PDF page count differs from sheet inventory".into());
    }
    let mut rows = Vec::new();
    for page_index in 0..pdf.page_count()? {
        let number = numbers[page_index as usize];
        let diag = |error: mupdf::Error| format!("Sheet {number}: {error}");
        let page = pdf.load_page(page_index).map_err(diag)?;
        let bounds = page.bounds().map_err(diag)?;
        let text = page.to_text_page(TextPageFlags::empty()).map_err(diag)?;
        let images = page
            .to_text_page(TextPageFlags::PRESERVE_IMAGES)
            .map_err(diag)?
            .blocks()
            .filter(|block| block.image().is_some())
            .count();
        let mut spans = Vec::new();
        for (block_index, block) in text.blocks().enumerate() {
            for (line_index, line) in block.lines().enumerate() {
                let chars: Vec<_> = line.chars().collect();
                let mut start = 0;
                while start < chars.len() {
                    let first = &chars[start];
                    if first.char() == Some(' ') && first.flags().contains(TextCharFlags::SYNTHETIC)
                    {
                        start += 1;
                        continue;
                    }
                    let first_font = first.font();
                    let name = first_font.as_ref().map(|f| f.name());
                    let quad = first.quad();
                    let along = Point::new(quad.ur.x - quad.ul.x, quad.ur.y - quad.ul.y);
                    let mut end = start + 1;
                    while end < chars.len()
                        && !(chars[end].char() == Some(' ')
                            && chars[end].flags().contains(TextCharFlags::SYNTHETIC))
                        && chars[end].size() == first.size()
                        && chars[end].argb() == first.argb()
                        && chars[end].font().as_ref().map(|f| f.name()) == name
                        && continues_in_reading_direction(
                            chars[end - 1].origin(),
                            chars[end].origin(),
                            along,
                        )
                    {
                        end += 1;
                    }
                    let run = &chars[start..end];
                    let value: String =
                        run.iter().map(|c| c.char().unwrap_or('\u{fffd}')).collect();
                    start = end;
                    if value.trim().is_empty() {
                        continue;
                    }
                    let q = first.quad();
                    let direction = (f64::from(q.ur.x - q.ul.x), f64::from(q.ur.y - q.ul.y));
                    let norm = direction.0.hypot(direction.1);
                    let (dx, dy) = if norm > 0. {
                        (direction.0 / norm, direction.1 / norm)
                    } else {
                        (1., 0.)
                    };
                    let origin = first.origin();
                    let cap = f64::from(first.size()) * MM * 0.72;
                    let mut box_pt = [
                        f64::INFINITY,
                        f64::INFINITY,
                        f64::NEG_INFINITY,
                        f64::NEG_INFINITY,
                    ];
                    let mut lo = f64::INFINITY;
                    let mut hi = f64::NEG_INFINITY;
                    for c in run {
                        let q = c.quad();
                        for p in [q.ul, q.ur, q.ll, q.lr] {
                            let (x, y) = (f64::from(p.x), f64::from(p.y));
                            box_pt = [
                                box_pt[0].min(x),
                                box_pt[1].min(y),
                                box_pt[2].max(x),
                                box_pt[3].max(y),
                            ];
                            let projection = x * dx + y * dy;
                            lo = lo.min(projection);
                            hi = hi.max(projection);
                        }
                    }
                    let natural = value
                        .chars()
                        .map(|c| font.glyph_index(c).unwrap_or(ttf_parser::GlyphId(0)))
                        .filter_map(|g| font.glyph_hor_advance(g))
                        .map(f64::from)
                        .sum::<f64>()
                        * cap
                        / h;
                    spans.push(json!({"value":value,"origin":[f64::from(origin.x)*MM,f64::from(origin.y)*MM],
                        "cap_height":cap,"width_factor":if natural>0. && hi>lo {(hi-lo)*MM/natural} else {1.},
                        "rotation":(-dy).atan2(dx),"bbox":box_pt.map(|v|v*MM),"block":block_index,"line":line_index}));
                }
            }
        }
        let (strokes, unsupported, beziers) = if dashes_flag == "1" {
            let drawings = page.drawings().map_err(diag)?;
            let beziers = short_beziers(&drawings);
            let (strokes, unsupported) = dashes(drawings);
            (strokes, unsupported, beziers)
        } else {
            (Vec::new(), 0, Vec::new())
        };
        rows.push(json!({"number":number,"omitted_images":images,"size_pt":[bounds.x1-bounds.x0,bounds.y1-bounds.y0],
            "spans":spans,"trial_mtext":true,"dash_strokes":strokes,"dash_beziers":beziers,"dash_unsupported":unsupported}));
    }
    serde_json::to_writer(fs::File::create(PathBuf::from(output))?, &rows)?;
    Ok(())
}

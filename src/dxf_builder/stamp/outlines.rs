use super::{Rect, EPS};
use acadrust::entities::LwPolyline;

type Contour<'a> = (Rect, &'a LwPolyline);

pub(super) fn has_sheet_headers(region: Rect, lines: &[Rect], contours: &[Contour<'_>]) -> bool {
    let [left, bottom, right, top] = region.0;
    let width = right - left;
    let height = top - bottom;
    let near = |a: f64, b: f64| (a - b).abs() <= EPS;
    let mut revision_rows: Vec<_> = lines
        .iter()
        .filter_map(|&Rect([x, lo, x1, hi])| {
            (near(lo, hi)
                && near(x, left)
                && (0.22..=0.45).contains(&((x1 - left) / width))
                && lo > bottom + EPS
                && hi < top - EPS)
                .then_some(lo)
        })
        .collect();
    revision_rows.sort_by(f64::total_cmp);
    revision_rows.dedup_by(|a, b| near(*a, *b));
    if revision_rows.len() < 5 {
        return false;
    }
    for &Rect([start, y, end, y1]) in lines {
        // The sheet counter has a horizontal divider above the number cells.
        if !near(y, y1)
            || !near(end, right)
            || !(0.18..=0.36).contains(&((end - start) / width))
            || !(0.35..=0.65).contains(&((y - bottom) / height))
        {
            continue;
        }
        for &Rect([high_start, high_min, high_end, high]) in lines {
            if !near(high_min, high)
                || high_start > start + EPS
                || high_end < end - EPS
                || !(0.06..=0.14).contains(&((high - y) / height))
            {
                continue;
            }
            let mut dividers: Vec<_> = lines
                .iter()
                .filter_map(|line| {
                    let [x, lo, x1, hi] = line.0;
                    (near(x, x1)
                        && lo < y - EPS
                        && near(hi, high)
                        && x > start + EPS
                        && x < end - EPS)
                        .then_some(x)
                })
                .collect();
            dividers.sort_by(f64::total_cmp);
            dividers.dedup_by(|a, b| near(*a, *b));
            let [a, b] = dividers.as_slice() else {
                continue;
            };
            if !near(a - start, b - a) || !(1.0..=1.6).contains(&((end - b) / (b - a))) {
                continue;
            }
            let marks = |x0, x1| {
                let cell = Rect([x0, y, x1, high]);
                let mut marks: Vec<_> = contours
                    .iter()
                    .copied()
                    .filter(|(bbox, _)| cell.contains(*bbox, EPS))
                    .collect();
                marks.sort_by(|(a, _), (b, _)| a.0[0].total_cmp(&b.0[0]));
                marks
            };
            let stage = marks(start, *a);
            let sheet = marks(*a, *b);
            let sheets = marks(*b, end);
            if stage.len() >= 5 && repeated_sheet_word(&sheet, &sheets) {
                return true;
            }
        }
    }
    false
}

fn repeated_sheet_word(short: &[Contour<'_>], long: &[Contour<'_>]) -> bool {
    if short.len() < 5 || long.len() <= short.len() {
        return false;
    }
    let offset = long[0].0 .0[0] - short[0].0 .0[0];
    let mut matches = long.iter();
    short.iter().all(|(_, path)| {
        matches.any(|(_, other)| {
            path.vertices.len() == other.vertices.len()
                && path.vertices.iter().zip(&other.vertices).all(|(a, b)| {
                    (b.location.x - a.location.x - offset).abs() < 0.12
                        && (b.location.y - a.location.y).abs() < 0.12
                        && (b.bulge - a.bulge).abs() < 0.01
                })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use acadrust::types::Vector2;

    #[test]
    fn duplicated_revision_row_cannot_confirm_sheet_headers() {
        let region = Rect([0., 0., 100., 50.]);
        let headers = [
            Rect([70., 25., 100., 25.]),
            Rect([70., 30., 100., 30.]),
            Rect([80., 20., 80., 30.]),
            Rect([90., 20., 90., 30.]),
        ];
        let mut paths = Vec::new();
        for (start, count) in [(71., 5), (81., 5), (91., 6)] {
            for index in 0..count {
                let x = start + f64::from(index);
                paths.push((
                    Rect([x, 26., x + 0.25, 28.]),
                    LwPolyline::from_points(vec![
                        Vector2::new(x, 26.),
                        Vector2::new(x + 0.25, 28.),
                    ]),
                ));
            }
        }
        let contours: Vec<_> = paths.iter().map(|(bounds, path)| (*bounds, path)).collect();
        let mut lines = headers.to_vec();
        lines.extend((1..=5).map(|index| {
            let y = f64::from(index) * 5.;
            Rect([0., y, 30., y])
        }));
        assert!(has_sheet_headers(region, &lines, &contours));

        lines.truncate(headers.len());
        for index in 0..5 {
            let y = 10. + f64::from(index) * EPS / 10.;
            lines.push(Rect([0., y, 30., y]));
        }
        assert!(!has_sheet_headers(region, &lines, &contours));
    }

    #[test]
    fn requires_matching_paths_and_an_extension() {
        let path =
            |x, tip| LwPolyline::from_points(vec![Vector2::new(x, 0.), Vector2::new(x + tip, 1.)]);
        let short: Vec<_> = (0..5).map(|i| path(f64::from(i), 0.25)).collect();
        let mut long: Vec<_> = (0..6).map(|i| path(10. + f64::from(i), 0.25)).collect();
        let signatures = |paths: &[LwPolyline]| {
            paths
                .iter()
                .map(|path| {
                    let x = path.vertices[0].location.x;
                    Rect([x, 0., x + 0.25, 1.])
                })
                .collect::<Vec<_>>()
        };
        let a = signatures(&short)
            .into_iter()
            .zip(&short)
            .collect::<Vec<_>>();
        let b = signatures(&long).into_iter().zip(&long).collect::<Vec<_>>();
        assert!(repeated_sheet_word(&a, &b));
        assert!(!repeated_sheet_word(&a, &b[..5]));
        long[2].vertices[1].location.x += 0.5;
        let changed = signatures(&long).into_iter().zip(&long).collect::<Vec<_>>();
        assert!(!repeated_sheet_word(&a, &changed));
    }
}

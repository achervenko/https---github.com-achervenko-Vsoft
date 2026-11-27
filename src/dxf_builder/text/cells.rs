use super::Rect;
use acadrust::{
    entities::EntityType,
    types::{Handle, Vector2, Vector3},
    CadDocument,
};

const EPS: f64 = 0.08;

#[derive(Default)]
pub(super) struct Grid {
    horizontal: Vec<Edge>,
    vertical: Vec<Edge>,
}

#[derive(Clone, Copy)]
struct Edge {
    axis: f64,
    start: f64,
    end: f64,
}

impl Grid {
    pub(super) fn read(doc: &CadDocument, members: &[Handle]) -> Self {
        let mut grid = Self::default();
        for entity in members.iter().filter_map(|id| doc.get_entity(*id)) {
            if entity.common().layer == "VSOFT_FRAME" {
                continue;
            }
            match entity {
                EntityType::Line(line) if line.start.z.abs() <= EPS && line.end.z.abs() <= EPS => {
                    grid.push(
                        Vector2::new(line.start.x, line.start.y),
                        Vector2::new(line.end.x, line.end.y),
                    );
                }
                EntityType::LwPolyline(polyline)
                    if polyline.normal == Vector3::UNIT_Z && polyline.elevation.abs() <= EPS =>
                {
                    let vertices = &polyline.vertices;
                    for (a, b) in vertices.iter().zip(vertices.iter().cycle().skip(1)).take(
                        vertices
                            .len()
                            .saturating_sub(usize::from(!polyline.is_closed)),
                    ) {
                        if a.bulge == 0.0 {
                            grid.push(a.location, b.location);
                        }
                    }
                }
                _ => {}
            }
        }
        let sort = |edges: &mut Vec<Edge>| {
            edges.sort_by(|a, b| a.start.total_cmp(&b.start));
        };
        sort(&mut grid.horizontal);
        sort(&mut grid.vertical);
        grid
    }

    fn push(&mut self, a: Vector2, b: Vector2) {
        let (dx, dy) = ((a.x - b.x).abs(), (a.y - b.y).abs());
        if dx > EPS && dy <= EPS {
            self.horizontal.push(Edge {
                axis: (a.y + b.y) * 0.5,
                start: a.x.min(b.x),
                end: a.x.max(b.x),
            });
        } else if dy > EPS && dx <= EPS {
            self.vertical.push(Edge {
                axis: (a.x + b.x) * 0.5,
                start: a.y.min(b.y),
                end: a.y.max(b.y),
            });
        }
    }

    fn covers(edges: &[Edge], axis: f64, start: f64, end: f64) -> bool {
        let mut reached = start;
        for edge in edges {
            if edge.start > reached + EPS {
                break;
            }
            if (edge.axis - axis).abs() <= EPS {
                reached = reached.max(edge.end);
                if reached >= end - EPS {
                    return true;
                }
            }
        }
        false
    }

    pub(super) fn cell(&self, bounds: Rect) -> Option<Rect> {
        let [x0, y0, x1, y1] = bounds.0;
        let nearest = |edges: &[Edge], low: f64, high: f64, start: f64, end: f64| {
            let mid = (start + end) * 0.5;
            let eligible = || {
                edges
                    .iter()
                    .filter(|e| e.start <= mid + EPS && e.end >= mid - EPS)
            };
            [
                eligible()
                    .filter(|e| e.axis < low - EPS)
                    .map(|e| e.axis)
                    .max_by(f64::total_cmp),
                eligible()
                    .filter(|e| e.axis > high + EPS)
                    .map(|e| e.axis)
                    .min_by(f64::total_cmp),
            ]
        };
        let [Some(left), Some(right)] = nearest(&self.vertical, x0, x1, y0, y1) else {
            return None;
        };
        let [Some(bottom), Some(top)] = nearest(&self.horizontal, y0, y1, x0, x1) else {
            return None;
        };
        if !Self::covers(&self.horizontal, bottom, left, right)
            || !Self::covers(&self.horizontal, top, left, right)
            || !Self::covers(&self.vertical, left, bottom, top)
            || !Self::covers(&self.vertical, right, bottom, top)
        {
            return None;
        }
        let enters = |edges: &[Edge], low, high, start, end| {
            edges.iter().any(|e| {
                e.axis > low + EPS
                    && e.axis < high - EPS
                    && e.start < end - EPS
                    && e.end > start + EPS
            })
        };
        if enters(&self.horizontal, bottom, top, left, right)
            || enters(&self.vertical, left, right, bottom, top)
        {
            return None;
        }
        Some(Rect([left, bottom, right, top]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acadrust::entities::{Line, LwPolyline};

    #[test]
    fn read_rejects_edges_outside_the_text_plane() {
        let mut doc = CadDocument::new();
        let mut members = Vec::new();
        for (start, end) in [
            ([0., 0., 0.], [10., 0., 0.]),
            ([0., 0., 0.], [0., 8., 0.]),
            ([10., 0., 0.], [10., 8., 0.]),
        ] {
            let [x, y, z] = start;
            let [a, b, c] = end;
            members.push(
                doc.add_entity(EntityType::Line(Line::from_points(
                    Vector3::new(x, y, z),
                    Vector3::new(a, b, c),
                )))
                .unwrap(),
            );
        }
        let area = Rect([2., 2., 4., 4.]);
        for (start, end) in [(1., 1.), (EPS * 0.8, EPS * 1.2), (EPS * 1.2, EPS * 0.8)] {
            let handle = doc
                .add_entity(EntityType::Line(Line::from_points(
                    Vector3::new(0., 8., start),
                    Vector3::new(10., 8., end),
                )))
                .unwrap();
            members.push(handle);
            assert!(Grid::read(&doc, &members).cell(area).is_none());
            members.pop();
        }
        for elevation in [1., EPS * 0.5] {
            let mut edge =
                LwPolyline::from_points(vec![Vector2::new(0., 8.), Vector2::new(10., 8.)]);
            edge.elevation = elevation;
            members.push(doc.add_entity(EntityType::LwPolyline(edge)).unwrap());
            assert_eq!(
                Grid::read(&doc, &members).cell(area).is_some(),
                elevation <= EPS
            );
            members.pop();
        }
    }

    #[test]
    fn confirms_cells_at_t_junctions_and_rejects_broken_sides() {
        let mut grid = Grid::default();
        for (a, b) in [
            ((0., 0.), (10., 0.)),
            ((0., 8.), (5., 8.)),
            ((5., 8.), (10., 8.)),
            ((0., 0.), (0., 8.)),
            ((10., 0.), (10., 8.)),
            ((5., 0.), (5., 8.)),
            ((0., 4.), (5., 4.)),
        ] {
            grid.push(Vector2::new(a.0, a.1), Vector2::new(b.0, b.1));
        }
        grid.horizontal.sort_by(|a, b| a.start.total_cmp(&b.start));
        grid.vertical.sort_by(|a, b| a.start.total_cmp(&b.start));
        assert_eq!(
            grid.cell(Rect([1., 5., 3., 7.])).unwrap().0,
            [0., 4., 5., 8.]
        );
        assert_eq!(
            grid.cell(Rect([6., 3., 9., 6.])).unwrap().0,
            [5., 0., 10., 8.]
        );
        assert!(grid.cell(Rect([1., 3., 3., 6.])).is_none());
        grid.horizontal
            .iter_mut()
            .find(|edge| edge.axis == 8. && edge.start == 5.)
            .unwrap()
            .start = 6.;
        assert!(grid.cell(Rect([6., 3., 9., 6.])).is_none());
    }
}

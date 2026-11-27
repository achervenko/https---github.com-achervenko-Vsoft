use acadrust::{
    entities::{
        hatch::{BoundaryEdge, BoundaryPath, CircularArcEdge, EllipticArcEdge, LineEdge},
        Entity, EntityType, Hatch, Line,
    },
    objects::ObjectType,
    types::Vector2,
    CadDocument, DxfReader, DxfWriter,
};
use std::{fs, process::Command};

#[test]
fn short_hatch_arcs_near_sheet_edge_stay_on_their_sheet() {
    let root = std::env::temp_dir().join(format!("vsoft-hatch-assignment-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let input = root.join("native.dxf");
    let payload = root.join("pages.json");
    let output = root.join("drawing.dxf");

    // Sheet 1 spans 0–10 inches, with an import boundary at 11 inches.
    // The arc bows left, entirely inside sheet 1; the whole circle reaches 11.4.
    let start = 150_f64.to_radians();
    let end = 210_f64.to_radians();
    let point = |angle: f64| Vector2::new(10.4 + angle.cos(), 3.0 + angle.sin());
    let mut path = BoundaryPath::new();
    path.edges.push(BoundaryEdge::CircularArc(CircularArcEdge {
        center: Vector2::new(10.4, 3.0),
        radius: 1.0,
        start_angle: start,
        end_angle: end,
        counter_clockwise: true,
    }));
    path.edges.push(BoundaryEdge::Line(LineEdge {
        start: point(end),
        end: point(start),
    }));
    let mut hatch = Hatch::solid();
    hatch.add_path(path);
    assert!(hatch.bounding_box().max.x > 11.0);

    let mut document = CadDocument::new();
    document
        .add_entity(EntityType::Hatch(hatch.into()))
        .unwrap();

    // A rotated ellipse with major axis (1, 0.6) and minor axis (-0.27, 0.45).
    // Its short left-hand arc and closing chord stay left of X=10.
    let ellipse_point = |t: f64| {
        Vector2::new(
            10.4 + t.cos() - 0.27 * t.sin(),
            5.0 + 0.6 * t.cos() + 0.45 * t.sin(),
        )
    };
    for (start, end, counter_clockwise) in [(start, end, true), (end, start, false)] {
        let mut path = BoundaryPath::new();
        path.edges.push(BoundaryEdge::EllipticArc(EllipticArcEdge {
            center: Vector2::new(10.4, 5.0),
            major_axis_endpoint: Vector2::new(1.0, 0.6),
            minor_axis_ratio: 0.45,
            start_angle: start,
            end_angle: end,
            counter_clockwise,
        }));
        path.edges.push(BoundaryEdge::Line(LineEdge {
            start: ellipse_point(end),
            end: ellipse_point(start),
        }));
        let mut hatch = Hatch::solid();
        hatch.add_path(path);
        assert!(hatch.bounding_box().max.x > 11.0);
        document
            .add_entity(EntityType::Hatch(hatch.into()))
            .unwrap();
    }
    document
        .add_entity(EntityType::Line(Line::from_coords(
            13.0, 1.0, 0.0, 14.0, 1.0, 0.0,
        )))
        .unwrap();
    DxfWriter::new(&document).write_to_file(&input).unwrap();
    fs::write(
        &payload,
        r#"[{"number":85,"size_pt":[720,720],"spans":[]},{"number":2,"size_pt":[720,720],"spans":[]}]"#,
    )
    .unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_vsoft"))
        .arg("_build-dxf")
        .args([&input, &payload, &output])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result_dxf = DxfReader::from_file(&output).unwrap().read().unwrap();
    let members = |name| {
        let handle = result_dxf
            .objects
            .values()
            .find_map(|object| match object {
                ObjectType::Dictionary(dict) => dict
                    .entries
                    .iter()
                    .find_map(|(key, handle)| (key == name).then_some(handle)),
                _ => None,
            })
            .unwrap();
        let ObjectType::Group(group) = &result_dxf.objects[handle] else {
            panic!("expected group {name}");
        };
        group
            .entities
            .iter()
            .map(|id| {
                result_dxf
                    .get_entity(*id)
                    .unwrap()
                    .as_entity()
                    .entity_type()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(members("SHEET_085"), ["HATCH", "HATCH", "HATCH"]);
    assert_eq!(members("SHEET_002"), ["LINE"]);
    for file in [input, payload, output] {
        fs::remove_file(file).unwrap();
    }
    fs::remove_dir(root).unwrap();
}

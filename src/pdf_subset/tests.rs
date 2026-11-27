use super::*;
use lopdf::{dictionary, Stream};

#[test]
fn filters_acroform_and_rejects_other_references_to_excluded_pages() {
    let directory = std::env::temp_dir().join(format!("vsoft-pdf-form-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let input = directory.join("input.pdf");
    let output = directory.join("output.pdf");
    let rejected = directory.join("rejected.pdf");
    let mut doc = Document::with_version("1.6");
    let root = doc.new_object_id();
    let first = doc.new_object_id();
    let second = doc.new_object_id();
    let field = doc.new_object_id();
    let keep = doc.add_object(dictionary! {
        "Subtype" => "Widget", "Parent" => field, "P" => first,
    });
    let drop = doc.add_object(dictionary! {
        "Subtype" => "Widget", "Parent" => field,
    });
    let drop_field = doc.add_object(dictionary! {
        "Subtype" => "Widget", "FT" => "Tx", "P" => second,
    });
    doc.objects.insert(
        field,
        dictionary! {
            "FT" => "Tx", "Kids" => vec![Object::Reference(keep), Object::Reference(drop)],
        }
        .into(),
    );
    let stream = doc.add_object(Stream::new(
        dictionary! {},
        b"BT (excluded form page) Tj ET".to_vec(),
    ));
    doc.objects.insert(
        first,
        dictionary! {
            "Type" => "Page", "Parent" => root, "Annots" => vec![Object::Reference(keep)],
        }
        .into(),
    );
    doc.objects.insert(
        second,
        dictionary! {
            "Type" => "Page", "Parent" => root, "Contents" => stream,
            "Annots" => vec![Object::Reference(drop), Object::Reference(drop_field)],
        }
        .into(),
    );
    doc.objects.insert(
        root,
        dictionary! {
            "Type" => "Pages", "Kids" => vec![Object::Reference(first), Object::Reference(second)],
            "Count" => 2, "MediaBox" => vec![0.into(), 0.into(), 300.into(), 400.into()],
        }
        .into(),
    );
    let form = doc.add_object(dictionary! {
        "Fields" => vec![Object::Reference(field), Object::Reference(drop_field)],
        "CO" => vec![Object::Reference(drop_field), Object::Reference(field)],
    });
    let catalog =
        doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => root, "AcroForm" => form });
    doc.trailer.set("Root", catalog);
    doc.save(&input).unwrap();
    prepare(&input, &output, Some("1")).unwrap();

    let subset = Document::load(&output).unwrap();
    assert_eq!(subset.page_iter().collect::<Vec<_>>(), vec![first]);
    for id in [second, stream, drop, drop_field] {
        assert!(!subset.objects.contains_key(&id));
    }
    let form = subset
        .catalog()
        .unwrap()
        .get(b"AcroForm")
        .unwrap()
        .as_dict()
        .unwrap();
    for key in [b"Fields".as_slice(), b"CO"] {
        assert_eq!(
            form.get(key).unwrap().as_array().unwrap(),
            &[Object::Reference(field)]
        );
    }
    let kids = subset
        .get_dictionary(field)
        .unwrap()
        .get(b"Kids")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(kids, &[Object::Reference(keep)]);
    assert!(subset.objects.contains_key(&keep));

    doc.catalog_mut().unwrap().set("OtherPageReference", second);
    doc.save(&input).unwrap();
    let error = prepare(&input, &rejected, Some("1")).unwrap_err();
    assert!(error.to_string().contains("remains reachable"));
    assert!(!rejected.exists());
    std::fs::remove_file(input).unwrap();
    std::fs::remove_file(output).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn removes_links_and_content_of_excluded_pages() {
    let directory = std::env::temp_dir().join(format!("vsoft-pdf-links-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let input = directory.join("input.pdf");
    let output = directory.join("output.pdf");
    let marker = b"EXCLUDED_PAGE_UNIQUE_TEXT";
    let mut doc = Document::with_version("1.6");
    let root = doc.new_object_id();
    let first = doc.new_object_id();
    let excluded_stream = doc.add_object(Stream::new(
        dictionary! {},
        [b"BT /F1 12 Tf 20 20 Td (".as_slice(), marker, b") Tj ET"].concat(),
    ));
    let second = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => root, "Contents" => excluded_stream,
    });
    let target = |id| vec![Object::Reference(id), Object::Name(b"Fit".to_vec())];
    let to_excluded = doc.add_object(dictionary! { "Subtype" => "Link", "Dest" => target(second) });
    let action = doc.add_object(dictionary! { "S" => "GoTo", "D" => target(second) });
    let via_action = doc.add_object(dictionary! { "Subtype" => "Link", "A" => action });
    let to_retained = doc.add_object(dictionary! { "Subtype" => "Link", "Dest" => target(first) });
    let old_drop = doc.add_object(dictionary! {
        "Subtype" => "Link", "Dest" => Object::Name(b"old-drop".to_vec()),
    });
    let old_keep = doc.add_object(dictionary! {
        "Subtype" => "Link", "Dest" => Object::Name(b"old-keep".to_vec()),
    });
    let named_action = doc.add_object(dictionary! {
        "S" => "GoTo", "D" => Object::string_literal("drop"),
    });
    let named_drop = doc.add_object(dictionary! { "Subtype" => "Link", "A" => named_action });
    let named_keep = doc.add_object(dictionary! {
        "Subtype" => "Link", "Dest" => Object::string_literal("keep"),
    });
    let annotations = doc.add_object(vec![
        to_excluded.into(),
        via_action.into(),
        to_retained.into(),
        old_drop.into(),
        old_keep.into(),
        named_drop.into(),
        named_keep.into(),
    ]);
    doc.objects.insert(
        first,
        dictionary! {
            "Type" => "Page", "Parent" => root, "Annots" => annotations,
        }
        .into(),
    );
    doc.objects.insert(root, dictionary! {
        "Type" => "Pages", "Kids" => vec![Object::Reference(first), Object::Reference(second)], "Count" => 2,
        "MediaBox" => vec![0.into(), 0.into(), 300.into(), 400.into()],
    }.into());
    let drop_destination = doc.add_object(dictionary! { "D" => target(second) });
    let leaf = doc.add_object(dictionary! {
        "Names" => vec![
            Object::string_literal("drop"), Object::Reference(drop_destination),
            Object::string_literal("keep"), Object::Array(target(first)),
        ],
        "Limits" => vec![Object::string_literal("drop"), Object::string_literal("keep")],
    });
    let tree = doc.add_object(dictionary! { "Kids" => vec![Object::Reference(leaf)] });
    let catalog = doc.add_object(dictionary! {
        "Type" => "Catalog", "Pages" => root,
        "Names" => dictionary! { "Dests" => tree },
        "Dests" => dictionary! { "old-drop" => target(second), "old-keep" => target(first) },
    });
    doc.trailer.set("Root", catalog);
    doc.save(&input).unwrap();

    prepare(&input, &output, Some("1")).unwrap();
    let subset = Document::load(&output).unwrap();
    assert_eq!(subset.page_iter().collect::<Vec<_>>(), vec![first]);
    assert!(!subset.objects.contains_key(&second));
    assert!(!subset.objects.contains_key(&excluded_stream));
    assert!(!subset
        .objects
        .values()
        .filter_map(|object| object.as_stream().ok())
        .any(|stream| {
            let content = stream
                .decompressed_content()
                .unwrap_or_else(|_| stream.content.clone());
            content.windows(marker.len()).any(|window| window == marker)
        }));
    let links = subset
        .get_dictionary(first)
        .unwrap()
        .get(b"Annots")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(
        links,
        &[
            Object::Reference(to_retained),
            Object::Reference(old_keep),
            Object::Reference(named_keep),
        ]
    );
    let catalog = subset.catalog().unwrap();
    let legacy = catalog.get(b"Dests").unwrap().as_dict().unwrap();
    assert!(!legacy.has(b"old-drop"));
    assert_eq!(
        legacy.get(b"old-keep").unwrap().as_array().unwrap(),
        &target(first)
    );
    let names = catalog.get(b"Names").unwrap().as_dict().unwrap();
    let tree = names.get(b"Dests").unwrap().as_dict().unwrap();
    assert_eq!(
        tree.get(b"Names").unwrap().as_array().unwrap(),
        &[Object::string_literal("keep"), Object::Array(target(first)),]
    );
    std::fs::remove_file(input).unwrap();
    std::fs::remove_file(output).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn reorders_pages_and_preserves_inherited_boxes_resources_and_rotation() {
    let directory = std::env::temp_dir().join(format!("vsoft-pdf-subset-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let input = directory.join("input.pdf");
    let output = directory.join("output.pdf");
    let mut doc = Document::with_version("1.6");
    let root = doc.new_object_id();
    let branch = doc.new_object_id();
    let first = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => root, "MediaBox" => vec![0.into(), 0.into(), 300.into(), 400.into()],
    });
    let second = doc.add_object(dictionary! { "Type" => "Page", "Parent" => branch });
    doc.objects.insert(branch, dictionary! {
        "Type" => "Pages", "Parent" => root, "Kids" => vec![Object::Reference(second)], "Count" => 1,
        "MediaBox" => vec![0.into(), 0.into(), 1000.into(), 600.into()],
        "CropBox" => vec![10.into(), 20.into(), 990.into(), 580.into()],
        "Rotate" => -270, "Resources" => dictionary! { "ProcSet" => vec![Object::Name(b"PDF".to_vec())] },
    }.into());
    doc.objects.insert(root, dictionary! {
        "Type" => "Pages", "Kids" => vec![Object::Reference(first), Object::Reference(branch)], "Count" => 2,
    }.into());
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => root });
    doc.trailer.set("Root", catalog);
    doc.save(&input).unwrap();

    assert!(prepare(&input, &output, Some("2,2")).is_err());
    assert!(!output.exists());
    let result = prepare(&input, &output, Some("2,1")).unwrap();
    assert_eq!(result.total, 2);
    assert_eq!(result.pages[0].number, 2);
    assert_eq!(result.pages[0].size_pt, [560., 980.]);
    assert_eq!(result.pages[0].rotation, 90);
    assert_eq!(result.pages[1].number, 1);
    assert_eq!(result.pages[1].size_pt, [300., 400.]);
    let output_doc = Document::load(&output).unwrap();
    let mut ordered = output_doc.page_iter();
    let page = output_doc.get_dictionary(ordered.next().unwrap()).unwrap();
    assert!(page.has(b"Resources"));
    assert!(page.has(b"CropBox"));
    assert_eq!(page.get(b"Rotate").unwrap().as_i64().unwrap(), -270);
    assert_eq!(ordered.count(), 1);
    assert!(prepare(&input, &output, Some("1")).is_err());
    std::fs::remove_file(input).unwrap();
    std::fs::remove_file(output).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

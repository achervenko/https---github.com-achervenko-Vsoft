use crate::dashes;
mod geometry;
mod layouts;
mod sheet_layers;
mod stamp;
mod text;
use acadrust::{
    entities::EntityType,
    objects::{Group, ObjectType},
    tables::{Layer, TextStyle},
    types::Transform,
    DxfReader, DxfWriter,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::{error::Error, ffi::OsString, fs::File, io::BufReader, path::PathBuf};
use vsoft::universal_layout::{plan_sheets, SheetInput};

#[derive(Deserialize)]
struct Sheet {
    number: usize,
    size_pt: [f64; 2],
    spans: Vec<text::Span>,
    #[serde(default)]
    trial_mtext: bool,
    #[serde(default)]
    dash_strokes: Vec<dashes::Stroke>,
    #[serde(default)]
    dash_unsupported: usize,
    #[serde(default)]
    dash_beziers: Vec<dashes::BezierStroke>,
}

#[derive(Serialize)]
struct SheetResult {
    sheet: usize,
    geometry: BTreeMap<&'static str, usize>,
    dash_segments_restored: usize,
    dash_curves_restored: usize,
    dash_unmatched: usize,
    multiline: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    mtext_trial: Option<text::TrialReport>,
}

pub fn run(args: impl Iterator<Item = OsString>) -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = args.map(PathBuf::from).collect();
    let [input, payload, output] = args.as_slice() else {
        return Err("expected native DXF, sheet JSON and output DXF".into());
    };
    let pages: Vec<Sheet> = serde_json::from_reader(BufReader::new(File::open(payload)?))?;
    let plan = plan_sheets(
        &pages
            .iter()
            .map(|s| SheetInput {
                number: s.number,
                size_pt: s.size_pt,
            })
            .collect::<Vec<_>>(),
    )?;
    let mut doc = DxfReader::from_file(input)?.read()?;
    doc.header.insertion_units = 4;
    doc.header.measurement = 1;
    let model_owner = doc
        .block_records
        .get("*Model_Space")
        .ok_or("missing model space")?
        .handle;
    let remove = doc
        .entities()
        .filter(|e| {
            e.common().owner_handle == model_owner
                && matches!(
                    e,
                    EntityType::Text(_) | EntityType::MText(_) | EntityType::RasterImage(_)
                )
        })
        .map(|e| e.common().handle)
        .collect::<Vec<_>>();
    for handle in remove {
        doc.remove_entity(handle);
    }
    let mut groups = vec![Vec::new(); plan.len()];
    let mut reports = Vec::with_capacity(plan.len());
    let transforms: Vec<_> = plan
        .iter()
        .map(|s| {
            Transform::from_matrix(acadrust::types::Matrix4::scaling(25.4, 25.4, 25.4)).then(
                &Transform::from_matrix(acadrust::types::Matrix4::translation(
                    s.model_offset_mm - s.import_offset_in * 25.4,
                    0.0,
                    0.0,
                )),
            )
        })
        .collect();
    for entity in doc.entities_mut().filter(|e| {
        e.common().owner_handle == model_owner
            && !matches!(
                e,
                EntityType::Block(_) | EntityType::BlockEnd(_) | EntityType::Viewport(_)
            )
    }) {
        let (min_x, max_x) = geometry::sheet_x_bounds(entity);
        let center = (min_x + max_x) / 2.0;
        let index = plan.partition_point(|sheet| sheet.import_right_boundary_in <= center);
        let sheet = plan.get(index).ok_or("entity outside sheet inventory")?;
        if !center.is_finite()
            || min_x < sheet.import_offset_in - 1.0
            || max_x > sheet.import_right_boundary_in
        {
            return Err(format!("entity crosses sheet {} boundary", sheet.number).into());
        }
        groups[index].push(entity.common().handle);
        entity.apply_transform(&transforms[index]);
    }

    let mut style = TextStyle::new("VSOFT_PDF");
    style.font_file = "arial.ttf".to_string();
    style.handle = doc.allocate_handle();
    doc.text_styles.add(style)?;
    if doc.layers.get("PDF_TEXT").is_none() {
        let mut layer = Layer::new("PDF_TEXT");
        layer.handle = doc.allocate_handle();
        doc.layers.add(layer)?;
    }
    let default_layout = doc
        .objects
        .iter()
        .find_map(|(h, o)| match o {
            ObjectType::Layout(l) if l.name == "Layout1" => Some(*h),
            _ => None,
        })
        .ok_or("missing Layout1")?;
    let find_dictionary = |name: &str| {
        doc.objects.values().find_map(|object| match object {
            ObjectType::Dictionary(dict) => dict
                .entries
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, handle)| *handle),
            _ => None,
        })
    };
    let layout_dictionary = find_dictionary("ACAD_LAYOUT").ok_or("missing layout dictionary")?;
    let group_dictionary =
        find_dictionary("ACAD_GROUP").ok_or("missing group dictionary registration")?;
    doc.header.acad_layout_dict_handle = layout_dictionary;
    doc.header.acad_group_dict_handle = group_dictionary;
    for (index, ((page, sheet), group_members)) in
        pages.iter().zip(&plan).zip(&mut groups).enumerate()
    {
        let restored = dashes::restore(&mut doc, sheet, group_members, &page.dash_strokes)?;
        let curves = dashes::restore_beziers(&mut doc, sheet, group_members, &page.dash_beziers)?;
        sheet_layers::assign_frame(&mut doc, sheet, group_members)?;
        let stamp = stamp::assign(&mut doc, sheet, group_members, &page.spans)?;
        let mut geometry = BTreeMap::new();
        for handle in group_members.iter() {
            let entity = doc.get_entity(*handle).ok_or("missing group member")?;
            *geometry
                .entry(entity.as_entity().entity_type())
                .or_insert(0) += 1;
        }
        let mtext_trial = text::add(
            &mut doc,
            sheet,
            group_members,
            &page.spans,
            stamp,
            page.trial_mtext,
        )?;
        reports.push(SheetResult {
            sheet: sheet.number,
            geometry,
            dash_segments_restored: restored,
            dash_curves_restored: curves,
            dash_unmatched: page.dash_unsupported.saturating_sub(curves) + page.dash_strokes.len()
                - restored,
            multiline: mtext_trial.merged_groups,
            mtext_trial: page.trial_mtext.then_some(mtext_trial),
        });
        let group_name = format!("SHEET_{:03}", sheet.number);
        let group_dict = doc.header.acad_group_dict_handle;
        let group_handle = doc.allocate_handle();
        let mut group = Group::new(&group_name);
        group.handle = group_handle;
        group.owner = group_dict;
        group.entities = std::mem::take(group_members);
        let Some(ObjectType::Dictionary(dict)) = doc.objects.get_mut(&group_dict) else {
            return Err("missing group dictionary".into());
        };
        dict.add_entry(group_name, group_handle);
        doc.objects.insert(group_handle, ObjectType::Group(group));

        layouts::add_sheet_layout(&mut doc, sheet, index, default_layout, layout_dictionary)?;
    }
    DxfWriter::new(&doc).write_to_file(output)?;
    serde_json::to_writer(std::io::stdout().lock(), &reports)?;
    Ok(())
}

use acadrust::{
    entities::{EntityType, Viewport},
    objects::ObjectType,
    types::{Handle, Vector3},
    CadDocument,
};
use std::error::Error;
use vsoft::universal_layout::SheetLayout;

pub(super) fn add_sheet_layout(
    doc: &mut CadDocument,
    sheet: &SheetLayout,
    index: usize,
    default_layout: Handle,
    layout_dictionary: Handle,
) -> Result<(), Box<dyn Error>> {
    let [width, height] = sheet.size_mm;
    let [cx, cy] = sheet.paper_center_mm;
    let [vx, vy] = sheet.view_center_mm;
    let name = format!("{:03}", sheet.number);
    let layout_handle = if index == 0 {
        let Some(ObjectType::Dictionary(dict)) = doc.objects.get_mut(&layout_dictionary) else {
            return Err("missing layout dictionary".into());
        };
        dict.entries.retain(|(key, _)| key != "Layout1");
        dict.add_entry(&name, default_layout);
        default_layout
    } else {
        doc.add_layout(&name)?
    };
    let Some(ObjectType::Layout(layout)) = doc.objects.get_mut(&layout_handle) else {
        return Err("missing newly created layout".into());
    };
    layout.name = name.clone();
    layout.tab_order = i16::try_from(index + 1)?;
    layout.min_limits = (0.0, 0.0);
    layout.max_limits = (width, height);
    layout.paper_width = width;
    layout.paper_height = height;
    layout.paper_size = sheet.plot_media.clone().unwrap_or_default();
    layout.plot_printer_name = "DWG To PDF.pc3".into();
    layout.raw_plot_settings_codes = Some(vec![
        (1, "".into()),
        (2, layout.plot_printer_name.clone()),
        (4, layout.paper_size.clone()),
        (40, "0".into()),
        (41, "0".into()),
        (42, "0".into()),
        (43, "0".into()),
        (44, width.to_string()),
        (45, height.to_string()),
        (46, "0".into()),
        (47, "0".into()),
        (142, "1".into()),
        (143, "1".into()),
        (70, "672".into()),
        (72, "1".into()),
        (73, "0".into()),
        (74, "5".into()),
        (75, "16".into()),
        (147, "1".into()),
    ]);
    let overall_handle = layout.viewport;
    if let Some(EntityType::Viewport(overall)) = doc.get_entity_mut(overall_handle) {
        overall.center = Vector3::new(cx, cy, 0.0);
        overall.view_center = Vector3::new(cx, cy, 0.0);
        overall.width = width * 1.1;
        overall.height = height * 1.1;
        overall.view_height = height * 1.1;
    }
    let mut viewport = Viewport::with_size(Vector3::new(cx, cy, 0.0), width, height);
    viewport.common.layer = "Defpoints".into();
    viewport.id = 2;
    viewport.view_center = Vector3::new(vx, vy, 0.0);
    viewport.view_height = height;
    viewport.status.locked = true;
    doc.add_entity_to_layout(EntityType::Viewport(Box::new(viewport)), &name)?;
    Ok(())
}

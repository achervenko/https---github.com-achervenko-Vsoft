//! Experimental PDF page selection. CAD import still uses the established pipeline.
use lopdf::{Dictionary, Document, Object, ObjectId};
use serde::Serialize;
use std::collections::HashSet;
use std::error::Error;
use std::fs::OpenOptions;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct Page {
    pub number: usize,
    pub size_pt: [f64; 2],
    pub rotation: i64,
}

#[derive(Debug, Serialize)]
pub struct Subset {
    pub total: usize,
    pub pages: Vec<Page>,
}

const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

fn inherited<'a>(
    doc: &'a Document,
    mut id: ObjectId,
    key: &[u8],
) -> Result<Option<&'a Object>, Box<dyn Error>> {
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(id) {
            return Err("cycle in PDF page parents".into());
        }
        let node = doc.get_dictionary(id)?;
        if let Ok(value) = node.get(key) {
            return Ok(Some(doc.dereference(value)?.1));
        }
        match node.get(b"Parent") {
            Ok(parent) => id = parent.as_reference()?,
            Err(_) => return Ok(None),
        }
    }
}

fn number(value: &Object) -> lopdf::Result<f64> {
    match value {
        Object::Integer(value) => Ok(*value as f64),
        _ => value.as_f32().map(f64::from),
    }
}

fn rectangle(doc: &Document, value: &Object) -> Result<[f64; 4], Box<dyn Error>> {
    let mut coords = [0.0; 4];
    let values = value.as_array()?;
    if values.len() != 4 {
        return Err("invalid PDF page box".into());
    }
    for (coord, value) in coords.iter_mut().zip(values) {
        *coord = number(doc.dereference(value)?.1)?;
    }
    if !coords.iter().all(|v| v.is_finite()) {
        return Err("non-finite PDF page box".into());
    }
    Ok([
        coords[0].min(coords[2]),
        coords[1].min(coords[3]),
        coords[0].max(coords[2]),
        coords[1].max(coords[3]),
    ])
}

fn targets_excluded_page(
    doc: &Document,
    destination: &Object,
    excluded: &HashSet<ObjectId>,
) -> lopdf::Result<bool> {
    let destination = doc.dereference(destination)?.1;
    let destination = match destination {
        Object::Dictionary(entry) => doc.dereference(entry.get(b"D")?)?.1,
        value => value,
    };
    Ok(matches!(destination, Object::Array(items)
        if matches!(items.first(), Some(Object::Reference(id)) if excluded.contains(id))))
}

#[derive(Default)]
struct RemovedDestinations {
    names: HashSet<Vec<u8>>,
    strings: HashSet<Vec<u8>>,
}

fn collect_destinations(
    doc: &Document,
    node: &Object,
    excluded: &HashSet<ObjectId>,
    visited: &mut HashSet<ObjectId>,
    retained: &mut Vec<Object>,
    removed: &mut HashSet<Vec<u8>>,
) -> Result<(), Box<dyn Error>> {
    let (id, value) = doc.dereference(node)?;
    if id.is_some_and(|id| !visited.insert(id)) {
        return Err("repeated node in PDF destination name tree".into());
    }
    let node = value.as_dict()?;
    if let Ok(entries) = node.get(b"Names") {
        let entries = doc.dereference(entries)?.1.as_array()?;
        let pairs = entries.chunks_exact(2);
        if !pairs.remainder().is_empty() {
            return Err("unpaired entry in PDF destination name tree".into());
        }
        for pair in pairs {
            if targets_excluded_page(doc, &pair[1], excluded)? {
                removed.insert(doc.dereference(&pair[0])?.1.as_str()?.to_vec());
            } else {
                retained.extend_from_slice(pair);
            }
        }
    }
    if let Ok(children) = node.get(b"Kids") {
        for child in doc.dereference(children)?.1.as_array()? {
            collect_destinations(doc, child, excluded, visited, retained, removed)?;
        }
    }
    Ok(())
}

fn filter_named_destinations(
    doc: &mut Document,
    excluded: &HashSet<ObjectId>,
) -> Result<RemovedDestinations, Box<dyn Error>> {
    let mut removed = RemovedDestinations::default();
    let mut catalog = doc.catalog()?.clone();
    if let Ok(value) = catalog.get(b"Dests") {
        let mut destinations = doc.dereference(value)?.1.as_dict()?.clone();
        for (name, destination) in destinations.iter() {
            if targets_excluded_page(doc, destination, excluded)? {
                removed.names.insert(name.clone());
            }
        }
        for name in &removed.names {
            destinations.remove(name);
        }
        if !removed.names.is_empty() {
            catalog.set("Dests", destinations);
        }
    }
    if let Ok(value) = catalog.get(b"Names") {
        let mut names = doc.dereference(value)?.1.as_dict()?.clone();
        if let Ok(tree) = names.get(b"Dests") {
            let mut retained = Vec::new();
            collect_destinations(
                doc,
                tree,
                excluded,
                &mut HashSet::new(),
                &mut retained,
                &mut removed.strings,
            )?;
            if !removed.strings.is_empty() {
                if retained.is_empty() {
                    names.remove(b"Dests");
                } else {
                    let mut tree = Dictionary::new();
                    tree.set("Names", retained);
                    names.set("Dests", tree);
                }
                catalog.set("Names", names);
            }
        }
    }
    *doc.catalog_mut()? = catalog;
    Ok(removed)
}

fn remove_excluded_links(
    doc: &Document,
    page: &mut Dictionary,
    excluded: &HashSet<ObjectId>,
    removed: &RemovedDestinations,
) -> lopdf::Result<()> {
    let Ok(annotations) = page.get(b"Annots") else {
        return Ok(());
    };
    let annotations = doc.dereference(annotations)?.1.as_array()?;
    let mut retained = Vec::with_capacity(annotations.len());
    for annotation in annotations {
        let entry = doc.dereference(annotation)?.1;
        if let Ok(link) = entry.as_dict() {
            if matches!(link.get(b"Subtype").and_then(Object::as_name), Ok(b"Link")) {
                let action = link
                    .get(b"A")
                    .ok()
                    .map(|value| doc.dereference(value))
                    .transpose()?
                    .and_then(|(_, value)| value.as_dict().ok());
                let action_destination = action
                    .filter(|action| {
                        matches!(action.get(b"S").and_then(Object::as_name), Ok(b"GoTo"))
                    })
                    .and_then(|action| action.get(b"D").ok());
                let mut excluded_target = false;
                for destination in link.get(b"Dest").ok().into_iter().chain(action_destination) {
                    let value = doc.dereference(destination)?.1;
                    excluded_target |= match value {
                        Object::Name(name) => removed.names.contains(name),
                        Object::String(name, _) => removed.strings.contains(name),
                        _ => targets_excluded_page(doc, value, excluded)?,
                    };
                }
                if excluded_target {
                    continue;
                }
            }
        }
        retained.push(annotation.clone());
    }
    if retained.len() != annotations.len() {
        if retained.is_empty() {
            page.remove(b"Annots");
        } else {
            page.set("Annots", retained);
        }
    }
    Ok(())
}

fn filter_form_fields(
    doc: &mut Document,
    fields: &Object,
    excluded: &HashSet<ObjectId>,
    excluded_annotations: &HashSet<ObjectId>,
    removed: &mut HashSet<ObjectId>,
    visited: &mut HashSet<ObjectId>,
) -> Result<Vec<Object>, Box<dyn Error>> {
    let fields = doc.dereference(fields)?.1.as_array()?.clone();
    let mut retained = Vec::with_capacity(fields.len());
    for field in fields {
        let (id, value) = doc.dereference(&field)?;
        if id.is_some_and(|id| !visited.insert(id)) {
            return Err("repeated field in PDF AcroForm tree".into());
        }
        let mut entry = value.as_dict()?.clone();
        let excluded_page = entry
            .get(b"P")
            .and_then(Object::as_reference)
            .is_ok_and(|page| excluded.contains(&page));
        let excluded_annotation = id.is_some_and(|id| excluded_annotations.contains(&id));
        let mut discard = excluded_page || excluded_annotation;
        if let Ok(kids) = entry.get(b"Kids") {
            let original_count = doc.dereference(kids)?.1.as_array()?.len();
            let children =
                filter_form_fields(doc, kids, excluded, excluded_annotations, removed, visited)?;
            if children.len() != original_count {
                discard |= children.is_empty();
                entry.set("Kids", children);
            }
        }
        if discard {
            removed.extend(id);
        } else if let Some(id) = id {
            doc.objects.insert(id, Object::Dictionary(entry));
            retained.push(field);
        } else {
            retained.push(Object::Dictionary(entry));
        }
    }
    Ok(retained)
}

fn filter_acroform(doc: &mut Document, excluded: &HashSet<ObjectId>) -> Result<(), Box<dyn Error>> {
    let Ok(form) = doc.catalog()?.get(b"AcroForm") else {
        return Ok(());
    };
    let mut form = doc.dereference(form)?.1.as_dict()?.clone();
    let mut excluded_annotations = HashSet::new();
    for page in excluded {
        if let Ok(annotations) = doc.get_dictionary(*page)?.get(b"Annots") {
            for annotation in doc.dereference(annotations)?.1.as_array()? {
                excluded_annotations.extend(doc.dereference(annotation)?.0);
            }
        }
    }
    let mut removed = HashSet::new();
    let fields = filter_form_fields(
        doc,
        form.get(b"Fields")?,
        excluded,
        &excluded_annotations,
        &mut removed,
        &mut HashSet::new(),
    )?;
    if fields.is_empty() {
        doc.catalog_mut()?.remove(b"AcroForm");
        return Ok(());
    }
    form.set("Fields", fields);
    if let Ok(order) = form.get(b"CO") {
        let mut order = doc.dereference(order)?.1.as_array()?.clone();
        order.retain(|field| !field.as_reference().is_ok_and(|id| removed.contains(&id)));
        form.set("CO", order);
    }
    doc.catalog_mut()?.set("AcroForm", form);
    Ok(())
}

pub fn prepare(
    input: &Path,
    output: &Path,
    selection: Option<&str>,
) -> Result<Subset, Box<dyn Error>> {
    let mut doc = Document::load(input)?;
    if doc.is_encrypted() {
        return Err("encrypted PDFs are unsupported by the Rust preparation command".into());
    }
    let ids: Vec<_> = doc.page_iter().collect();
    let selected = crate::input_pages::select_pages(selection, ids.len(), 4300)
        .map_err(|error| error.to_string())?;
    let selected_numbers: HashSet<_> = selected.iter().copied().collect();
    let excluded = ids
        .iter()
        .enumerate()
        .filter_map(|(index, id)| (!selected_numbers.contains(&(index + 1))).then_some(*id))
        .collect::<HashSet<_>>();
    let root = doc.catalog()?.get(b"Pages")?.as_reference()?;
    let removed = if excluded.is_empty() {
        RemovedDestinations::default()
    } else {
        filter_acroform(&mut doc, &excluded)?;
        filter_named_destinations(&mut doc, &excluded)?
    };
    let mut detached = Vec::with_capacity(selected.len());
    let mut pages = Vec::with_capacity(selected.len());
    for number_in_source in selected {
        let id = ids[number_in_source - 1];
        let mut page = doc.get_dictionary(id)?.clone();
        let media = rectangle(
            &doc,
            inherited(&doc, id, b"MediaBox")?.ok_or("missing PDF MediaBox")?,
        )?;
        let crop = inherited(&doc, id, b"CropBox")?
            .map(|value| rectangle(&doc, value))
            .transpose()?
            .unwrap_or(media);
        let rotation = inherited(&doc, id, b"Rotate")?
            .map(Object::as_i64)
            .transpose()?
            .unwrap_or(0)
            .rem_euclid(360);
        if rotation % 90 != 0 {
            return Err(format!("page {number_in_source}: invalid rotation {rotation}").into());
        }
        let unit = page
            .get(b"UserUnit")
            .ok()
            .map(|value| doc.dereference(value).and_then(|(_, value)| number(value)))
            .transpose()?
            .unwrap_or(1.0);
        let mut size_pt = [
            (media[2].min(crop[2]) - media[0].max(crop[0])) * unit,
            (media[3].min(crop[3]) - media[1].max(crop[1])) * unit,
        ];
        if !size_pt.iter().all(|v| v.is_finite() && *v > 0.0) || unit <= 0.0 {
            return Err(format!("page {number_in_source}: invalid visible size").into());
        }
        if rotation == 90 || rotation == 270 {
            size_pt.swap(0, 1);
        }
        for key in INHERITED {
            if !page.has(key) {
                if let Some(value) = inherited(&doc, id, key)? {
                    page.set(key, value.clone());
                }
            }
        }
        page.set("Parent", root);
        if !excluded.is_empty() {
            remove_excluded_links(&doc, &mut page, &excluded, &removed)?;
        }
        detached.push((id, page));
        pages.push(Page {
            number: number_in_source,
            size_pt,
            rotation,
        });
    }
    let kids = detached
        .iter()
        .map(|(id, _)| Object::Reference(*id))
        .collect::<Vec<_>>();
    for (id, page) in detached {
        doc.objects.insert(id, Object::Dictionary(page));
    }
    let tree = doc.get_object_mut(root)?.as_dict_mut()?;
    tree.set("Kids", kids);
    tree.set("Count", pages.len() as i64);
    // These catalog entries refer to the source document's page order.
    let catalog = doc.catalog_mut()?;
    for key in [b"PageLabels".as_slice(), b"Outlines", b"OpenAction"] {
        catalog.remove(key);
    }
    doc.prune_objects();
    if let Some(id) = ids
        .iter()
        .find(|id| excluded.contains(id) && doc.objects.contains_key(id))
    {
        return Err(format!("excluded PDF page {} {} remains reachable", id.0, id.1).into());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    if let Err(error) = doc.save_to(&mut file) {
        drop(file);
        let _ = std::fs::remove_file(output);
        return Err(error.into());
    }
    Ok(Subset {
        total: ids.len(),
        pages,
    })
}

#[cfg(test)]
mod tests;

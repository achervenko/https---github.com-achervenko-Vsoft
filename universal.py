"""Assemble imported PDF geometry in paper millimetres."""
import json
import os
from bisect import bisect_right
from collections import Counter
from io import BytesIO
from math import atan2, degrees, isclose
from pathlib import Path

import ezdxf
import pymupdf
from ezdxf import bbox
from ezdxf.fonts.ttfonts import TTFontRenderer
from ezdxf.math import Matrix44
from fontTools.ttLib import TTFont

PT_TO_MM = 25.4 / 72
ISO_SIZES = ((841, 1189), (594, 841), (420, 594), (297, 420), (210, 297))
PLOT_TOLERANCE_PT = 2


def media(size):
    for index, dimensions in enumerate(ISO_SIZES):
        for width, height in (dimensions, dimensions[::-1]):
            if all(abs(value - mm / PT_TO_MM) <= PLOT_TOLERANCE_PT
                   for value, mm in zip(size, (width, height))):
                return f'ISO_full_bleed_A{index}_({width:.2f}_x_{height:.2f}_MM)'
    return None


def import_offsets(sizes):
    offsets = []
    position = 0.0
    for width, _ in sizes:
        offsets.append(position)
        position += width / 72 + 2
    return offsets


def plan_layout(numbers, sizes):
    sheets = []
    shift = 0.0
    for number, size, offset in zip(numbers, sizes, import_offsets(sizes)):
        width, height = (value * PT_TO_MM for value in size)
        sheets.append({
            'number': number, 'size_mm': [width, height],
            'import_offset_in': offset, 'import_right_boundary_in': offset + size[0] / 72 + 1,
            'model_offset_mm': shift, 'paper_center_mm': [width / 2, height / 2],
            'view_center_mm': [shift + width / 2, height / 2], 'plot_media': media(size),
        })
        shift += width + 20
    return sheets


def matching_layout_value(expected, actual):
    if isinstance(expected, float):
        return type(actual) in (int, float) and isclose(expected, actual, rel_tol=0, abs_tol=1e-8)
    if isinstance(expected, list):
        return (isinstance(actual, list) and len(expected) == len(actual)
                and all(matching_layout_value(a, b) for a, b in zip(expected, actual)))
    return type(expected) is type(actual) and expected == actual


def text_font_path():
    windows = os.environ.get('WINDIR')
    if not windows:
        raise RuntimeError('WINDIR is not set. Cannot locate Arial for the universal profile.')
    font = Path(windows) / 'Fonts' / 'arial.ttf'
    if not font.is_file():
        raise RuntimeError(f'Arial is required for the universal profile: {font}')
    return font


def build(work, stage, pdf, sizes, plan, restore_dashes=None):
    doc = ezdxf.readfile(work / 'native.dxf')
    doc.units = 4
    doc.header['$MEASUREMENT'] = 1
    model = doc.modelspace()
    old_layouts = [name for name in doc.layouts.names() if name != 'Model']
    buckets = [[] for _ in plan]
    boundaries = [sheet['import_right_boundary_in'] for sheet in plan]
    for entity in list(model):
        if entity.dxftype() in ('TEXT', 'MTEXT', 'IMAGE'):
            model.delete_entity(entity)
            continue
        bounds = bbox.extents([entity])
        if not bounds.has_data:
            raise ValueError(f'Cannot locate imported {entity.dxftype()} geometry')
        index = bisect_right(boundaries, bounds.center.x)
        if index >= len(plan) or bounds.extmin.x < plan[index]['import_offset_in'] - 1 or bounds.extmax.x > boundaries[index]:
            raise ValueError(f'Imported {entity.dxftype()} crosses the sheet boundary')
        buckets[index].append(entity)

    font_path = text_font_path()
    try:
        text_font = TTFontRenderer(TTFont(BytesIO(font_path.read_bytes())))
    except Exception as error:
        raise RuntimeError(f'Cannot load Arial metrics from {font_path}: {error}') from error
    doc.styles.new('VSOFT_PDF', dxfattribs={'font': font_path.name})
    if 'PDF_TEXT' not in doc.layers:
        doc.layers.new('PDF_TEXT')
    report = []
    for index, (page, size, sheet) in enumerate(zip(pdf, sizes, plan)):
        number = sheet['number']
        width, height = sheet['size_mm']
        shift = sheet['model_offset_mm']
        transform = Matrix44.chain(
            Matrix44.translate(-sheet['import_offset_in'], 0, 0),
            Matrix44.scale(25.4),
            Matrix44.translate(shift, 0, 0),
        )
        entities = buckets[index]
        for entity in entities:
            entity.transform(transform)
        restored = unmatched = 0
        if restore_dashes:
            entities, restored, unmatched = restore_dashes(doc, page, sheet, entities)
        counts = Counter(entity.dxftype() for entity in entities)

        spans = 0
        replacements = 0
        for block in page.get_text('dict', flags=0)['blocks']:
            for line in block.get('lines', []):
                dx, dy = line['dir']
                angle = degrees(atan2(-dy, dx))
                for span in line['spans']:
                    value = span['text']
                    if not value.strip():
                        continue
                    x, y = span['origin']
                    cap_height = span['size'] * PT_TO_MM * .72
                    quad = pymupdf.recover_span_quad(line['dir'], span)
                    source_width = (quad.ur - quad.ul).norm() * PT_TO_MM
                    arial_width = text_font.get_text_length(value, cap_height)
                    width_factor = source_width / arial_width if source_width > 0 and arial_width > 0 else 1
                    entities.append(model.add_text(value, dxfattribs={
                        'insert': (shift + x * PT_TO_MM, height - y * PT_TO_MM),
                        'height': cap_height, 'width': width_factor,
                        'rotation': angle, 'style': 'VSOFT_PDF', 'layer': 'PDF_TEXT',
                    }))
                    spans += 1
                    replacements += value.count('\ufffd')

        doc.groups.new(f'SHEET_{number:03d}').set_data(entities)
        paper = doc.layouts.new(f'{number:03d}')
        paper.page_setup(size=(width, height), margins=(0, 0, 0, 0), units='mm', device='DWG To PDF.pc3')
        if name := sheet['plot_media']:
            paper.dxf_layout.dxf.paper_size = name
        paper.dxf_layout.dxf.taborder = index + 1
        viewport = paper.add_viewport(
            center=sheet['paper_center_mm'], size=(width, height),
            view_center_point=sheet['view_center_mm'], view_height=height,
        )
        viewport.dxf.flags |= 16384
        viewport.dxf.layer = 'Defpoints'
        for view in paper.query('VIEWPORT'):
            if view.dxf.id == 1:
                view.dxf.layer = '0'
        report.append({
            'sheet': number, 'size_pt': size, 'geometry': dict(counts), 'text_spans': spans,
            'multiline': 0, 'dash_segments_restored': restored, 'dash_unmatched': unmatched,
            'replacement_chars': replacements,
            'omitted_images': len(page.get_image_info()),
        })

    for name in old_layouts:
        doc.layouts.delete(name)
    width, height = sizes[0]
    doc.set_modelspace_vport(height=height * PT_TO_MM * 1.15,
                            center=(width * PT_TO_MM / 2, height * PT_TO_MM / 2))
    doc.header['$TILEMODE'] = 1
    if doc.audit().errors:
        raise ValueError('Universal DXF audit failed')
    stage.mkdir()
    doc.saveas(stage / 'drawing.dxf')
    (work / 'build_report.json').write_text(json.dumps(report), encoding='utf-8')

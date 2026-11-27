"""Pass PDF data to the universal Rust DXF builder."""
import json
from io import BytesIO
from math import atan2
import subprocess

import pymupdf
from ezdxf.fonts.ttfonts import TTFontRenderer
from fontTools.ttLib import TTFont

from universal import PT_TO_MM, text_font_path
from universal_dashes import short_beziers, source_patterns


def python_payload(pdf, sizes, plan, restore_dashes, trial_mtext):
    font_path = text_font_path()
    font = TTFontRenderer(TTFont(BytesIO(font_path.read_bytes())))
    pages = []
    report = []
    for page, size, sheet in zip(pdf, sizes, plan, strict=True):
        spans = []
        replacements = 0
        for block_index, block in enumerate(page.get_text('dict', flags=0)['blocks']):
            for line_index, line in enumerate(block.get('lines', [])):
                dx, dy = line['dir']
                for span in line['spans']:
                    value = span['text']
                    if not value.strip():
                        continue
                    height = span['size'] * PT_TO_MM * .72
                    quad = pymupdf.recover_span_quad(line['dir'], span)
                    width = (quad.ur - quad.ul).norm() * PT_TO_MM
                    arial_width = font.get_text_length(value, height)
                    spans.append({
                        'value': value, 'origin': [v * PT_TO_MM for v in span['origin']],
                        'cap_height': height,
                        'width_factor': width / arial_width if width > 0 and arial_width > 0 else 1,
                        'rotation': atan2(-dy, dx),
                        'bbox': [v * PT_TO_MM for v in span['bbox']],
                    })
                    if trial_mtext:
                        spans[-1].update(block=block_index, line=line_index)
                    replacements += value.count('\ufffd')
        payload_sheet = {'number': sheet['number'], 'size_pt': size, 'spans': spans}
        if trial_mtext:
            payload_sheet['trial_mtext'] = True
        if restore_dashes:
            dashed, solid, unsupported = source_patterns(page)
            payload_sheet['dash_strokes'] = [
                {'endpoints': key, 'solid_overlap': key in solid, 'patterns': patterns}
                for key, patterns in dashed.items()
            ]
            payload_sheet['dash_unsupported'] = unsupported
            payload_sheet['dash_beziers'] = short_beziers(page) if unsupported else []
        pages.append(payload_sheet)
        report.append({
            'sheet': sheet['number'], 'size_pt': size, 'text_spans': len(spans),
            'multiline': 0, 'dash_unmatched': 0, 'replacement_chars': replacements,
            'omitted_images': len(page.get_image_info()),
        })

    return pages, report


def build(command, work, stage, pdf, sizes, plan, restore_dashes, trial_mtext, extractor=None):
    payload = work / 'rust-payload.json'
    if extractor is None:
        pages, report = python_payload(pdf, sizes, plan, restore_dashes, trial_mtext)
    else:
        numbers = [sheet['number'] for sheet in plan]
        result = subprocess.run(
            [str(extractor), '_extract-pdf', str(work / 'source.pdf'), str(payload),
             str(text_font_path()), ','.join(map(str, numbers)), str(int(restore_dashes))],
            stdin=subprocess.DEVNULL, capture_output=True, encoding='utf-8', errors='replace', timeout=300,
        )
        if result.returncode:
            raise RuntimeError(f'Rust PDF extraction failed ({result.returncode}): {result.stderr.strip()}')
        pages = json.loads(payload.read_text(encoding='utf-8'))
        if not isinstance(pages, list) or [page.get('number') for page in pages] != numbers:
            raise RuntimeError('Rust PDF extractor returned an unexpected sheet inventory')
        report = []
        for page, size in zip(pages, sizes, strict=True):
            number = page['number']
            if not isinstance(page.get('spans'), list) or type(page.get('omitted_images')) is not int or page['omitted_images'] < 0:
                raise RuntimeError(f'Sheet {number}: invalid Rust PDF extraction report')
            page['size_pt'] = size
            page['trial_mtext'] = trial_mtext
            report.append({
                'sheet': number, 'size_pt': size, 'text_spans': len(page['spans']),
                'replacement_chars': sum(span['value'].count('\ufffd') for span in page['spans']),
                'omitted_images': page['omitted_images'],
            })
    payload.write_text(json.dumps(pages), encoding='utf-8')
    stage.mkdir()
    result = subprocess.run(
        [*command, str(work / 'native.dxf'), str(payload), str(stage / 'drawing.dxf')],
        stdin=subprocess.DEVNULL, capture_output=True, encoding='utf-8', errors='replace', timeout=300,
    )
    if result.returncode:
        raise RuntimeError(f'Rust DXF builder failed ({result.returncode}): {result.stderr.strip()}')
    geometry_rows = json.loads(result.stdout)
    if not isinstance(geometry_rows, list) or len(geometry_rows) != len(report):
        raise RuntimeError('Rust DXF builder returned an unexpected sheet inventory')
    for row, summary in zip(report, geometry_rows):
        if not isinstance(summary, dict) or summary.get('sheet') != row['sheet']:
            raise RuntimeError('Rust DXF builder returned sheets in an unexpected order')
        geometry = summary.get('geometry')
        if not isinstance(geometry, dict) or any(type(n) is not int or n < 0 for n in geometry.values()):
            raise RuntimeError(f"Rust DXF builder returned invalid geometry for sheet {row['sheet']}")
        row['geometry'] = geometry
        if trial_mtext:
            trial_report = summary.get('mtext_trial')
            if not isinstance(trial_report, dict) or not isinstance(trial_report.get('separate_rows'), dict):
                raise RuntimeError(f"Rust DXF builder returned invalid MTEXT diagnostics for sheet {row['sheet']}")
            row['mtext_trial'] = trial_report
        for field in ('dash_segments_restored', 'dash_curves_restored', 'dash_unmatched', 'multiline'):
            value = summary.get(field)
            if type(value) is not int or value < 0:
                raise RuntimeError(f"Rust DXF builder returned invalid {field} for sheet {row['sheet']}")
            row[field] = value
    if not (stage / 'drawing.dxf').is_file():
        raise RuntimeError('Rust DXF builder did not write drawing.dxf')
    (work / 'build_report.json').write_text(json.dumps(report), encoding='utf-8')

"""Restore unambiguous straight PDF dash strokes in universal drawings."""
import re
from collections import defaultdict
from math import isfinite

from universal import PT_TO_MM


def point_key(point):
    def quantize(value):
        scaled = value * 100
        half = round(scaled * 2) / 2
        # PDF extraction and the CAD unit round-trip can move a tie slightly.
        return round(half if abs(scaled - half) <= 1e-5 else scaled)
    return tuple(quantize(value) for value in point)


def segment_key(start, end):
    return tuple(sorted((point_key(start), point_key(end))))


def solid_edges(path):
    first = last = None
    for kind, *points in path['items']:
        if kind in ('re', 'qu'):
            shape = points[0].quad if kind == 're' else points[0]
            corners = (shape.ul, shape.ur, shape.lr, shape.ll)
            yield from zip(corners, corners[1:] + corners[:1])
            first = last = None
        elif kind in ('l', 'c'):
            start, end = points[0], points[-1]
            if start != last:
                first = start
            last = end
            if kind == 'l':
                yield start, end
        else:
            first = last = None
    if path['closePath'] and first is not None and first != last:
        yield last, first


def source_patterns(page):
    dashed = defaultdict(list)
    solid = set()
    unsupported = 0
    for path in page.get_drawings():
        if 's' not in path['type']:
            continue
        match = re.fullmatch(r'\[([^]]*)\]\s*(\S+)\s*', path.get('dashes') or '')
        values = tuple(map(float, match[1].split())) if match else ()
        if not values:
            solid.update(segment_key(start, end) for start, end in solid_edges(path))
            continue
        for previous, item in enumerate(path['items']):
            if item[0] != 'l':
                unsupported += 1
                continue
            key = segment_key(item[1], item[2])
            if previous or path['closePath'] or path['type'] != 's' or float(match[2]) != 0 or not all(isfinite(v) and v > 0 for v in values):
                dashed[key].append(None)
            else:
                # An odd PDF pattern is repeated before alternating dash and gap.
                dashed[key].append((values * 2 if len(values) % 2 else values,
                                    point_key(item[1])))
    return dashed, solid, unsupported


def short_beziers(page):
    occurrences = defaultdict(list)
    for path in page.get_drawings():
        if 's' not in path['type']:
            continue
        for kind, *points in path['items']:
            if kind != 'c':
                continue
            controls = tuple(point_key(p) for p in points)
            match = re.fullmatch(r'\[([^]]*)\]\s*(\S+)\s*', path.get('dashes') or '')
            pattern = list(map(float, match[1].split())) if match else []
            eligible = (
                pattern and all(isfinite(v) and v > 0 for v in pattern)
                and float(match[2]) == 0 and path['type'] == 's'
                and len(path['items']) == 1 and not path['closePath']
                and sum(abs(b - a) for a, b in zip(points, points[1:])) + 0.03 <= pattern[0]
            )
            if len(pattern) % 2:
                pattern *= 2
            occurrences[min(controls, controls[::-1])].append(
                {'controls': controls, 'pattern': pattern} if eligible else None
            )
    return [rows[0] for rows in occurrences.values() if len(rows) == 1 and rows[0] is not None]


def restore(doc, page, sheet, entities):
    dashed, solid, unsupported = source_patterns(page)
    if not dashed:
        return entities, 0, unsupported
    candidates = defaultdict(list)
    segments = {}
    shift = sheet['model_offset_mm']
    height = sheet['size_mm'][1]

    def key(segment):
        return segment_key(*[
            ((p.x - shift) / PT_TO_MM, (height - p.y) / PT_TO_MM)
            for p in (segment.dxf.start, segment.dxf.end)
        ])

    for entity in entities:
        kind = entity.dxftype()
        if kind == 'LINE':
            parts = [entity]
        elif kind == 'LWPOLYLINE' and not entity.has_width:
            parts = list(entity.virtual_entities())
        else:
            continue
        segments[entity.dxf.handle] = parts
        for index, part in enumerate(parts):
            if part.dxftype() == 'LINE' and (stroke := key(part)) in dashed:
                candidates[stroke].append((entity.dxf.handle, index))

    updates = defaultdict(dict)
    matched = 0
    for stroke, patterns in dashed.items():
        matches = candidates[stroke]
        if stroke in solid or len(set(patterns)) != 1 or patterns[0] is None or not matches:
            continue
        # AutoCAD can emit the same edge in neighbouring polylines. Restore
        # every copy when the PDF agrees on the stroke and its direction.
        for handle, index in matches:
            updates[handle][index] = patterns[0]
        matched += 1

    model = doc.modelspace()
    result = []
    for entity in entities:
        styles = updates.get(entity.dxf.handle)
        if not styles:
            result.append(entity)
            continue
        parts = segments[entity.dxf.handle]
        for index, (pattern, pdf_start) in styles.items():
            name = 'VSOFT_DASH_' + '_'.join(format(v, '.12g') for v in pattern)
            if name not in doc.linetypes:
                lengths = [v * PT_TO_MM for v in pattern]
                doc.linetypes.new(name, dxfattribs={
                    'description': 'PDF dash pattern',
                    'pattern': [sum(lengths), *(v if i % 2 == 0 else -v for i, v in enumerate(lengths))],
                })
            part = parts[index]
            start = part.dxf.start
            if point_key(((start.x - shift) / PT_TO_MM, (height - start.y) / PT_TO_MM)) != pdf_start:
                part.dxf.start, part.dxf.end = part.dxf.end, start
            part.dxf.linetype = name
            part.dxf.ltscale = 1
        if entity.dxftype() != 'LINE':
            for part in parts:
                model.add_entity(part)
            model.delete_entity(entity)
        result.extend(parts)
    return result, matched, unsupported + len(dashed) - matched

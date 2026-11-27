import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from types import SimpleNamespace

import ezdxf
import pymupdf

from universal import PT_TO_MM
from universal_dashes import point_key, restore, segment_key, source_patterns


class SolidContourTests(unittest.TestCase):
    def test_half_centipoint_matches_with_and_without_solid_overlap(self):
        self.assertEqual(point_key((2.675, -2.675)), (268, -268))
        for overlap in (False, True):
            with self.subTest(overlap=overlap), pymupdf.open() as pdf:
                page = pdf.new_page(width=100, height=100)
                if overlap:
                    page.draw_rect((2.675, 10, 90, 90))
                page.draw_line((2.675, 10), (90, 10), dashes='[2 3] 0')
                drawing = ezdxf.new()
                start, end = [(x * PT_TO_MM, 90 * PT_TO_MM) for x in (2.675, 90)]
                line = drawing.modelspace().add_line(start, end)
                sheet = {'model_offset_mm': 0, 'size_mm': (100 * PT_TO_MM,) * 2}
                entities, restored, unmatched = restore(drawing, page, sheet, [line])
                self.assertEqual((restored, unmatched), (0, 1) if overlap else (1, 0))
                self.assertEqual(entities, [line])
                self.assertEqual(line.dxf.linetype.startswith('VSOFT_DASH_'), not overlap)
                self.assertEqual(tuple(line.dxf.start)[:2], start)

    def test_dashed_stroke_over_rectangle_keeps_original_geometry(self):
        with pymupdf.open() as pdf:
            page = pdf.new_page(width=100, height=100)
            page.draw_rect((10, 10, 90, 90))
            page.draw_line((10, 10), (90, 10), dashes='[2 3] 0')
            self.assertEqual(page.get_drawings()[0]['items'][0][0], 're')

            drawing = ezdxf.new()
            model = drawing.modelspace()
            points = [(x * PT_TO_MM, (100 - y) * PT_TO_MM)
                      for x, y in [(10, 10), (90, 10), (90, 90), (10, 90)]]
            frame = model.add_lwpolyline(points, close=True)
            edge = model.add_line(*points[:2])
            sheet = {'model_offset_mm': 0, 'size_mm': (100 * PT_TO_MM,) * 2}
            entities, restored, unmatched = restore(drawing, page, sheet, [frame, edge])

        self.assertEqual((restored, unmatched), (0, 1))
        self.assertEqual(entities, [frame, edge])
        self.assertEqual(list(model), [frame, edge])
        self.assertTrue(all(e.dxf.linetype == 'BYLAYER' for e in entities))

    def test_quad_and_implicit_closure_contribute_solid_edges(self):
        ul, ur, ll, lr = [(10, 10), (80, 15), (20, 75), (90, 80)]
        quad = pymupdf.Quad(ul, ur, ll, lr)
        a, b, c = (0, 0), (2, 0), (1, 1)
        page = SimpleNamespace(get_drawings=lambda: [
            {'type': 's', 'dashes': '[] 0', 'closePath': False, 'items': [('qu', quad)]},
            {'type': 's', 'dashes': '[] 0', 'closePath': True,
             'items': [('l', a, b), ('l', b, c)]},
        ])
        _, solid, _ = source_patterns(page)
        self.assertEqual(solid, {segment_key(x, y) for x, y in [
            (ul, ur), (ur, lr), (lr, ll), (ll, ul), (a, b), (b, c), (c, a),
        ]})

    @unittest.skipUnless(os.environ.get('VSOFT_BIN'), 'requires VSOFT_BIN and AutoCAD Core Console')
    def test_half_centipoint_round_trip_to_dwg(self):
        launcher = Path(os.environ['VSOFT_BIN']).resolve()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for overlap in (False, True):
                source = root / f'edge-{overlap}.pdf'
                with pymupdf.open() as pdf:
                    page = pdf.new_page(width=100, height=100)
                    if overlap:
                        page.draw_rect((2.675, 10, 90, 90))
                    page.draw_line((2.675, 10), (90, 10), dashes='[2 3] 0')
                    pdf.save(source)
                for builder in ('python', 'rust'):
                    with self.subTest(builder=builder, overlap=overlap):
                        destination = root / f'{builder}-{overlap}'
                        result = subprocess.run([
                            str(launcher), 'convert-result', str(source), 'none', str(destination),
                            '--profile', 'universal', '--skip-preview', '--builder', builder,
                        ], cwd=Path(__file__).resolve().parents[1], capture_output=True, timeout=60)
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertTrue((destination / 'drawing.dwg').is_file())
                        row, = json.loads((destination / 'report.json').read_text(encoding='utf-8'))
                        self.assertEqual(row['dash_segments_restored'], int(not overlap))
                        self.assertEqual(row['dash_unmatched'], int(overlap))
                        self.assertEqual('dash_review' in row['review'], overlap)

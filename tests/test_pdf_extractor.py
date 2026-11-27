"""Regression for positioned PDF text. Run with the built Windows vsoft.exe."""
import json
import math
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

import pymupdf

ROOT = Path(__file__).resolve().parents[1]


class PdfExtractorTest(unittest.TestCase):
    def test_preserves_independent_origins_and_real_spaces(self):
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / 'text.pdf'
            output = Path(tmp) / 'extracted.json'
            with pymupdf.open() as pdf:
                page = pdf.new_page()
                page.insert_text((105, 100), '=', fontsize=10)
                page.insert_text((102, 99), '=', fontsize=10)
                page.insert_text((100, 130), 'Left', fontsize=10)
                page.insert_text((122, 130), 'Right', fontsize=10)
                page.insert_text((100, 160), 'Real space', fontsize=10)
                page.draw_line((100, 200), (150, 200), dashes='[5 5] 0')
                page.draw_bezier((100, 220), (101, 220), (102, 221), (103, 221), dashes='[5 5] 0')
                pdf.save(source)
            command = [
                str(ROOT / 'target/release/vsoft.exe'), '_extract-pdf',
                str(source), str(output), str(Path(os.environ['WINDIR']) / 'Fonts/arial.ttf'),
                '85',
            ]
            subprocess.run([*command, '0'], check=True)
            sheet, = json.loads(output.read_text(encoding='utf-8'))
            self.assertEqual(sheet['number'], 85)
            self.assertEqual(sheet['dash_strokes'], [])
            self.assertEqual(sheet['dash_beziers'], [])
            spans = sheet['spans']
            self.assertEqual(sorted(span['value'] for span in spans), ['=', '=', 'Left', 'Real space', 'Right'])
            for label, x, y in [('=', 105, 100), ('=', 102, 99), ('Left', 100, 130), ('Right', 122, 130)]:
                expected = (x * 25.4 / 72, y * 25.4 / 72)
                self.assertEqual(sum(span['value'] == label and math.dist(span['origin'], expected) < .001
                                     for span in spans), 1)
            subprocess.run([*command, '1'], check=True)
            enabled, = json.loads(output.read_text(encoding='utf-8'))
            self.assertEqual(len(enabled['dash_strokes']), 1)
            self.assertEqual(len(enabled['dash_beziers']), 1)


if __name__ == '__main__':
    unittest.main()

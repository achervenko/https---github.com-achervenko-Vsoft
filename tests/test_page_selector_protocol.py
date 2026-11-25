import os
import subprocess
import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RUN_CLI = """
import os
import runpy
import subprocess
import sys
from unittest.mock import patch

code, payload = sys.argv[1:]
answer = subprocess.CompletedProcess(
    args=['selector'], returncode=int(code), stdout=bytes.fromhex(payload),
    stderr=b'The page selection must contain no duplicates.' if code == '2' else b'',
)
os.environ['VSOFT_PAGE_SELECTOR'] = 'selector.exe'
sys.argv = ['convert.py', 'source.pdf', 'reference.dxf', 'unused', '--acad', sys.executable]
real_run = subprocess.run
def run(args, *rest, **kwargs):
    if args[:2] == ['selector.exe', '_select-pages']:
        return answer
    return real_run(args, *rest, **kwargs)

with patch('pymupdf.open', return_value=[None] * 103), patch('subprocess.run', side_effect=run):
    runpy.run_path('convert.py', run_name='__main__')
"""


@unittest.skipUnless(os.name == 'nt', 'convert.py uses Windows file locking')
class PageSelectorProtocolTests(unittest.TestCase):
    def run_cli(self, code, payload):
        return subprocess.run(
            [sys.executable, '-c', RUN_CLI, str(code), payload.hex()],
            cwd=ROOT,
            capture_output=True,
            check=False,
        )

    def test_invalid_success_response_exits_one(self):
        for payload in (b'', b'not JSON', b'\xff', b'{}', b'[]', b'[true]', b'[1.0]',
                        b'["1"]', b'[1,1]', b'[0]', b'[104]'):
            with self.subTest(payload=payload):
                result = self.run_cli(0, payload)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn(b'Invalid Rust page selector response', result.stderr)

    def test_selector_rejection_exits_two(self):
        result = self.run_cli(2, b'')
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn(b'convert.py: error: The page selection must contain no duplicates.', result.stderr)

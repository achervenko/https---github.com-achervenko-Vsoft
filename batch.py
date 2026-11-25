"""Run an explicit queue of PDF jobs without a chat or an API."""
import argparse
import json
import subprocess
import sys
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('queue', type=Path)
parser.add_argument('reference_dxf', type=Path)
parser.add_argument('--skip-preview', action='store_true')
args = parser.parse_args()
queue = args.queue.resolve()
jobs = json.loads(queue.read_text(encoding='utf-8-sig'))
results = []
for job in jobs:
    source = queue.parent / job['pdf']
    destination = queue.parent / job['destination']
    command = [sys.executable, str(Path(__file__).with_name('convert.py')),
               str(source), str(args.reference_dxf.resolve()), str(destination)]
    if job.get('pages'):
        command.extend(['--pages', job['pages']])
    if args.skip_preview:
        command.append('--skip-preview')
    code = subprocess.run(command).returncode
    results.append({'pdf': job['pdf'], 'destination': job['destination'], 'exit_code': code})
    queue.with_suffix('.status.json').write_text(json.dumps(results, ensure_ascii=False, indent=2), encoding='utf-8')
raise SystemExit(int(any(row['exit_code'] for row in results)))

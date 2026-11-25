"""Make a queue for all PDFs in a directory, split into bounded DWG jobs."""
import argparse
import hashlib
import json
from pathlib import Path
import pymupdf

parser=argparse.ArgumentParser()
parser.add_argument('source_folder',type=Path)
parser.add_argument('result_folder',type=Path)
parser.add_argument('queue_file',type=Path)
parser.add_argument('--sheets-per-dwg',type=int,default=100,choices=range(1,201),metavar='1..200')
args=parser.parse_args()
sources=sorted(p for p in args.source_folder.resolve().rglob('*') if p.is_file() and p.suffix.lower()=='.pdf')
if not sources: parser.error('No PDFs found')
jobs=[]
for pdf in sources:
    with pymupdf.open(pdf) as source:
        total=len(source)
    project=hashlib.sha256(str(pdf).encode()).hexdigest()[:12]
    for first in range(1,total+1,args.sheets_per_dwg):
        last=min(total,first+args.sheets_per_dwg-1)
        jobs.append({'pdf':str(pdf),'destination':str(args.result_folder.resolve()/project/f'{first:04d}-{last:04d}'),'pages':f'{first}-{last}'})
args.queue_file.parent.mkdir(parents=True,exist_ok=True)
args.queue_file.write_text(json.dumps(jobs,ensure_ascii=False,indent=2),encoding='utf-8')
print(f'{len(sources)} PDFs, {len(jobs)} jobs')

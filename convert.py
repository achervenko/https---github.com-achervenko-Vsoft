import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
import atexit
import os
import msvcrt
from pathlib import Path

import ezdxf
import pymupdf
import plot_verify
import input_pages

parser = argparse.ArgumentParser()
parser.add_argument('pdf', type=Path)
parser.add_argument('reference_dxf', type=Path, help='DXF export of the agreed DWG reference')
parser.add_argument('destination', type=Path)
parser.add_argument('--pages', help='Comma-separated PDF page numbers and ranges, e.g. 1,2,16,51,100-103')
parser.add_argument('--acad', type=Path, default=Path(r'C:\Program Files\Autodesk\AutoCAD 2024\accoreconsole.exe'))
parser.add_argument('--skip-preview', action='store_true', help='Validate DWG and DXF without plotting PDF')
args = parser.parse_args()

try:
    acad_executable = args.acad.resolve(strict=True)
except (OSError, RuntimeError) as error:
    parser.error(f'Cannot resolve --acad path {args.acad}: {error}')
if not acad_executable.is_file():
    parser.error(f'--acad must point to an AutoCAD executable file: {acad_executable}')

src = args.pdf.resolve()
dest = args.destination.resolve()
reference = args.reference_dxf.resolve()
doc = pymupdf.open(src)
total = len(doc)
try:
    numbers = input_pages.select_pages(args.pages, total)
except ValueError as error:
    parser.error(str(error))
options = json.dumps([numbers, args.skip_preview, str(acad_executable)]).encode('utf-8')
key = hashlib.sha256(src.read_bytes()+reference.read_bytes()+options+Path(__file__).read_bytes()+Path(__file__).with_name('build.py').read_bytes()+Path(plot_verify.__file__).read_bytes()+Path(input_pages.__file__).read_bytes()).hexdigest()
dest.mkdir(parents=True,exist_ok=True)
lock_dir = dest/'_work'
lock_dir.mkdir(exist_ok=True)
lock = (lock_dir/'package.lock').open('a+b')
try:
    msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
except OSError:
    raise SystemExit('Another conversion is already using this destination.')
atexit.register(lock.close)
done = dest/'complete.json'
outputs = ('drawing.dwg','report.json')
if not args.skip_preview:
    outputs += ('preview.pdf',)
elif (dest/'preview.pdf').exists():
    raise SystemExit('Use a separate destination for --skip-preview so an old PDF is not mistaken for a new one.')

def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream,'sha256').hexdigest()

try:
    previous = json.loads(done.read_text(encoding='utf-8'))
    cached = previous['key']==key and all(
        digest(dest/name)==previous['sha256'][name] for name in outputs
    )
except (OSError, ValueError, KeyError, TypeError):
    cached = False
if cached:
    print(f'Already verified: {len(numbers)} sheets')
    raise SystemExit
if done.exists():
    print('Cached results cannot be verified. Rebuilding.')
    done.unlink()
logs = dest/'_work'/key[:12]
logs.mkdir(parents=True,exist_ok=True)
scratch_base = Path(os.environ.get('CAD_SCRATCH_DIR', tempfile.gettempdir())).resolve()
if not scratch_base.as_posix().isascii() or any(ch.isspace() for ch in str(scratch_base)):
    raise SystemExit('Set CAD_SCRATCH_DIR to an existing writable path with ASCII letters and no spaces.')
scratch = tempfile.TemporaryDirectory(prefix='cad_',dir=scratch_base)
atexit.register(scratch.cleanup)
work = Path(scratch.name)

subset = pymupdf.open()
for n in numbers:
    p = doc[n-1]
    if tuple(p.rect) != (0,0,1127,762) or p.rotation or not p.get_text().strip() or not p.get_drawings():
        raise SystemExit(f'Sheet {n}: unsupported template, rotation, or missing editable source; manual review needed')
    subset.insert_pdf(doc,from_page=n-1,to_page=n-1)
subset.save(work/'source.pdf')
(work/'pages.json').write_text(json.dumps({'numbers':numbers,'total':total}))
def acad(name, drawing, commands):
    script = work/(name+'.scr')
    script.write_text('\n'.join(commands)+'\n',encoding='utf-8')
    with (logs/(name+'.log')).open('wb') as log:
        try:
            result = subprocess.run([str(acad_executable),'/i',str(drawing),'/s',str(script),'/l','en-US'],cwd=work,stdout=log,stderr=subprocess.STDOUT,timeout=300)
        except subprocess.TimeoutExpired as error:
            raise RuntimeError(f'AutoCAD timed out. See {log.name}') from error
    if result.returncode:
        raise RuntimeError(f'AutoCAD failed. See {logs/(name+".log")}')

blank=work/'blank.dxf'
ezdxf.new('R2018').saveas(blank)
native=work/'native.dxf'
if not native.exists():
    commands=['FILEDIA','0','PDFIMPORTFILTER','8','PDFIMPORTLAYERS','1','PDFIMPORTMODE','6']
    for i,n in enumerate(numbers):
        commands.extend(['_-PDFIMPORT','_F',(work/'source.pdf').as_posix(),str(i+1),f'{i*20},0','1','0'])
    acad('import',blank,commands+['_DXFOUT',native.as_posix(),'16','_QUIT','_Y'])
if not native.exists(): raise RuntimeError('Missing native DXF. See import.log.')
stage=work/'stage'
subprocess.run([sys.executable,str(Path(__file__).with_name('build.py')),str(work),str(stage),str(reference)],check=True)
dwg=stage/'drawing.dwg'
if dwg.exists(): dwg.unlink()
acad('save',stage/'drawing.dxf',['FILEDIA','0','_SAVEAS','2018',dwg.as_posix(),'_QUIT'])
if not dwg.exists(): raise RuntimeError('Missing DWG. See save.log.')
acad('audit',dwg,['FILEDIA','0','_AUDIT','_N','_QUIT','_Y'])
audit=(logs/'audit.log').read_text(encoding='utf-16-le',errors='replace')
if not any(marker in audit for marker in ('Всего найдено ошибок: 0','Total errors found 0','Total errors found: 0')):
    raise RuntimeError('Review AutoCAD audit.log')
check=ezdxf.readfile(stage/'drawing.dxf')
plot_checks={}
if not args.skip_preview:
    commands=['FILEDIA','0','BACKGROUNDPLOT','0']
    for n in numbers:
        plot=work/f'print-{n:03d}.pdf'
        if plot.exists(): plot.unlink()
        commands.extend(['_-PLOT','_N',f'{n:03d}','','',plot.as_posix(),'_N','_Y'])
    acad('plot',dwg,commands+['_QUIT','_Y'])
    preview=pymupdf.open()
    for n in numbers:
        with pymupdf.open(work/f'print-{n:03d}.pdf') as plot:
            if len(plot)!=1 or not plot[0].get_drawings():
                raise RuntimeError(f'Sheet {n}: empty or invalid plot')
            group=check.groups.get(f'SHEET_{n:03d}')
            plot_checks[n]=plot_verify.inspect(plot[0],n,total,list(group))
            preview.insert_pdf(plot)
    preview.save(stage/'preview.pdf',garbage=4,deflate=True)
report=json.loads((work/'build_report.json').read_text(encoding='utf-8'))
if [row['sheet'] for row in report]!=numbers or len(check.modelspace().query('INSERT'))!=len(numbers) or len(check.layouts)!=len(numbers)+1:
    raise RuntimeError('Invalid sheet inventory in the output drawing or report.')
for row in report:
    group = check.groups.get(f'SHEET_{row["sheet"]:03d}')
    entities = list(group) if group is not None else []
    if not any(e.dxftype()=='MTEXT' and e.dxf.layer=='TEXT' for e in entities) or not any(e.dxftype() in ('LINE','LWPOLYLINE','ARC','CIRCLE','HATCH','SOLID','SPLINE','ELLIPSE') and e.dxf.layer!='LOGO' for e in entities):
        raise RuntimeError(f'Sheet {row["sheet"]}: missing text or geometry in DXF.')
    row['review']=[flag for flag, condition in [('missing_geometry',not row['geometry']),('missing_text',not row['text_spans']),('unmapped_characters',row['replacement_chars']>0),('dash_review',row['dash_unmatched']>0)] if condition]
    if row['sheet'] in plot_checks:
        row['plot_check']=plot_checks[row['sheet']]
        diagnostics=row['plot_check']
        if diagnostics['missing_word_count'] or diagnostics['replacement_chars']:
            row['review'].append('plot_text_review')
        if diagnostics['missing_title_words']:
            row['review'].append('plot_title_review')
(stage/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
checksums={name:digest(stage/name) for name in outputs}
for name in outputs:
    temporary=dest/(name+'.pending')
    shutil.copyfile(stage/name,temporary)
    temporary.replace(dest/name)
temporary=dest/'complete.json.pending'
temporary.write_text(json.dumps({'key':key,'sheets':numbers,'total':total,'sha256':checksums,'preview_printed':not args.skip_preview}),encoding='utf-8')
temporary.replace(done)
print(f'{len(numbers)}/{total} sheets; {sum(r["multiline"] for r in report)} multiline groups; {sum(bool(r["review"]) for r in report)} flagged sheets')





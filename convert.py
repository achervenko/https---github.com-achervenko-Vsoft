import argparse
import hashlib
import json
from math import isclose
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
import universal
import universal_dashes
import universal_rust


def select_source_pages(selection, total):
    selector = os.environ.get('VSOFT_PAGE_SELECTOR')
    if not selector:
        return input_pages.select_pages(selection, total)
    result = subprocess.run(
        [selector, '_select-pages', str(total), str(sys.get_int_max_str_digits())],
        input=(selection or '').encode('utf-8'),
        capture_output=True,
        check=False,
    )
    diagnostic = result.stderr.decode('utf-8', errors='replace').strip()
    if result.returncode == 2:
        raise ValueError(diagnostic)
    if result.returncode:
        raise RuntimeError(f'Rust page selection failed: {diagnostic}')
    try:
        numbers = json.loads(result.stdout)
    except ValueError as error:
        raise RuntimeError(f'Invalid Rust page selector response: {error}') from error
    if (
        not isinstance(numbers, list)
        or not numbers
        or any(type(number) is not int or not 1 <= number <= total for number in numbers)
        or len(set(numbers)) != len(numbers)
    ):
        raise RuntimeError(
            f'Invalid Rust page selector response: expected a nonempty list of unique integers within 1-{total}'
        )
    return numbers


parser = argparse.ArgumentParser()
parser.add_argument('pdf', type=Path)
parser.add_argument('reference_dxf', type=Path, help='DXF reference for EPLAN; pass none for the universal profile')
parser.add_argument('destination', type=Path)
parser.add_argument('--pages', help='Comma-separated PDF page numbers and ranges, e.g. 1,2,16,51,100-103')
parser.add_argument('--acad', type=Path, default=Path(r'C:\Program Files\Autodesk\AutoCAD 2024\accoreconsole.exe'))
parser.add_argument('--skip-preview', action='store_true', help='Validate DWG and DXF without plotting PDF')
parser.add_argument('--profile', choices=('eplan', 'universal'), default='eplan')
parser.add_argument('--compare-layout', action='store_true', help='Compare Python and Rust universal layout calculations')
parser.add_argument('--no-restore-dashes', action='store_true', help='Disable dash restoration in the universal profile')
parser.add_argument('--builder', choices=('python', 'rust'),
                    help='DXF builder (default: Rust for universal, Python for EPLAN)')
args = parser.parse_args()

try:
    acad_executable = args.acad.resolve(strict=True)
except (OSError, RuntimeError) as error:
    parser.error(f'Cannot resolve --acad path {args.acad}: {error}')
if not acad_executable.is_file():
    parser.error(f'--acad must point to an AutoCAD executable file: {acad_executable}')

src = args.pdf.resolve()
dest = args.destination.resolve()
generic = args.profile == 'universal'
restore_dashes = generic and not args.no_restore_dashes
rust_builder = None
pdf_preparer = None
builder_command = None
if (args.builder or ('rust' if generic else 'python')) == 'rust':
    if not generic:
        parser.error('--builder rust requires the universal profile')
    override = os.environ.get('VSOFT_RUST_BUILDER')
    launcher = os.environ.get('VSOFT_PAGE_SELECTOR')
    configured_builder = override or launcher or Path(__file__).parent / 'target' / 'release' / 'vsoft.exe'
    try:
        rust_builder = Path(configured_builder).resolve(strict=True)
    except (OSError, RuntimeError) as error:
        parser.error(f'Cannot locate the Rust builder: {error}. Build VSoft or pass --builder python.')
    if not rust_builder.is_file():
        parser.error(f'Rust builder must point to a file: {rust_builder}')
    builder_command = [str(rust_builder)] if override else [str(rust_builder), '_build-dxf']
    # An overridden DXF builder may be a standalone executable.
    pdf_preparer = Path(launcher or Path(__file__).parent / 'target' / 'release' / 'vsoft.exe').resolve()
    if not pdf_preparer.is_file():
        parser.error(f'Cannot locate the VSoft PDF preparer: {pdf_preparer}')
reference = None if generic else args.reference_dxf.resolve()
doc = pymupdf.open(src)
total = len(doc)
try:
    numbers = select_source_pages(args.pages, total)
except ValueError as error:
    parser.error(str(error))
sizes = [[doc[n-1].rect.width, doc[n-1].rect.height] for n in numbers] if generic else []
plan = universal.plan_layout(numbers, sizes) if generic else []
if args.compare_layout:
    selector = os.environ.get('VSOFT_PAGE_SELECTOR')
    if not generic or not selector:
        parser.error('--compare-layout requires the universal profile and the vsoft launcher')
    payload = [{'number': number, 'size_pt': size} for number, size in zip(numbers, sizes)]
    comparison = subprocess.run(
        [selector, '_plan-layout'], input=json.dumps(payload), encoding='utf-8',
        capture_output=True, check=True,
    )
    rust_plan = json.loads(comparison.stdout)
    if not isinstance(rust_plan, list) or len(rust_plan) != len(plan):
        raise RuntimeError('Rust layout returned an unexpected sheet inventory')
    for expected, actual in zip(plan, rust_plan):
        if not isinstance(actual, dict) or expected.keys() != actual.keys():
            raise RuntimeError(f"Sheet {expected['number']}: unexpected Rust layout fields")
        for field, value in expected.items():
            if not universal.matching_layout_value(value, actual[field]):
                raise RuntimeError(f"Sheet {expected['number']}: layout mismatch for {field}: {value!r} != {actual[field]!r}")
    print(f'Rust layout matches Python: {len(plan)} sheets', file=sys.stderr)
unsupported_plot_sheets = {
    sheet['number'] for sheet in plan if sheet['plot_media'] is None
} if not args.skip_preview else set()
no_plot_media = bool(unsupported_plot_sheets)
args.skip_preview = args.skip_preview or no_plot_media
trial_mtext = rust_builder is not None and os.environ.get('VSOFT_TRIAL_MTEXT', '1') == '1'
extractor = os.environ.get('VSOFT_PDF_EXTRACTOR', 'rust') if rust_builder else 'python'
if extractor not in ('python', 'rust'):
    parser.error('VSOFT_PDF_EXTRACTOR must be python or rust')
options = json.dumps([extractor, numbers, args.skip_preview, str(acad_executable), args.profile, no_plot_media, trial_mtext]).encode('utf-8')
builder = Path(__file__).with_name('build.py')
implementation = Path(universal.__file__) if generic else builder
reference_bytes = reference.read_bytes() if reference is not None else b''
key = hashlib.sha256(src.read_bytes()+reference_bytes+options+Path(__file__).read_bytes()+implementation.read_bytes()+Path(plot_verify.__file__).read_bytes()+Path(input_pages.__file__).read_bytes()).hexdigest()
if restore_dashes:
    key = hashlib.sha256(bytes.fromhex(key) + Path(universal_dashes.__file__).read_bytes()).hexdigest()
if rust_builder is not None:
    with rust_builder.open('rb') as stream:
        builder_version = hashlib.file_digest(stream, 'sha256').digest()
    key = hashlib.sha256(bytes.fromhex(key) + json.dumps(builder_command[1:]).encode() + builder_version
                         + Path(universal_rust.__file__).read_bytes()).hexdigest()
    if pdf_preparer != rust_builder:
        with pdf_preparer.open('rb') as stream:
            key = hashlib.sha256(bytes.fromhex(key) + hashlib.file_digest(stream, 'sha256').digest()).hexdigest()
    if extractor == 'rust':
        with universal.text_font_path().open('rb') as stream:
            key = hashlib.sha256(bytes.fromhex(key) + hashlib.file_digest(stream, 'sha256').digest()).hexdigest()
dest.mkdir(parents=True,exist_ok=True)
lock_dir = dest/'_work'
lock_dir.mkdir(exist_ok=True)
lock = (lock_dir/'package.lock').open('a+b')
try:
    lock.seek(0)
    msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
except OSError:
    raise SystemExit('Another conversion is already using this destination.')
atexit.register(lock.close)
done = dest/'complete.json'
outputs = ('drawing.dwg','report.json')
if not args.skip_preview:
    outputs += ('preview.pdf',)
elif not no_plot_media and (dest/'preview.pdf').exists():
    raise SystemExit('Use a separate destination for --skip-preview so an old PDF is not mistaken for a new one.')

def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream,'sha256').hexdigest()


def write_run_receipt(checksums):
    # The launcher supplies a fresh path for this invocation. Write before
    # returning, while the destination is still locked.
    receipt_path = os.environ.get('VSOFT_RESULT_RECEIPT')
    if receipt_path:
        try:
            with Path(receipt_path).open('x', encoding='utf-8') as stream:
                json.dump({
                    'key': key,
                    'sha256': checksums,
                    'preview_printed': not args.skip_preview,
                }, stream)
        except OSError as error:
            print(f'Could not write run receipt {receipt_path!r}: {error}', file=sys.stderr)


try:
    previous = json.loads(done.read_text(encoding='utf-8'))
    cached = (
        previous['key'] == key
        and previous['preview_printed'] is (not args.skip_preview)
        and set(previous['sha256']) == set(outputs)
        and (not no_plot_media or not (dest/'preview.pdf').exists())
        and all(digest(dest/name) == previous['sha256'][name] for name in outputs)
    )
except (OSError, ValueError, KeyError, TypeError):
    cached = False
if cached:
    write_run_receipt(previous['sha256'])
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

def matching_pdf_size(actual, expected):
    # PyMuPDF rounds rectangle coordinates and dimensions to float32.
    return len(actual) == 2 and all(
        isclose(a, b, rel_tol=2e-7, abs_tol=0.001) for a, b in zip(actual, expected, strict=True)
    )


def normalize_pdf_rotation(page):
    if not page.rotation:
        return
    media = page.mediabox
    crop = page.cropbox
    visible = pymupdf.Rect(crop.x0, media.y1 - crop.y1, crop.x1, media.y1 - crop.y0) & media
    transform = ~page.remove_rotation()
    # Quarter turns reset CropBox. Apply the same transform to the visible area.
    if visible != media:
        visible = visible * transform
        media = media * transform
        page.set_mediabox(media)
        page.set_cropbox(pymupdf.Rect(
            visible.x0, media.y1 - visible.y1, visible.x1, media.y1 - visible.y0,
        ))


if pdf_preparer is not None:
    prepared = work/'prepared.pdf'
    selection = ','.join(map(str, numbers))
    result = subprocess.run(
        [str(pdf_preparer), '_prepare-pdf', str(src), str(prepared), selection],
        capture_output=True, encoding='utf-8', errors='replace', timeout=300,
    )
    if result.returncode:
        raise RuntimeError(f'Rust PDF preparation failed: {result.stderr.strip()}')
    metadata = json.loads(result.stdout)
    pages = metadata['pages']
    if metadata['total'] != total or [page['number'] for page in pages] != numbers:
        raise RuntimeError('Rust PDF preparer returned an unexpected page inventory')
    subset = pymupdf.open(prepared)
    atexit.register(subset.close)
    if len(subset) != len(numbers):
        raise RuntimeError('Prepared PDF has an unexpected page count')
    for page, info, size in zip(subset, pages, sizes, strict=True):
        n = info['number']
        if (
            info['rotation'] != doc[n-1].rotation or page.rotation != info['rotation']
            or not matching_pdf_size(info['size_pt'], size)
        ):
            raise RuntimeError(f'Sheet {n}: Rust PDF metadata does not match the source')
        if not page.get_drawings():
            raise SystemExit(f'Sheet {n}: missing vector geometry')
        # PDF viewport measurements can make PDFIMPORT apply a drawing scale.
        subset.xref_set_key(page.xref, 'VP', 'null')
        normalize_pdf_rotation(page)
        if page.rotation != 0 or not matching_pdf_size((page.rect.width, page.rect.height), size):
            raise RuntimeError(f'Sheet {n}: incorrect visible size after rotation normalization')
else:
    subset = pymupdf.open()
    for n in numbers:
        p = doc[n-1]
        if not generic:
            if p.rotation:
                raise SystemExit(f'Sheet {n}: unsupported rotation ({p.rotation} degrees)')
            if tuple(p.rect) != (0, 0, 1127, 762):
                raise SystemExit(
                    f'Sheet {n}: unsupported page size ({p.rect.width:g} x {p.rect.height:g} pt; expected 1127 x 762 pt)'
                )
            if not p.get_text().strip():
                raise SystemExit(f'Sheet {n}: missing PDF text')
        if not p.get_drawings():
            raise SystemExit(f'Sheet {n}: missing vector geometry')
        subset.insert_pdf(doc, from_page=n-1, to_page=n-1)
        if generic:
            normalize_pdf_rotation(subset[-1])
subset.save(work/'source.pdf')
(work/'pages.json').write_text(json.dumps({'numbers':numbers,'total':total, **({'sizes_pt': sizes} if generic else {})}))
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
offsets = [sheet['import_offset_in'] for sheet in plan] if generic else [i*20 for i in range(len(numbers))]
if not native.exists():
    commands=['FILEDIA','0','PDFIMPORTFILTER','8','PDFIMPORTLAYERS','1','PDFIMPORTMODE','6']
    for i,n in enumerate(numbers):
        commands.extend(['_-PDFIMPORT','_F',(work/'source.pdf').as_posix(),str(i+1),f'{offsets[i]},0','1','0'])
    acad('import',blank,commands+['_DXFOUT',native.as_posix(),'16','_QUIT','_Y'])
if not native.exists(): raise RuntimeError('Missing native DXF. See import.log.')
stage=work/'stage'
if rust_builder is not None:
    universal_rust.build(builder_command, work, stage, subset, sizes, plan, restore_dashes, trial_mtext, pdf_preparer if extractor == "rust" else None)
elif generic:
    universal.build(work, stage, subset, sizes, plan, universal_dashes.restore if restore_dashes else None)
else:
    subprocess.run([sys.executable,str(builder),str(work),str(stage),str(reference)],check=True)
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
    printable = list(zip(numbers, sizes)) if generic else [(n, None) for n in numbers]
    for n, _ in printable:
        plot=work/f'print-{n:03d}.pdf'
        if plot.exists(): plot.unlink()
        commands.extend(['_-PLOT','_N',f'{n:03d}','','',plot.as_posix(),'_N','_Y'])
    acad('plot',dwg,commands+['_QUIT','_Y'])
    preview=pymupdf.open()
    for n, size in printable:
        with pymupdf.open(work/f'print-{n:03d}.pdf') as plot:
            if len(plot)!=1 or not plot[0].get_drawings():
                raise RuntimeError(f'Sheet {n}: empty or invalid plot')
            group=check.groups.get(f'SHEET_{n:03d}')
            plot_checks[n]=(plot_verify.inspect_universal(plot[0], n, size, doc[n-1].get_text())
                            if generic else plot_verify.inspect(plot[0],n,total,list(group)))
            preview.insert_pdf(plot)
    preview.save(stage/'preview.pdf',garbage=4,deflate=True)
report=json.loads((work/'build_report.json').read_text(encoding='utf-8'))
if [row['sheet'] for row in report]!=numbers or (not generic and len(check.modelspace().query('INSERT'))!=len(numbers)) or len(check.layouts)!=len(numbers)+1:
    raise RuntimeError('Invalid sheet inventory in the output drawing or report.')
for row in report:
    group = check.groups.get(f'SHEET_{row["sheet"]:03d}')
    entities = list(group) if group is not None else []
    if (not generic and not any(e.dxftype()=='MTEXT' and e.dxf.layer=='TEXT' for e in entities)) or not any(e.dxftype() in ('LINE','LWPOLYLINE','ARC','CIRCLE','HATCH','SOLID','SPLINE','ELLIPSE','POLYLINE') and e.dxf.layer!='LOGO' for e in entities):
        raise RuntimeError(f'Sheet {row["sheet"]}: missing text or geometry in DXF.')
    row['review']=[flag for flag, condition in [('missing_geometry',not row['geometry']),('missing_text',not row['text_spans']),('unmapped_characters',row['replacement_chars']>0),('dash_review',row['dash_unmatched']>0)] if condition]
    if no_plot_media:
        row['review'].append(
            'plot_media_unavailable' if row['sheet'] in unsupported_plot_sheets
            else 'plot_skipped_due_to_other_sheet'
        )
    if generic and row['omitted_images']:
        row['review'].append('raster_images_omitted')
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
if no_plot_media:
    (dest/'preview.pdf').unlink(missing_ok=True)
temporary.write_text(json.dumps({'key':key,'sheets':numbers,'total':total,'sha256':checksums,'preview_printed':not args.skip_preview}),encoding='utf-8')
temporary.replace(done)
write_run_receipt(checksums)
print(f'{len(numbers)}/{total} sheets; {sum(r["multiline"] for r in report)} multiline groups; {sum(bool(r["review"]) for r in report)} flagged sheets')





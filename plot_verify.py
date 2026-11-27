"""Check the actual pages printed by AutoCAD against the corresponding CAD sheet."""
from collections import Counter
import re
import unicodedata
import pymupdf


def words(text):
    normalized = unicodedata.normalize('NFC', text).casefold()
    return Counter(re.findall(r'[^\W_]+', normalized))


def inspect_universal(page, sheet, size, source_text):
    from universal import PLOT_TOLERANCE_PT
    if (any(abs(actual - expected) > PLOT_TOLERANCE_PT
            for actual, expected in zip((page.rect.width, page.rect.height), size))
            or not page.get_drawings()):
        raise ValueError(f'Sheet {sheet}: plot size differs from source or geometry is missing.')
    text = page.get_text()
    expected = words(source_text)
    missing = expected - words(text)
    return {
        'expected_words': sum(expected.values()),
        'missing_word_count': sum(missing.values()),
        'missing_words': dict(missing),
        'missing_title_words': {},
        'replacement_chars': text.count('\ufffd'),
    }


def inspect(page, sheet, total, entities):
    width, height = page.rect.width, page.rect.height
    if abs(width-1190.55)>2 or abs(height-841.89)>2 or not page.get_drawings():
        raise ValueError(f'Sheet {sheet}: invalid A3 plot or missing geometry.')
    text = page.get_text()
    if not text.strip():
        raise ValueError(f'Sheet {sheet}: missing extractable text in plot.')

    def cell(x0, y0, x1, y1):
        clip=pymupdf.Rect(x0*width/1191, y0*height/842, x1*width/1191, y1*height/842)
        return page.get_text(clip=clip).strip()

    if cell(1080,760,1125,784)!=str(sheet) or cell(1126,760,1170,784)!=str(total):
        raise ValueError(f'Sheet {sheet}: wrong sheet number or total in the printed frame.')

    labels=[e for e in entities if e.dxftype()=='MTEXT']
    expected=words(' '.join(e.plain_text() for e in labels))
    missing=expected-words(text)
    title=[e.plain_text() for e in labels if e.dxf.layer=='FRAME_TEXT' and 11<e.dxf.insert.y<20]
    missing_title=words(' '.join(title))-words(cell(838,782,1029,828))
    return {
        'expected_words':sum(expected.values()),
        'missing_word_count':sum(missing.values()),
        'missing_words':dict(missing),
        'missing_title_words':dict(missing_title),
        'replacement_chars':text.count('\ufffd'),
    }

"""Generate reproducible vector PDF inputs for Windows conversion checks."""
import argparse
from pathlib import Path

import pymupdf


def framed_page(document, size, label):
    page = document.new_page(width=size[0], height=size[1])
    page.draw_rect((30, 30, size[0] - 30, size[1] - 30))
    page.insert_text((50, 65), label, fontsize=14)
    return page


def mixed_pack(output, filename, second_size, second_label, rotate=False):
    with pymupdf.open() as document:
        framed_page(document, (1684, 1191), 'Landscape A2')
        second = framed_page(document, second_size, second_label)
        if rotate:
            second.set_rotation(90)
        document.save(output / filename)


def stamp_fixture(output):
    with pymupdf.open() as document:
        page = document.new_page(width=1190.55, height=841.89)
        for rectangle in ((820, 730, 880, 755), (880, 730, 940, 755), (950, 650, 975, 750)):
            page.draw_rect(rectangle)
        for text, x in (('EQUIPMENT', 823), ('REVISION', 883)):
            anchor = pymupdf.Point(x, 746)
            page.insert_text(anchor, text, fontsize=15, morph=(anchor, pymupdf.Matrix(.53, 1)))
        anchor = pymupdf.Point(967, 746)
        page.insert_text(anchor, 'VERTICAL', fontsize=14, rotate=90,
                         morph=(anchor, pymupdf.Matrix(1, .55)))
        document.save(output / 'stamp.pdf')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    output = parser.parse_args().output
    output.mkdir(parents=True, exist_ok=True)
    mixed_pack(output, 'mixed-iso.pdf', (841.89, 1190.55), 'Portrait A3')
    mixed_pack(output, 'mixed-custom.pdf', (1000, 600), 'Custom rotated', rotate=True)
    with pymupdf.open() as document:
        page = framed_page(document, (700, 900), 'Fractional crop')
        page.set_cropbox(pymupdf.Rect(0.1, 0.375, 600.2, 800.625))
        page.set_rotation(90)
        document.save(output / 'fractional-crop.pdf')
    stamp_fixture(output)


if __name__ == '__main__':
    main()

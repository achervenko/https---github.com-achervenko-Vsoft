"""Page selection shared by conversion and future front ends."""


def select_pages(selection: str | None, total: int) -> list[int]:
    """Return one-based PDF page numbers, in the requested order."""
    if total < 1:
        raise ValueError('The PDF contains no pages.')
    if selection is None or not selection.strip():
        return list(range(1, total + 1))

    numbers = []
    seen = set()
    for segment in selection.split(','):
        first, separator, last = segment.strip().partition('-')
        try:
            start, end = int(first), int(last if separator else first)
        except ValueError:
            raise ValueError(f'Invalid page range. Use numbers within 1-{total}, e.g. 1,5-8.') from None
        if not 1 <= start <= end <= total:
            raise ValueError(f'Invalid page range. Use numbers within 1-{total}, e.g. 1,5-8.')
        for number in range(start, end + 1):
            if number in seen:
                raise ValueError('The page selection must contain no duplicates.')
            seen.add(number)
            numbers.append(number)
    return numbers

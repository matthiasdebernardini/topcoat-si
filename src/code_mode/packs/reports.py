import csv


def _columns(rows, columns):
    if columns is not None:
        return list(columns)
    if not rows:
        return []
    return list(rows[0].keys())


def _cell(value):
    if value is None:
        return ""
    return str(value).replace("|", "\\|").replace("\n", " ")


def table(rows, columns=None):
    names = _columns(rows, columns)
    lines = [
        "| " + " | ".join([str(name) for name in names]) + " |",
        "|" + "|".join(["---" for name in names]) + "|",
    ]
    for row in rows:
        lines.append("| " + " | ".join([_cell(row.get(name)) for name in names]) + " |")
    return "\n".join(lines)


class _Text:
    def __init__(self):
        self.parts = []

    def write(self, text):
        self.parts.append(text)


def to_csv(rows, columns=None):
    names = _columns(rows, columns)
    out = _Text()
    writer = csv.writer(out)
    writer.writerow(names)
    for row in rows:
        writer.writerow([row.get(name) for name in names])
    return "".join(out.parts)


def count_by(rows, key):
    counts = {}
    for row in rows:
        value = row.get(key)
        counts[value] = counts.get(value, 0) + 1
    totals = [{key: value, "count": count} for value, count in counts.items()]
    return sorted(totals, key=lambda total: -total["count"])

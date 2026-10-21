def _row(item):
    if isinstance(item, str):
        return {"text": item}
    return dict(item)


def _failed(row, error):
    row["answer"] = None
    row["confidence"] = None
    row["error"] = str(error)
    return row


async def classify_all(*, items, question, options, min_confidence=0.7):
    async def one(row):
        try:
            got = await choose(state=row, question=question, options=options, min_confidence=min_confidence)
        except (RuntimeError, ValueError) as error:
            return _failed(row, error)
        row["answer"] = got["answer"]
        row["confidence"] = got["confidence"]
        return row

    return await asyncio.gather(*[one(_row(item)) for item in items])


async def score_all(*, items, question, levels, min_confidence=0.7):
    async def one(row):
        try:
            got = await rate(state=row, question=question, levels=levels, min_confidence=min_confidence)
        except (RuntimeError, ValueError) as error:
            return _failed(row, error)
        row["answer"] = got["answer"]
        row["confidence"] = got["confidence"]
        return row

    return await asyncio.gather(*[one(_row(item)) for item in items])


def group_by(rows, key):
    groups = {}
    for row in rows:
        value = row.get(key)
        if value is None:
            value = "unsure"
        if value not in groups:
            groups[value] = []
        groups[value].append(row)
    return groups

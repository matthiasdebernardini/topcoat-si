_LEVELS = ["not useful", "partly useful", "directly useful"]


def _level_rank(level):
    if level is None:
        return -1
    return _LEVELS.index(level)


async def search_and_rank(*, query, task, limit=5, keep=3, min_confidence=0.7):
    hits = await search(query=query, limit=limit, scrape=True)

    async def judge(hit):
        page = hit.get("markdown") or hit.get("description") or ""
        state = {"url": hit["url"], "title": hit["title"], "page": page}
        try:
            rated = await rate(state=state, question=task, levels=_LEVELS, min_confidence=min_confidence)
        except (RuntimeError, ValueError) as error:
            return {
                "url": hit["url"],
                "title": hit["title"],
                "level": None,
                "confidence": None,
                "error": str(error),
            }
        return {
            "url": hit["url"],
            "title": hit["title"],
            "level": rated["answer"],
            "confidence": rated["confidence"],
        }

    def rank(page):
        rated = 0 if "error" in page else 1
        confidence = page["confidence"] if page["confidence"] is not None else -1.0
        return (rated, _level_rank(page["level"]), confidence)

    ranked = await asyncio.gather(*[judge(hit) for hit in hits])
    return sorted(ranked, key=rank, reverse=True)[:keep]


async def scrape_many(*, urls):
    async def one(url):
        try:
            return await scrape(url=url)
        except Exception as error:
            return {"url": url, "error": str(error)}

    return await asyncio.gather(*[one(url) for url in urls])


def _url_key(url):
    key = re.sub(r"^[A-Za-z][A-Za-z0-9+.-]*://", "", url.strip())
    host, slash, path = key.partition("/")
    host = host.lower()
    if host.startswith("www."):
        host = host[4:]
    return (host + slash + path).split("#")[0].rstrip("/")


def dedupe_urls(urls):
    seen = {}
    kept = []
    for url in urls:
        key = _url_key(url)
        if key not in seen:
            seen[key] = True
            kept.append(url)
    return kept

async def rows(*, surql, params={}):
    found = await query(surql=surql, params=params)
    return [row for row in found if not (isinstance(row, dict) and "_truncated" in row)]


async def one(*, surql, params={}):
    found = await rows(surql=surql, params=params)
    return found[0] if found else None


async def count(*, table):
    found = await rows(
        surql="SELECT count() FROM type::table($table) GROUP ALL",
        params={"table": table},
    )
    return found[0]["count"] if found else 0

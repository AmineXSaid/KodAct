# Idea: a per-prefix cache

1. In `config.py`, add `PREFIX_CACHE = True` and `PREFIX_LEN = 2`.
2. In `store.py`, add a `PrefixCache` class that maps the first `PREFIX_LEN` characters of a key
   to the lines of the table that start with them, filled on first use by scanning the table.
3. In `lookup.py`, make `find` ask the `PrefixCache` for the candidate lines instead of scanning
   the whole table, and add a `cache_stats()` function that returns the cache's hit count.

(Keys in this table share long prefixes, so this may not narrow the scan much. Measure it.)

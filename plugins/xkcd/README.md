# xkcd

Find xkcd comics from High Beam.

## Trigger

Type `xkcd <arg>` where `<arg>` is one of:

| Form              | Behaviour                                                |
|-------------------|----------------------------------------------------------|
| `xkcd latest`     | Newest comic (one result).                               |
| `xkcd random`     | Uniformly random comic in `[1, latest]`.                 |
| `xkcd <number>`   | That specific comic. Missing numbers yield 0 results.    |
| `xkcd <text>`     | Fuzzy title search across the cached index.              |

Each result's title is `<num>: <comic title>`; the subtitle is the comic's
`alt` (mouseover) text; the action opens `https://xkcd.com/<num>/`.

## Cache strategy

Text search relies on `xkcd-index.json` in the plugin's cache directory
(`highbeam:fs.cache`). It's populated **lazily on the first text search**:

- If the cache is missing or older than 24 hours (`last_updated` field),
  the plugin re-fetches.
- Fetching is parallel but bounded to **50 concurrent requests** so we
  don't fan out a few thousand connections to xkcd's CDN in one burst.
- The latest comic's metadata is reused from the same `info.0.json` call
  we already need for "newest comic number".

## Limitations

- **Only the latest 500 comics are indexed.** This keeps cache build time
  down (≈10 batches at 50-wide concurrency) and avoids hammering xkcd's
  CDN. Older comics are still reachable by exact number (`xkcd 42`), they
  just won't show up in text search.
- Comic #404 famously 404s — it's skipped during indexing.
- Cache writes that fail are silently swallowed; the index still works
  for the current session.
- The first text search after the cache expires triggers a foreground
  rebuild, so it'll feel slower than subsequent searches. Background
  refresh is post-v1.

## Test

```sh
npm install
npm test
```

# ont

The `ont` command: `schema check|install`, `describe`, `get`, `backup`, `index`, `search` over a format 3 store.

## index
- `ont index [--source NAME]...` runs the sources of the installed schemas, one write transaction per source. One run at a time per store (an advisory lock on `<store>.index.lock`; a held lock is `index_busy`).
- Source kinds `markdown_fields` (`glob`) and `file_digest` (`paths`) take `exclude`, `complete_listing`, `fresh_for_s`; `markdown_fields` also `head_lines`. Globs are absolute; `*` and `?` stay inside one segment; `**` is a whole segment (at most 8); no brace, class or escape syntax.
- `exclude`: an exclude pattern that matches a directory excludes everything under it (the directory is not read); a pattern that matches a file excludes that file.
- A symlink is never followed. A wildcard segment skips symlinks and counts them (`skipped_symlinks`); a literal (wildcard-free) segment that is a symlink fails the source (`glob_root_is_symlink`), so a replaced root never retracts objects. Files are read through one descriptor opened `O_NOFOLLOW|O_NONBLOCK` whose real path must equal the listed path (`path_not_canonical`).
- Bounds per run: 16 MiB per file, 1 GiB in all (`source_bytes_too_large`), 64 MiB retained parsed text (`source_text_too_large`), 10,000 records, 200,000 directory entries.
- Change kinds: `initial`, `update` (values differ), `refresh` (values equal and the head is stale, or the bytes read differ from the head's lineage), nothing otherwise, `retraction` (with `complete_listing`).
- `parse_integer` is strict: the whole trimmed string must parse as an `i64`, so `+5` and `007` are accepted (5 and 7) and `3 items` is unresolved.

## search
- `qualified` covers the page returned (the scanned heads of this request), not the whole set when `continuation` is present. `stale` and `partial` count scanned admitted heads; `stale_sources` lists the sources of every stale scanned head. The 10,000-head scan cap counts heads the principal may not read (a limit until scoped grants exist).

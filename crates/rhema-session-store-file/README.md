# rhema-session-store-file

The reference session store: one bare-JSON `Session` per file, written
atomically and fsynced.

Two stores over one writer:

- `FileStore` is the managed layout, `<base>/<id>.json` or
  `<base>/<encoded-workspace>/<id>.json`.
- `PathStore` is one explicit file, the path `--save` and `--resume`
  name. Its identity is the path, so it does not implement
  `SessionStore`.

```rust
use rhema_session_store::SessionStore;
use rhema_session_store_file::FileStore;

let store = FileStore::scoped(base, &workspace);
let id = store.next_id()?;          // <millis:016>-<n:02>, directory 0700
store.save(&id, &session)?;
let rows = store.list()?;           // newest first
```

The file is the same pretty JSON `--resume` takes: no wrapper, no
envelope. A file written by an older build loads unchanged, and a file
this crate writes resumes through `--save`/`--resume` with no
conversion.

## The write

The session streams into a private temp file
(`.<id>.<pid>.<seq>.tmp`, `create_new`, `0600`) in the target's
directory, the file is flushed and fsynced, renamed over the target, and
the parent directory is fsynced best effort. On any failure the temp is
removed and the previous file stays whole. A crash can leave a
dot-prefixed temp behind; `list` skips it.

Directories are `0700` and files `0600` on unix.

Spec: `specs/rhema-session-store.md` §6.

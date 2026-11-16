//! [`FileStore`]: the managed layout, one bare-JSON session per file.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rhema_core::Session;
use rhema_session_store::{Meta, SessionId, SessionStore, StoreError, opening_prompt};

use crate::layout::encode_workspace;
use crate::write::write_atomic;

/// A store over one directory of `<id>.json` files, optionally grouped
/// by an encoded workspace.
///
/// The directory is created `0700` and each file `0600` on the save
/// path. A store that has never been saved to lists as empty; a missing
/// directory is not an error.
#[derive(Debug, Clone)]
pub struct FileStore {
    /// The root the directories are private relative to. For a scoped
    /// store this is the shared parent, tightened before the workspace
    /// directory below it.
    base: PathBuf,
    /// The directory the session files live in.
    dir: PathBuf,
}

impl FileStore {
    /// A store at `base`, ACP's shape: `<base>/<id>.json`.
    pub fn new(base: impl Into<PathBuf>) -> Self {
        let base = base.into();
        Self {
            dir: base.clone(),
            base,
        }
    }

    /// A store at `<base>/<encoded-workspace>`, the TUI's shape.
    pub fn scoped(base: impl Into<PathBuf>, workspace: &Path) -> Self {
        let base = base.into();
        let dir = base.join(encode_workspace(workspace));
        Self { base, dir }
    }

    /// The directory the session files live in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file `id` saves to. Validated ids are the only path
    /// construction the store performs.
    pub fn path_for(&self, id: &SessionId) -> PathBuf {
        self.dir.join(format!("{}.json", id.as_str()))
    }

    /// Allocates the next free `<millis:016>-<n:02>` id.
    ///
    /// The fixed widths make lexicographic name order time order. The
    /// directory is created `0700`; nothing else is written. Two
    /// processes can pick the same name in the same millisecond, and
    /// the second `save` wins.
    pub fn next_id(&self) -> Result<SessionId, StoreError> {
        self.ensure_dir()?;
        let millis = now_millis();
        let mut n = 0u32;
        loop {
            let raw = format!("{millis:016}-{n:02}");
            let id = SessionId::new(raw.clone()).map_err(|_| {
                StoreError::Backend(format!("allocated session id {raw:?} is invalid"))
            })?;
            if !self.path_for(&id).exists() {
                return Ok(id);
            }
            n += 1;
        }
    }

    /// Creates and tightens both directories on the save path.
    fn ensure_dir(&self) -> Result<(), StoreError> {
        rhema_config::privacy::private_dir(&self.base).map_err(|source| StoreError::Io {
            path: self.base.clone(),
            source,
        })?;
        if self.dir != self.base {
            rhema_config::privacy::private_dir(&self.dir).map_err(|source| StoreError::Io {
                path: self.dir.clone(),
                source,
            })?;
        }
        Ok(())
    }

    /// Reads and parses one record; the shared body of [`Self::load`].
    fn read(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        let path = self.path_for(id);
        match fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|source| StoreError::Corrupt {
                    id: id.clone(),
                    source,
                }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(StoreError::Io { path, source }),
        }
    }
}

impl SessionStore for FileStore {
    fn load(&self, id: &SessionId) -> Result<Option<Session>, StoreError> {
        self.read(id)
    }

    fn save(&self, id: &SessionId, session: &Session) -> Result<(), StoreError> {
        self.ensure_dir()?;
        write_atomic(&self.path_for(id), id.as_str(), session)
    }

    fn delete(&self, id: &SessionId) -> Result<bool, StoreError> {
        match fs::remove_file(self.path_for(id)) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(StoreError::Io {
                path: self.path_for(id),
                source,
            }),
        }
    }

    fn list(&self) -> Result<Vec<Meta>, StoreError> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(StoreError::Io {
                    path: self.dir.clone(),
                    source,
                });
            }
        };
        let mut out = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Dot entries are temp files and anything else that never
            // became a record.
            if name.starts_with('.') {
                continue;
            }
            let Some(stem) = name.strip_suffix(".json") else {
                continue;
            };
            let Ok(id) = SessionId::new(stem) else {
                continue;
            };
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let path = entry.path();
            // A record never vanishes because its clock is unreadable:
            // the id's leading millisecond field is the fallback, and 0
            // the last resort.
            let saved_at = metadata
                .modified()
                .ok()
                .and_then(millis)
                .or_else(|| name_millis(&id))
                .unwrap_or(0);
            // A body that does not parse still lists; the resume
            // reports the corruption.
            let prompt = fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str::<Session>(&raw).ok())
                .and_then(|session| opening_prompt(&session));
            out.push(Meta {
                id,
                saved_at,
                bytes: metadata.len(),
                opening_prompt: prompt,
            });
        }
        out.sort_by(|a, b| b.saved_at.cmp(&a.saved_at).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }
}

/// Milliseconds since the epoch; a clock before it reads as 0.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

/// A system time in milliseconds, `None` when it sits before the epoch.
fn millis(at: SystemTime) -> Option<u64> {
    at.duration_since(UNIX_EPOCH)
        .ok()
        .map(|since| since.as_millis() as u64)
}

/// The millisecond field of an allocated id, `None` for any other
/// shape.
fn name_millis(id: &SessionId) -> Option<u64> {
    let (millis, suffix) = id.as_str().split_once('-')?;
    if millis.len() != 16
        || suffix.len() != 2
        || !millis.bytes().all(|byte| byte.is_ascii_digit())
        || !suffix.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    millis.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhema_core::{Message, Role};
    use rhema_session_store::contract;

    fn session_with(prompt: &str) -> Session {
        Session {
            history: vec![Message::text(Role::User, prompt)],
            ..Session::default()
        }
    }

    fn temp_store() -> (tempfile::TempDir, FileStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::new(dir.path().join("sessions"));
        (dir, store)
    }

    #[test]
    fn the_contract_suite_passes() {
        let (_dir, store) = temp_store();
        contract::run_all(&store);
    }

    #[test]
    fn next_id_never_returns_a_taken_name() {
        let (_dir, store) = temp_store();
        let first = store.next_id().unwrap();
        assert!(
            !store.path_for(&first).exists(),
            "allocation writes nothing"
        );
        store.save(&first, &session_with("taken")).unwrap();
        let second = store.next_id().unwrap();
        assert_ne!(first, second);
        assert!(!store.path_for(&second).exists());
    }

    #[test]
    fn listing_orders_by_mtime_newest_first() {
        let (_dir, store) = temp_store();
        let old = store.next_id().unwrap();
        store.save(&old, &session_with("old")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let new = store.next_id().unwrap();
        store.save(&new, &session_with("new")).unwrap();
        let rows = store.list().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, new);
        assert_eq!(rows[1].id, old);
        assert_eq!(rows[0].opening_prompt.as_deref(), Some("new"));
    }

    #[test]
    fn a_leftover_temp_never_lists() {
        let (_dir, store) = temp_store();
        let id = store.next_id().unwrap();
        store.save(&id, &session_with("real")).unwrap();
        fs::write(store.dir().join(".0001793121258253-00.json.4242.tmp"), b"{").unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn a_save_over_a_record_leaves_no_temp() {
        let (_dir, store) = temp_store();
        let id = store.next_id().unwrap();
        store.save(&id, &session_with("first")).unwrap();
        store.save(&id, &session_with("second")).unwrap();
        let leftovers: Vec<_> = fs::read_dir(store.dir())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp litter: {leftovers:?}");
    }

    #[test]
    fn a_corrupt_file_lists_without_a_prompt_and_fails_to_load() {
        let (_dir, store) = temp_store();
        let id = store.next_id().unwrap();
        store.save(&id, &session_with("prompt")).unwrap();
        fs::write(store.path_for(&id), "{not json").unwrap();

        let rows = store.list().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].opening_prompt, None);
        let err = store.load(&id).unwrap_err();
        assert!(matches!(err, StoreError::Corrupt { .. }), "{err}");
    }

    #[test]
    fn stray_names_are_skipped_and_a_stray_valid_id_lists() {
        let (_dir, store) = temp_store();
        store.ensure_dir().unwrap();
        fs::write(store.dir().join("notes.txt"), b"not a session").unwrap();
        fs::write(store.dir().join("foo.json"), b"{not json").unwrap();
        let rows = store.list().unwrap();
        assert_eq!(rows.len(), 1, "only the valid id lists: {rows:?}");
        assert_eq!(rows[0].id.as_str(), "foo");
        assert!(rows[0].saved_at > 0, "the mtime is the clock");
        assert!(matches!(
            store.load(&SessionId::new("foo").unwrap()),
            Err(StoreError::Corrupt { .. })
        ));
    }

    #[test]
    fn a_hand_written_bare_session_loads_unchanged() {
        let (_dir, store) = temp_store();
        store.ensure_dir().unwrap();
        let session = Session {
            history: vec![Message::text(Role::User, "from the old code")],
            driver_state: serde_json::json!({ "kept": true }),
            cost_usd: 3.5,
            usage_stats: serde_json::json!({ "requests": 1 }),
        };
        let path = store.path_for(&SessionId::new("0001759000000000-00").unwrap());
        fs::write(&path, serde_json::to_string_pretty(&session).unwrap()).unwrap();
        let id = SessionId::new("0001759000000000-00").unwrap();
        assert_eq!(store.load(&id).unwrap(), Some(session));
        let rows = store.list().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0].saved_at > 0,
            "the id's millis field is the fallback"
        );
    }

    #[test]
    #[cfg(unix)]
    fn the_store_lands_private() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, store) = temp_store();
        let id = store.next_id().unwrap();
        store.save(&id, &session_with("prompt")).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dir.path().join("sessions").as_path()), 0o700);
        assert_eq!(mode(store.path_for(&id).as_path()), 0o600);
        fs::set_permissions(store.path_for(&id), fs::Permissions::from_mode(0o644)).unwrap();
        store.save(&id, &session_with("again")).unwrap();
        assert_eq!(mode(store.path_for(&id).as_path()), 0o600);
    }

    #[test]
    fn a_scoped_store_tightens_its_base_too() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::scoped(dir.path().join("sessions"), Path::new("/home/u/proj"));
        let id = store.next_id().unwrap();
        store.save(&id, &session_with("scoped")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(dir.path().join("sessions").as_path()), 0o700);
            assert_eq!(mode(store.dir()), 0o700);
        }
        assert!(store.dir().to_string_lossy().contains("%2Fhome%2Fu%2Fproj"));
    }

    #[test]
    fn a_concurrent_reader_never_sees_a_partial_file() {
        let (_dir, store) = temp_store();
        let id = store.next_id().unwrap();
        let mut big = session_with("concurrent");
        big.history = (0..400)
            .map(|n| Message::text(Role::User, format!("message {n}")))
            .collect();
        store.save(&id, &big).unwrap();
        let writer = {
            let store = store.clone();
            let id = id.clone();
            let big = big.clone();
            std::thread::spawn(move || {
                for _ in 0..40 {
                    store.save(&id, &big).unwrap();
                }
            })
        };
        for _ in 0..200 {
            let parsed = store.load(&id).unwrap().expect("the file is always whole");
            assert_eq!(parsed.history.len(), 400);
        }
        writer.join().unwrap();
    }

    #[test]
    fn deleting_a_missing_id_is_false() {
        let (_dir, store) = temp_store();
        let id = SessionId::new("missing").unwrap();
        assert!(!store.delete(&id).unwrap());
        store.save(&id, &session_with("here")).unwrap();
        assert!(store.delete(&id).unwrap());
    }

    #[test]
    fn an_empty_store_lists_empty() {
        let (_dir, store) = temp_store();
        assert!(store.list().unwrap().is_empty());
    }
}

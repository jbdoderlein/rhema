use super::*;

#[test]
fn a_shell_line_wrapped_for_the_backend_comes_back_as_written() {
    let wrapped = vec!["bash".to_owned(), "-c".to_owned(), "npm run dev".to_owned()];
    assert_eq!(unwrap_command(&wrapped), "npm run dev");
}

#[test]
fn a_command_started_outside_the_shim_shows_its_argv() {
    let bare = vec!["npm".to_owned(), "run".to_owned(), "dev".to_owned()];
    assert_eq!(unwrap_command(&bare), "npm run dev");
}

#[test]
fn the_session_is_read_from_the_tag() {
    assert_eq!(
        owner_of(Some("[s7f2] npm run dev")),
        Some("s7f2".to_owned())
    );
    assert_eq!(owner_of(Some("a note with no tag")), None);
    assert_eq!(owner_of(Some("[unclosed")), None);
    assert_eq!(owner_of(Some("[] empty")), None);
    assert_eq!(owner_of(None), None);
}

#[test]
fn a_tag_longer_than_an_id_is_not_a_session() {
    let long = format!("[{}] cmd", "s".repeat(MAX_ID_CHARS + 1));
    assert_eq!(owner_of(Some(&long)), None);
}

#[test]
fn a_started_job_announces_its_id() {
    assert_eq!(
        parse_added_id("Added job M8c running: bash -c npm run dev\n"),
        Some("M8c".to_owned())
    );
    assert_eq!(
        parse_added_id("Job 8Pc already running (since 877ms ago)\n"),
        Some("8Pc".to_owned())
    );
    assert_eq!(parse_added_id("something else\n"), None);
}

/// A workspace with a fake backend: a shell script that logs every argv it
/// gets and answers `list` from a state file. The paths are baked into the
/// script, so no test mutates process-wide environment or cwd.
#[cfg(unix)]
struct Fake {
    dir: tempfile::TempDir,
    backend: PathBuf,
    log: PathBuf,
    state: PathBuf,
}

/// A backend script, settled: a parallel test can fork while this file's
/// write descriptor is still open, and the child's inherited copy keeps
/// the inode unexecutable until it execs (`ETXTBSY`). One successful
/// exec here proves the window has closed for every later one.
#[cfg(unix)]
fn script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.join(name);
    std::fs::write(&path, body).expect("script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    for _ in 0..100 {
        if std::process::Command::new(&path)
            .arg("--version")
            .output()
            .is_ok()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    path
}

#[cfg(unix)]
impl Fake {
    fn new(listing: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("calls.log");
        let state = dir.path().join("state.json");
        std::fs::write(&state, listing).expect("state");
        let backend = script(
            dir.path(),
            "gob",
            &format!(
                "#!/bin/sh\n\
                 case \"$1\" in\n\
                   --version) echo 'gob version 0.0.0'; exit 0;;\n\
                 esac\n\
                 echo \"$@\" >> '{}'\n\
                 case \"$1\" in\n\
                   list) cat '{}'; exit 0;;\n\
                   add) echo 'Added job new running: bash -c x'; exit 0;;\n\
                   stop) echo 'Stopped '$2; exit 0;;\n\
                   *) exit 0;;\n\
                 esac\n",
                log.display(),
                state.display()
            ),
        );
        Self {
            dir,
            backend,
            log,
            state,
        }
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn set_listing(&self, listing: &str) {
        std::fs::write(&self.state, listing).expect("state");
    }
}

/// `run_cli`, retried while the freshly written script is still
/// `ETXTBSY`: a test running in parallel can fork while this one holds an
/// open write descriptor on the script, and the child's inherited copy
/// keeps the inode unexecutable until it execs. The window is a few
/// milliseconds; the retry closes it without serializing the suite.
#[cfg(unix)]
fn call(fake: &Fake, argv: &[OsString], session: Option<&str>) -> i32 {
    let mut code = EXIT_NO_BACKEND;
    for _ in 0..50 {
        code = run_cli(argv, &fake.backend, fake.dir.path(), session);
        if code != EXIT_NO_BACKEND {
            return code;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    code
}

/// [`owned_rows`], retried for the same reason [`call`] retries.
#[cfg(unix)]
fn rows(fake: &Fake, scope: Scope, session: Option<&str>) -> Vec<JobRow> {
    for _ in 0..50 {
        if let Ok(rows) = owned_rows(&fake.backend, fake.dir.path(), scope, session) {
            return rows;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    owned_rows(&fake.backend, fake.dir.path(), scope, session).expect("rows")
}

#[cfg(unix)]
fn listing(entries: &str) -> String {
    format!("[{entries}]")
}

#[cfg(unix)]
fn entry(id: &str, status: &str, description: &str, exit_code: Option<i32>) -> String {
    entry_in(id, status, description, exit_code, None)
}

#[cfg(unix)]
fn entry_in(
    id: &str,
    status: &str,
    description: &str,
    exit_code: Option<i32>,
    workdir: Option<&str>,
) -> String {
    let exit = exit_code.map_or_else(|| "null".to_owned(), |code| code.to_string());
    let workdir = workdir.map_or_else(|| "null".to_owned(), |dir| format!("\"{dir}\""));
    format!(
        "{{\"id\":\"{id}\",\"status\":\"{status}\",\"command\":[\"bash\",\"-c\",\"npm run dev\"],\
         \"description\":\"{description}\",\"exit_code\":{exit},\
         \"workdir\":{workdir},\
         \"started_at\":\"2026-11-11T11:20:38+01:00\"}}"
    )
}

#[cfg(unix)]
#[test]
fn a_second_session_cannot_take_over_a_running_line() {
    let fake = Fake::new(&listing(&entry("abc", "running", "[s1] npm run dev", None)));
    let argv = vec![
        OsString::from("job"),
        OsString::from("run"),
        OsString::from("npm run dev"),
    ];
    let code = call(&fake, &argv, Some("s2"));
    assert_eq!(code, 0);
    // Refused: the backend never saw an `add`, so its dedupe cannot
    // rewrite the owner's description.
    assert!(
        !fake.calls().contains("add"),
        "the shim added a second job: {}",
        fake.calls()
    );
}

#[cfg(unix)]
#[test]
fn the_same_session_asking_again_is_told_the_id() {
    let fake = Fake::new(&listing(&entry("abc", "running", "[s1] npm run dev", None)));
    let argv = vec![
        OsString::from("job"),
        OsString::from("run"),
        OsString::from("npm run dev"),
    ];
    let code = call(&fake, &argv, Some("s1"));
    assert_eq!(code, 0);
    assert!(!fake.calls().contains("add"), "{}", fake.calls());
}

#[cfg(unix)]
#[test]
fn a_free_line_starts_through_the_shell_wrapper() {
    let fake = Fake::new(&listing(""));
    let argv = vec![
        OsString::from("job"),
        OsString::from("run"),
        OsString::from("npm run dev"),
    ];
    let code = call(&fake, &argv, Some("s7"));
    assert_eq!(code, 0);
    let calls = fake.calls();
    assert!(
        calls.contains("add --description [s7] npm run dev -- bash -c npm run dev"),
        "the job did not start wrapped and tagged: {calls}"
    );
}

#[cfg(unix)]
#[test]
fn a_stopped_job_does_not_block_a_new_one() {
    let fake = Fake::new(&listing(&entry(
        "abc",
        "stopped",
        "[s1] npm run dev",
        Some(0),
    )));
    let argv = vec![
        OsString::from("job"),
        OsString::from("run"),
        OsString::from("npm run dev"),
    ];
    assert_eq!(call(&fake, &argv, Some("s1")), 0);
    assert!(fake.calls().contains("add"), "{}", fake.calls());
}

#[cfg(unix)]
#[test]
fn the_listing_is_the_sessions_own_unless_asked_for_all() {
    let fake = Fake::new(&listing(&format!(
        "{},{}",
        entry("abc", "running", "[s1] npm run dev", None),
        entry("def", "stopped", "[s2] cargo build", Some(0)),
    )));
    let mine = rows(&fake, Scope::Session, Some("s1"));
    assert_eq!(
        mine.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["abc"]
    );
    let all = rows(&fake, Scope::Machine, Some("s1"));
    assert_eq!(
        all.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["abc", "def"]
    );
}

#[cfg(unix)]
#[test]
fn a_job_started_outside_the_shim_is_nobodys() {
    let fake = Fake::new(&listing(&entry("abc", "running", "hand started", None)));
    let mine = rows(&fake, Scope::Session, Some("s1"));
    assert!(mine.is_empty());
    let all = rows(&fake, Scope::Machine, Some("s1"));
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].session, None);
    assert_eq!(all[0].command, "npm run dev");
}

#[cfg(unix)]
#[test]
fn shutdown_stops_only_this_sessions_running_jobs() {
    let fake = Fake::new(&listing(&format!(
        "{},{},{}",
        entry("abc", "running", "[s1] npm run dev", None),
        entry("def", "running", "[s2] cargo build", None),
        entry("ghi", "stopped", "[s1] cargo test", Some(0)),
    )));
    let code = shutdown_command(&fake.backend, fake.dir.path(), Some("s1"));
    assert_eq!(code, 0);
    let calls = fake.calls();
    assert!(calls.contains("stop abc"), "{calls}");
    assert!(!calls.contains("stop def"), "{calls}");
    assert!(!calls.contains("stop ghi"), "{calls}");
}

#[cfg(unix)]
#[test]
fn a_stop_records_the_id_for_its_session() {
    let fake = Fake::new(&listing(&entry(
        "abc",
        "running",
        "[sr1] npm run dev",
        None,
    )));
    let argv = vec![
        OsString::from("job"),
        OsString::from("stop"),
        OsString::from("abc"),
    ];
    assert_eq!(call(&fake, &argv, Some("sr1")), 0);
    let recorded = stopped_ids_in(&stopped_record_path("sr1"));
    assert!(recorded.contains("abc"), "the stop left no record");
    forget_stopped_in(&stopped_record_path("sr1"));
}

#[cfg(unix)]
#[test]
fn a_stop_the_backend_refused_is_not_recorded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("state.json");
    std::fs::write(&state, "[]").expect("state");
    let backend = script(
        dir.path(),
        "gob",
        &format!(
            "#!/bin/sh\ncase \"$1\" in\n  list) cat '{}'; exit 0;;\n  stop) exit 1;;\n  *) exit 0;;\nesac\n",
            state.display()
        ),
    );
    let argv = vec![
        OsString::from("job"),
        OsString::from("stop"),
        OsString::from("abc"),
    ];
    assert_eq!(call_with(&backend, dir.path(), &argv, Some("sr2")), 1);
    assert!(
        stopped_ids_in(&stopped_record_path("sr2")).is_empty(),
        "a refused stop recorded nothing"
    );
    forget_stopped_in(&stopped_record_path("sr2"));
}

#[cfg(unix)]
#[test]
fn shutdown_records_the_jobs_it_stopped() {
    let fake = Fake::new(&listing(&format!(
        "{},{}",
        entry("abc", "running", "[sr3] npm run dev", None),
        entry("def", "running", "[s2] cargo build", None),
    )));
    assert_eq!(
        shutdown_command(&fake.backend, fake.dir.path(), Some("sr3")),
        0
    );
    let recorded = stopped_ids_in(&stopped_record_path("sr3"));
    assert!(recorded.contains("abc"), "the shutdown left no record");
    assert!(
        !recorded.contains("def"),
        "another session's job recorded against this one"
    );
    forget_stopped_in(&stopped_record_path("sr3"));
}

#[cfg(unix)]
#[test]
fn an_absent_record_reads_as_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(
        stopped_ids_in(&dir.path().join("absent")).is_empty(),
        "no file, no ids"
    );
}

#[cfg(unix)]
fn call_with(backend: &Path, cwd: &Path, argv: &[OsString], session: Option<&str>) -> i32 {
    let mut code = EXIT_NO_BACKEND;
    for _ in 0..50 {
        code = run_cli(argv, backend, cwd, session);
        if code != EXIT_NO_BACKEND {
            return code;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    code
}

#[cfg(unix)]
#[test]
fn a_missing_session_is_not_a_wired_shell() {
    let fake = Fake::new(&listing(""));
    let argv = vec![
        OsString::from("job"),
        OsString::from("run"),
        OsString::from("npm run dev"),
    ];
    assert_eq!(call(&fake, &argv, None), EXIT_NOT_WIRED);
    assert!(fake.calls().is_empty(), "{}", fake.calls());
}

#[cfg(unix)]
#[test]
fn an_unparsed_start_line_falls_back_to_the_tagged_listing() {
    let fake = Fake::new(&listing(&entry("xyz", "running", "[s1] npm run dev", None)));
    assert_eq!(
        find_by_description(&fake.backend, fake.dir.path(), "[s1] npm run dev"),
        Some("xyz".to_owned())
    );
    assert_eq!(
        find_by_description(&fake.backend, fake.dir.path(), "[s1] other"),
        None
    );
}

#[cfg(unix)]
fn fake_bin(dir: &std::path::Path) -> PathBuf {
    script(dir, "rhema-fake", "#!/bin/sh\necho \"$@\"\n")
}

#[cfg(unix)]
#[test]
fn the_init_file_defines_the_function_in_a_noninteractive_shell() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = fake_bin(dir.path());
    let wiring = provision_with_bin(None, &bin, "s9").expect("wiring");
    let out = std::process::Command::new("bash")
        .env(VAR_BASH_ENV, &wiring.shell_env[VAR_BASH_ENV])
        .env(VAR_BIN, &wiring.shell_env[VAR_BIN])
        .env(VAR_SESSION, &wiring.shell_env[VAR_SESSION])
        .arg("-c")
        .arg("job run 'echo hi'")
        .output()
        .expect("bash");
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    // The fake binary prints its argv, so this is the function's forward
    // and nothing more.
    assert!(text.contains("__jobs run echo hi"), "{text}");
}

#[cfg(unix)]
#[test]
fn a_predecessor_init_still_defines_its_own_function() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = fake_bin(dir.path());
    let previous = dir.path().join("previous.bash");
    std::fs::write(&previous, "previous_macro() { echo previous; }\n").expect("previous");
    let wiring = provision_with_bin(Some(previous.into_os_string()), &bin, "s9").expect("wiring");
    let out = std::process::Command::new("bash")
        .env(VAR_BASH_ENV, &wiring.shell_env[VAR_BASH_ENV])
        .env(VAR_BIN, &wiring.shell_env[VAR_BIN])
        .arg("-c")
        .arg("previous_macro && job ls")
        .output()
        .expect("bash");
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("previous"), "{text}");
    assert!(text.contains("__jobs ls"), "{text}");
}

#[cfg(unix)]
#[test]
fn a_job_started_elsewhere_in_the_session_still_belongs_to_it() {
    // The tag is the ownership record, not the directory: a server a
    // `cd` started belongs to the strip that started it.
    let fake = Fake::new("[]");
    let elsewhere = fake.dir.path().join("sub");
    std::fs::create_dir_all(&elsewhere).expect("sub");
    fake.set_listing(&listing(&entry_in(
        "abc",
        "running",
        "[s1] npm run dev",
        None,
        Some(elsewhere.to_str().expect("utf8 path")),
    )));
    let mine = rows(&fake, Scope::Session, Some("s1"));
    assert_eq!(
        mine.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["abc"]
    );
}

#[cfg(unix)]
#[test]
fn the_same_line_in_another_directory_is_a_different_job() {
    // The backend dedupes by argv AND working directory, so a session
    // starting the same command in its own directory must not be told
    // about a job running somewhere else.
    let fake = Fake::new("[]");
    fake.set_listing(&listing(&entry_in(
        "abc",
        "running",
        "[s1] npm run dev",
        None,
        Some("/somewhere/else"),
    )));
    let argv = vec![
        OsString::from("job"),
        OsString::from("run"),
        OsString::from("npm run dev"),
    ];
    assert_eq!(call(&fake, &argv, Some("s2")), 0);
    let calls = fake.calls();
    assert!(calls.contains("add"), "the run was refused: {calls}");
}

#[cfg(unix)]
#[test]
fn the_init_file_is_exactly_the_forward_and_its_guard() {
    // The manual end-to-end runs reproduce this text byte for byte, and
    // the function is the model's whole interface: pin both.
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = fake_bin(dir.path());
    let wiring = provision_with_bin(None, &bin, "s9").expect("wiring");
    let text = std::fs::read_to_string(&wiring.init_path).expect("init");
    assert_eq!(
        text,
        format!(
            "# generated by rhema-agent-tools; do not edit\n\
             {NAME}() {{ \"${VAR_BIN}\" {MODE} \"$@\"; }}\n"
        )
    );

    let previous = dir.path().join("previous.bash");
    std::fs::write(&previous, "").expect("previous");
    let chained =
        provision_with_bin(Some(previous.clone().into_os_string()), &bin, "s9").expect("wiring");
    let text = std::fs::read_to_string(&chained.init_path).expect("init");
    assert_eq!(
        text,
        format!(
            "# generated by rhema-agent-tools; do not edit\n\
             source '{}' || true\n\
             {NAME}() {{ \"${VAR_BIN}\" {MODE} \"$@\"; }}\n",
            previous.display()
        )
    );
}

#[cfg(unix)]
#[test]
fn an_absent_backend_is_not_a_fault() {
    // Nobody installing the backend gets a session without `job` and no
    // warning: the feature was never asked for.
    assert!(matches!(
        probe(Path::new("/nonexistent/gob")),
        Installed::Absent
    ));
}

#[cfg(unix)]
#[test]
fn a_backend_without_the_flag_is_installed_but_unusable() {
    // gob 1.2.3, which `go install ...@latest` fetches: it runs, it
    // starts jobs, and it cannot carry the owner tag.
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = script(
        dir.path(),
        "gob",
        "#!/bin/sh\necho 'Usage: gob add <command> [flags]'\necho '  -f, --follow   Follow output'\n",
    );
    match probe(&bin) {
        Installed::TooOld(reason) => {
            assert!(reason.contains("--description"), "{reason}");
            assert!(reason.contains("Homebrew"), "{reason}");
        }
        Installed::Absent | Installed::Ready => panic!("an old backend did not read as old"),
    }
}

#[cfg(unix)]
#[test]
fn a_backend_with_the_flag_is_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = script(
        dir.path(),
        "gob",
        "#!/bin/sh\necho 'Usage: gob add <command> [flags]'\necho '      --description string   A note'\n",
    );
    assert!(matches!(probe(&bin), Installed::Ready));
}

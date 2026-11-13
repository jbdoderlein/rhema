//! Opening a URL in the user's default browser: the login flow's
//! primary button. Best effort and detached; the launcher's streams are
//! null, so a browser that prints cannot scribble over the TUI.

use std::process::{Command, Stdio};

/// Opens `url` in the default browser, returning whether a launcher
/// process started. The child is detached from the TUI's terminal.
pub fn browser(url: &str) -> bool {
    launchers(url).iter().any(|(program, args)| {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    })
}

/// The launchers to try, in order. Linux follows the `$BROWSER`
/// convention first, then the desktop's opener, then GNOME's.
fn launchers(url: &str) -> Vec<(String, Vec<String>)> {
    #[cfg(target_os = "macos")]
    let commands = vec![("open".to_owned(), vec![url.to_owned()])];
    #[cfg(target_os = "windows")]
    // `rundll32` takes the URL as a plain argument, so shell
    // metacharacters in a query string never reach `cmd`.
    let commands = vec![(
        "rundll32.exe".to_owned(),
        vec!["url.dll,FileProtocolHandler".to_owned(), url.to_owned()],
    )];
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let commands = {
        let mut commands = vec![
            ("xdg-open".to_owned(), vec![url.to_owned()]),
            ("gio".to_owned(), vec!["open".to_owned(), url.to_owned()]),
        ];
        if let Ok(browser) = std::env::var("BROWSER")
            && !browser.trim().is_empty()
        {
            commands.insert(0, (browser, vec![url.to_owned()]));
        }
        commands
    };
    commands
}

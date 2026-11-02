//! Crash reporting (Sentry), gated by `Settings::telemetry`.

use std::sync::atomic::{AtomicBool, Ordering};

use sentry::protocol::Event;

const DSN: &str = "https://76c2e1e6697b4ce02d03366b6275008b@o4512363074682880.ingest.de.sentry.io/4512363096047696";

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Starts the Sentry client. Keep the guard alive for the whole of `main`:
/// dropping it flushes queued events.
pub fn init(enabled: bool) -> sentry::ClientInitGuard {
    set_enabled(enabled);
    let home = home_dir();
    let options = sentry::ClientOptions::new()
        .release(concat!("gitwarp@", env!("CARGO_PKG_VERSION")))
        .send_default_pii(false)
        .before_breadcrumb(|_| None)
        .before_send(move |event| before_send(event, home.as_deref()));
    sentry::init((DSN, options))
}

/// Applies the Settings switch to the running client.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// The client is always initialised and this gate drops every event while the
/// switch is off; that is why toggling it works without a restart.
fn before_send(event: Event<'static>, home: Option<&str>) -> Option<Event<'static>> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    Some(scrub(event, home))
}

fn home_dir() -> Option<String> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .map(|value| value.to_string_lossy().into_owned())
        .filter(|value| !value.is_empty())
}

/// Removes the host name and user, and replaces the home directory with `~`
/// in everything that can carry a path (panic messages, file names).
fn scrub(mut event: Event<'static>, home: Option<&str>) -> Event<'static> {
    // The `contexts` integration fills `server_name` with the host name on its
    // own, so it has to be cleared here rather than only left unset in the options.
    event.server_name = None;
    event.user = None;
    let Some(home) = home else {
        return event;
    };
    let replace = |text: &mut String| {
        if text.contains(home) {
            *text = text.replace(home, "~");
        }
    };
    if let Some(message) = event.message.as_mut() {
        replace(message);
    }
    if let Some(entry) = event.logentry.as_mut() {
        replace(&mut entry.message);
    }
    for exception in &mut event.exception.values {
        if let Some(value) = exception.value.as_mut() {
            replace(value);
        }
        // Panic backtraces carry absolute source paths, too.
        if let Some(trace) = exception.stacktrace.as_mut() {
            for frame in &mut trace.frames {
                if let Some(path) = frame.abs_path.as_mut() {
                    replace(path);
                }
                if let Some(name) = frame.filename.as_mut() {
                    replace(name);
                }
            }
        }
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentry::protocol::{Exception, LogEntry, User};

    fn sample() -> Event<'static> {
        Event {
            message: Some("failed at /home/ann/repo/a.rs".into()),
            logentry: Some(LogEntry {
                message: "open /home/ann/x".into(),
                params: Vec::new(),
            }),
            server_name: Some("ann-laptop".into()),
            user: Some(User {
                username: Some("ann".into()),
                ..Default::default()
            }),
            exception: vec![Exception {
                ty: "panic".into(),
                value: Some("panicked at /home/ann/src/main.rs:1".into()),
                ..Default::default()
            }]
            .into(),
            ..Default::default()
        }
    }

    #[test]
    fn scrub_replaces_home_and_clears_identity() {
        let event = scrub(sample(), Some("/home/ann"));
        assert_eq!(event.message.as_deref(), Some("failed at ~/repo/a.rs"));
        assert_eq!(event.logentry.unwrap().message, "open ~/x");
        assert_eq!(
            event.exception.values[0].value.as_deref(),
            Some("panicked at ~/src/main.rs:1")
        );
        assert!(event.server_name.is_none());
        assert!(event.user.is_none());
    }

    #[test]
    fn scrub_without_home_still_clears_identity() {
        let event = scrub(sample(), None);
        assert_eq!(
            event.message.as_deref(),
            Some("failed at /home/ann/repo/a.rs")
        );
        assert!(event.server_name.is_none());
        assert!(event.user.is_none());
    }

    #[test]
    fn before_send_follows_the_switch() {
        set_enabled(false);
        assert!(before_send(sample(), None).is_none());
        set_enabled(true);
        assert!(before_send(sample(), None).is_some());
        set_enabled(false);
    }
}

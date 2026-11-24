//! The portal hotkey backend against a mock `org.freedesktop.portal.Desktop` (GlobalShortcuts and
//! the host app registry) on the container's private D-Bus session bus. Runs only with
//! `SOUNDBOARS_REQUIRE_PIPEWIRE=1` (`scripts/test-headless.sh`), so it never claims the portal's
//! name on the desktop's bus.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use soundboars::app::APP_ID;
use soundboars::hotkeys::portal::PortalBackend;
use soundboars::hotkeys::{Binding, EventSink, HotkeyBackend, HotkeyEvent};
use zbus::message::Header;
use zbus::{Connection, interface};
use zbus::{ObjectPath, OwnedObjectPath, OwnedVariant, Variant};

const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";

/// A shortcut as the mock received it: id, description, preferred trigger.
type Shortcut = (String, String, Option<String>);

/// What the mock portal saw.
#[derive(Debug, Default)]
struct Seen {
    app_ids: Vec<String>,
    sessions: Vec<String>,
    binds: Vec<(String, Vec<Shortcut>)>,
    closed: Vec<String>,
}

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Seen>>);

impl Shared {
    fn with<R>(&self, f: impl FnOnce(&mut Seen) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

struct Registry(Shared);

#[interface(name = "org.freedesktop.host.portal.Registry")]
impl Registry {
    #[zbus(property)]
    fn version(&self) -> u32 {
        1
    }

    fn register(&self, app_id: String, _options: HashMap<String, OwnedVariant>) {
        self.0.with(|seen| seen.app_ids.push(app_id));
    }
}

struct GlobalShortcuts(Shared);

#[interface(name = "org.freedesktop.portal.GlobalShortcuts")]
impl GlobalShortcuts {
    #[zbus(property)]
    fn version(&self) -> u32 {
        1
    }

    async fn create_session(
        &self,
        options: HashMap<String, OwnedVariant>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let sender = sender_segment(&header);
        let session = format!(
            "{DESKTOP_PATH}/session/{sender}/{}",
            option(&options, "session_handle_token")
        );
        connection
            .object_server()
            .at(session.as_str(), Session(self.0.clone(), session.clone()))
            .await?;
        self.0.with(|seen| seen.sessions.push(session.clone()));
        let results = HashMap::from([("session_handle", Variant::from(session))]);
        respond(connection, &header, &options, results).await
    }

    async fn bind_shortcuts(
        &self,
        session: OwnedObjectPath,
        shortcuts: Vec<(String, HashMap<String, OwnedVariant>)>,
        _parent_window: String,
        options: HashMap<String, OwnedVariant>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let received: Vec<Shortcut> = shortcuts
            .iter()
            .map(|(id, info)| {
                let trigger = info
                    .get("preferred_trigger")
                    .map(|_| option(info, "preferred_trigger"));
                (id.clone(), option(info, "description"), trigger)
            })
            .collect();
        self.0
            .with(|seen| seen.binds.push((session.to_string(), received.clone())));
        let bound: Vec<(String, HashMap<String, Variant>)> = received
            .into_iter()
            .map(|(id, description, trigger)| {
                let info = HashMap::from([
                    ("description".to_owned(), Variant::from(description)),
                    (
                        "trigger_description".to_owned(),
                        Variant::from(trigger.unwrap_or_default()),
                    ),
                ]);
                (id, info)
            })
            .collect();
        let results = HashMap::from([("shortcuts", Variant::from(bound))]);
        respond(connection, &header, &options, results).await
    }
}

struct Session(Shared, String);

#[interface(name = "org.freedesktop.portal.Session")]
impl Session {
    fn close(&self) {
        self.0.with(|seen| seen.closed.push(self.1.clone()));
    }
}

/// The caller's unique name as used in request and session paths: `:1.42` → `1_42`.
fn sender_segment(header: &Header<'_>) -> String {
    let sender = header
        .sender()
        .map(|name| name.to_string())
        .unwrap_or_default();
    sender.trim_start_matches(':').replace('.', "_")
}

fn option(options: &HashMap<String, OwnedVariant>, key: &str) -> String {
    options
        .get(key)
        .and_then(|value| <&str>::try_from(&**value).ok())
        .unwrap_or_default()
        .to_owned()
}

/// Answers a portal request: emits `Response` with `results` on the request object the caller
/// waits on, and returns its path.
async fn respond(
    connection: &Connection,
    header: &Header<'_>,
    options: &HashMap<String, OwnedVariant>,
    results: HashMap<&str, Variant<'_>>,
) -> zbus::fdo::Result<OwnedObjectPath> {
    let request = format!(
        "{DESKTOP_PATH}/request/{}/{}",
        sender_segment(header),
        option(options, "handle_token")
    );
    connection
        .emit_signal(
            header.sender(),
            request.as_str(),
            "org.freedesktop.portal.Request",
            "Response",
            &(0_u32, results),
        )
        .await?;
    let path =
        ObjectPath::try_from(request).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
    Ok(path.into())
}

fn start_mock_portal(seen: &Shared) -> Connection {
    let builder = zbus::connection::Builder::session()
        .name("org.freedesktop.portal.Desktop")
        .serve_at(DESKTOP_PATH, GlobalShortcuts(seen.clone()))
        .serve_at(DESKTOP_PATH, Registry(seen.clone()));
    zbus::block_on(builder.build()).expect("mock portal on the session bus")
}

fn emit_shortcut_signal(portal: &Connection, signal: &str, session: &str, id: &str) {
    let session = ObjectPath::try_from(session).expect("session path");
    let options = HashMap::<&str, Variant>::new();
    zbus::block_on(portal.emit_signal(
        None::<()>,
        DESKTOP_PATH,
        "org.freedesktop.portal.GlobalShortcuts",
        signal,
        &(session, id, 0_u64, options),
    ))
    .expect("emit signal");
}

fn binding(id: &str, description: &str, hotkey: &str) -> Binding {
    Binding {
        id: id.to_owned(),
        description: description.to_owned(),
        hotkey: Some(hotkey.parse().expect("valid hotkey")),
    }
}

fn next_event(events: &mpsc::Receiver<HotkeyEvent>) -> HotkeyEvent {
    events
        .recv_timeout(common::TIMEOUT)
        .expect("a hotkey event")
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + common::TIMEOUT;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_portal_backend_binds_shortcuts_and_reports_activations() {
    if !common::pipewire_required() {
        return;
    }
    let (sender, events) = mpsc::channel();
    let sink = EventSink::channel(sender);
    let airhorn = binding("play-airhorn", "Play Airhorn", "Ctrl+Shift+F13");
    let stop_all = binding("stop-all", "Stop all sounds", "Ctrl+Pause");

    // Without a portal, registering fails with an error; the backend doesn't panic.
    let mut backend = PortalBackend::start(sink.clone(), "KDE").expect("thread");
    backend.register(std::slice::from_ref(&airhorn));
    match next_event(&events) {
        HotkeyEvent::Error(error) => assert!(error.contains("global shortcuts"), "{error}"),
        event => panic!("expected an error, got {event:?}"),
    }
    drop(backend);

    let seen = Shared::default();
    let portal = start_mock_portal(&seen);
    let mut backend = PortalBackend::start(sink, "KDE").expect("thread");
    backend.register(&[stop_all.clone(), airhorn.clone()]);
    wait_until("the shortcuts to be bound", || {
        seen.with(|seen| seen.binds.len() == 1)
    });

    let (session, shortcuts) = seen.with(|seen| seen.binds[0].clone());
    assert_eq!(seen.with(|seen| seen.app_ids.clone()), [APP_ID]);
    assert_eq!(
        seen.with(|seen| seen.sessions.clone()),
        std::slice::from_ref(&session)
    );
    let shortcut = |id: &str, description: &str, trigger: &str| {
        (
            id.to_owned(),
            description.to_owned(),
            Some(trigger.to_owned()),
        )
    };
    assert_eq!(
        shortcuts,
        [
            shortcut("stop-all", "Stop all sounds", "CTRL+Pause"),
            shortcut("play-airhorn", "Play Airhorn", "CTRL+SHIFT+F13"),
        ]
    );

    emit_shortcut_signal(&portal, "Activated", &session, "play-airhorn");
    assert_eq!(
        next_event(&events),
        HotkeyEvent::Pressed("play-airhorn".into())
    );
    emit_shortcut_signal(&portal, "Deactivated", &session, "play-airhorn");
    assert_eq!(
        next_event(&events),
        HotkeyEvent::Released("play-airhorn".into())
    );

    // New bindings: the old session is closed and a new one binds them (a session binds once).
    let without_hotkey = Binding {
        hotkey: None,
        ..stop_all
    };
    backend.register(std::slice::from_ref(&without_hotkey));
    wait_until("the shortcuts to be bound again", || {
        seen.with(|seen| seen.binds.len() == 2)
    });
    let (new_session, shortcuts) = seen.with(|seen| seen.binds[1].clone());
    assert_ne!(new_session, session);
    assert_eq!(seen.with(|seen| seen.closed.clone()), [session]);
    assert_eq!(
        shortcuts,
        [("stop-all".to_owned(), "Stop all sounds".to_owned(), None)]
    );

    drop(backend);
    wait_until("the session to be closed", || {
        seen.with(|seen| seen.closed.len() == 2)
    });
    assert!(events.try_recv().is_err(), "no stray events");
}

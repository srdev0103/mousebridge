//! Wiring the parts together into something that actually shares input.
//!
//! Everything below this module has been separately built and tested. This is
//! where they are connected: capture feeds the router, the router feeds a
//! connection, the connection feeds injection on the other machine, and the
//! topology decides which machine that is.
//!
//! # Threads
//!
//! Four, and each is where it is for a reason:
//!
//! * **The OS capture thread** — owned by CoreGraphics or the Win32 hook. It only
//!   ever enqueues; see [`crate::routing`].
//! * **The injection thread** — platform injection APIs are not `Sync`, and
//!   posting synthetic events wants a thread of its own so a slow post cannot
//!   stall the network.
//! * **The tokio runtime** — networking and discovery.
//! * **The UI thread** — Tauri's. It only reads snapshots.
//!
//! # What the user has to do
//!
//! Grant permissions, pair once, and arrange the screens. The rest is automatic:
//! discovery finds paired machines and reconnects to them, and crossing a screen
//! edge moves the pointer.

use crate::handoff::{Handoff, HandoffAction};
use crate::peers::{PeerChange, PeerSet, PeerState};
use crate::routing::{Destination, ReceiveState, Router, to_input_event};
use crate::{Core, CoreError};
use mb_discovery::BeaconSocket;
use mb_discovery::beacon::Announcement;
use mb_discovery::registry::{DiscoveredPeer, PeerRegistry, PeerSource};
use mb_input::InputEvent;
use mb_input::capture::InputCapture;
use mb_net::Endpoint;
use mb_net::session::{ClosedReason, Session, SessionEvent, SessionHandle};
use mb_protocol::message::{ControlMessage, ScreenDescriptor};
use mb_protocol::version::VersionRange;
use mb_security::pairing::{PairingOffer, PairingSession, PairingState};
use mb_security::{Identity, TrustStore};
use mb_topology::layout::PlacedScreen;
use mb_topology::{DeviceScreens, ScreenMapping, SwitchingRules};
use mb_types::{DeviceId, DeviceName, GlobalScreenId, LogicalPoint};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// How often to announce this device on the network.
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(3);

/// How often to try reconnecting to a paired peer that is not connected.
const RECONNECT_INTERVAL: Duration = Duration::from_secs(5);

/// Offset from the transport port to the pairing port.
///
/// # Why pairing needs a port of its own
///
/// The transport endpoint authenticates with pinned certificates and refuses
/// anything not already in the trust store — which is exactly right, and which
/// makes it useless for pairing, because a device being paired is by definition
/// not in the trust store yet. Dialling it produced a TLS rejection that
/// surfaced as "connection lost", with nothing to say the connection had been
/// deliberately refused.
///
/// So pairing gets a second endpoint that accepts any certificate. What it
/// permits is narrow: nonce exchange and confirmation, nothing else. The security
/// comes from the user comparing a six-digit code on both screens, not from the
/// transport.
const PAIRING_PORT_OFFSET: u16 = 2;

/// How long a pairing attempt waits for the other side.
const PAIRING_TIMEOUT: Duration = Duration::from_secs(30);

/// A pairing attempt in progress, as the interface sees it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PairingView {
    /// The peer's advertised name.
    pub peer_name: String,
    /// The six-digit code, formatted for reading aloud.
    pub code: String,
    /// Whether this machine's user has confirmed.
    pub confirmed_here: bool,
    /// Whether the peer reported its user confirming.
    pub confirmed_there: bool,
}

/// A machine seen on the network, as the interface sees it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DiscoveredView {
    /// Short fingerprint.
    pub id_short: String,
    /// Advertised name.
    pub name: String,
    /// Where to reach it.
    pub address: String,
    /// True when this device is already paired.
    pub paired: bool,
    /// True when a session is established.
    pub connected: bool,
    /// False when the peer speaks an incompatible protocol version.
    pub compatible: bool,
}

/// What the interface polls.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EngineSnapshot {
    /// Whether capture is running.
    pub capturing: bool,
    /// Why capture is not running, if it is not.
    pub capture_error: Option<String>,
    /// The port this device listens on.
    pub port: u16,
    /// Machines seen on the network.
    pub discovered: Vec<DiscoveredView>,
    /// A pairing attempt in progress.
    pub pairing: Option<PairingView>,
    /// Where input is currently going: `local`, or a device's short id.
    pub input_destination: String,
    /// Recent events, newest first, for the diagnostics pane.
    pub log: Vec<String>,
}

/// Shared engine state.
struct Shared {
    identity: Arc<Identity>,
    trust: Arc<RwLock<TrustStore>>,
    trust_path: std::path::PathBuf,
    local_name: DeviceName,
    port: u16,
    peers: Mutex<PeerSet>,
    discovered: Mutex<PeerRegistry>,
    pairing: Mutex<Option<PairingSession>>,
    pairing_handle: Mutex<Option<SessionHandle>>,
    router: Router,
    handoff: Mutex<Option<Handoff>>,
    capturing: AtomicBool,
    capture_error: Mutex<Option<String>>,
    /// This machine's primary display in its own pixel coordinates, for turning
    /// an incoming fraction back into a place the pointer can go.
    local_native_bounds: Mutex<Option<mb_types::LogicalRect>>,
    inject: mpsc::UnboundedSender<InputEvent>,
    log: Mutex<Vec<String>>,
}

impl Shared {
    /// Turns a fraction of this machine's screen into one of its own pixels.
    ///
    /// Uses the *native* display bounds, not the shared-space ones: injection
    /// speaks the platform's own coordinate system, and converting through the
    /// shared units would reintroduce the rounding the native space exists to
    /// avoid.
    fn to_local_pixels(&self, normalized: (f64, f64)) -> Option<(f64, f64)> {
        let bounds = (*self.local_native_bounds.lock().ok()?)?;
        Some((
            bounds.min_x() + normalized.0.clamp(0.0, 1.0) * bounds.size.width,
            bounds.min_y() + normalized.1.clamp(0.0, 1.0) * bounds.size.height,
        ))
    }

    fn note(&self, message: impl Into<String>) {
        let message = message.into();
        info!(target: "mousebridge::engine", "{message}");
        if let Ok(mut log) = self.log.lock() {
            log.insert(0, message);
            log.truncate(40);
        }
    }
}

/// The running system.
pub struct Engine {
    shared: Arc<Shared>,
    runtime: tokio::runtime::Runtime,
    capture: Mutex<Option<Box<dyn InputCapture>>>,
}

impl Engine {
    /// Starts everything.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError`] if the identity or the network endpoint could not be
    /// prepared. Capture failing is *not* an error: the most common cause is a
    /// permission the user has not granted yet, and the application must keep
    /// running so it can tell them that.
    pub fn start(core: &Core) -> Result<Self, CoreError> {
        let config = core.config()?;
        let directory = core.config_directory().to_path_buf();

        let identity = Arc::new(
            mb_security::identity::load_or_create(&directory.join("identity.key"))
                .map_err(|e| CoreError::Engine(e.to_string()))?,
        );
        let trust_path = directory.join("trusted.toml");
        let trust = Arc::new(RwLock::new(
            TrustStore::load(&trust_path).map_err(|e| CoreError::Engine(e.to_string()))?,
        ));

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| CoreError::Engine(e.to_string()))?;

        let (router, queue) = Router::new();
        let (inject_tx, inject_rx) = mpsc::unbounded_channel();

        let local_device = identity.device_id();
        let shared = Arc::new(Shared {
            identity: Arc::clone(&identity),
            trust: Arc::clone(&trust),
            trust_path,
            local_name: config.device.name.clone(),
            port: config.network.port,
            peers: Mutex::new(PeerSet::new(local_device, Vec::new())),
            discovered: Mutex::new(PeerRegistry::default()),
            pairing: Mutex::new(None),
            pairing_handle: Mutex::new(None),
            router: router.clone(),
            handoff: Mutex::new(None),
            capturing: AtomicBool::new(false),
            capture_error: Mutex::new(None),
            local_native_bounds: Mutex::new(None),
            inject: inject_tx,
            log: Mutex::new(Vec::new()),
        });

        // The configuration carries a device id generated on first run, before
        // there was a key to derive one from. The engine's identity is the
        // certificate fingerprint, and that is the one peers see. Left
        // unreconciled, the dashboard showed one value while the log and the
        // pairing prompt showed another — the user has no way to tell which is
        // "their" device id, and neither did anyone reading a bug report.
        if config.device.id != local_device
            && let Err(e) = core.update_config(|config| config.device.id = local_device)
        {
            warn!(error = %e, "could not record the device identity in the configuration");
        }

        spawn_injector(inject_rx);
        refresh_local_screens(&shared, core);

        // Binding a QUIC endpoint registers with the async reactor, so it has to
        // happen inside the runtime. Doing it outside fails with "no async
        // runtime found" — at start-up, before anything is listening, which
        // presents as the application simply never finding any peers.
        let endpoint = {
            let _guard = runtime.enter();
            Endpoint::bind(
                SocketAddr::from(([0, 0, 0, 0], config.network.port)),
                Arc::clone(&identity),
                config.device.name.clone(),
                Arc::clone(&trust) as mb_security::pinning::SharedTrust,
            )
            .map_err(|e| CoreError::Engine(e.to_string()))?
        };

        shared.note(format!(
            "listening on port {} as {}",
            config.network.port,
            local_device.short()
        ));

        // A second endpoint, accepting any certificate, for pairing only.
        let pairing_endpoint = {
            let _guard = runtime.enter();
            Endpoint::bind_for_pairing(
                SocketAddr::from((
                    [0, 0, 0, 0],
                    config.network.port.saturating_add(PAIRING_PORT_OFFSET),
                )),
                Arc::clone(&identity),
                config.device.name.clone(),
            )
            .map_err(|e| CoreError::Engine(e.to_string()))?
        };

        runtime.spawn(accept_loop(Arc::clone(&shared), endpoint.clone()));
        runtime.spawn(pairing_accept_loop(Arc::clone(&shared), pairing_endpoint));
        runtime.spawn(discovery_loop(Arc::clone(&shared)));
        spawn_mdns(Arc::clone(&shared));
        runtime.spawn(reconnect_loop(Arc::clone(&shared), endpoint));
        runtime.spawn(crate::routing::forward_dynamic(
            queue,
            Arc::clone(&shared) as Arc<dyn ActiveSession>,
            router.monitor(),
        ));

        let mut engine = Self {
            shared,
            runtime,
            capture: Mutex::new(None),
        };
        engine.start_capture(core);
        Ok(engine)
    }

    /// Starts input capture, if permissions allow.
    ///
    /// Failure is recorded rather than returned: the usual cause is a permission
    /// the user has not granted, and the dashboard has to stay up to say so.
    pub fn start_capture(&mut self, core: &Core) {
        let missing = core.platform().missing_permissions();
        if !missing.is_empty() {
            let names: Vec<String> = missing.iter().map(ToString::to_string).collect();
            let message = format!("waiting for permission: {}", names.join(" and "));
            self.shared.note(message.clone());
            if let Ok(mut slot) = self.shared.capture_error.lock() {
                *slot = Some(message);
            }
            return;
        }

        let mut capture = match mb_input_native::capture() {
            Ok(capture) => capture,
            Err(e) => {
                self.shared.note(format!("no input backend: {e}"));
                return;
            }
        };

        let sink: Arc<dyn mb_input::capture::InputSink> = Arc::new(EngineSink {
            shared: Arc::clone(&self.shared),
            router: self.shared.router.as_sink(),
        });

        match capture.start(sink) {
            Ok(()) => {
                self.shared.capturing.store(true, Ordering::SeqCst);
                if let Ok(mut slot) = self.shared.capture_error.lock() {
                    *slot = None;
                }
                self.shared.note("input capture started");
                if let Ok(mut slot) = self.capture.lock() {
                    *slot = Some(capture);
                }
            }
            Err(e) => {
                let message = format!("could not start capture: {e}");
                self.shared.note(message.clone());
                if let Ok(mut slot) = self.shared.capture_error.lock() {
                    *slot = Some(message);
                }
            }
        }
    }

    /// Connected peers, in the shape the dashboard renders.
    ///
    /// The dashboard cannot read the peer set directly — it lives in the engine,
    /// which the status snapshot knows nothing about. Without this the interface
    /// reported "there is nothing to share input with" while plainly connected,
    /// and the layout editor had no remote screen to place.
    #[must_use]
    pub fn peer_statuses(&self) -> Vec<crate::status::PeerStatus> {
        let Ok(peers) = self.shared.peers.lock() else {
            return Vec::new();
        };
        let active = peers.active();
        let unreachable = peers
            .layout()
            .ok()
            .and_then(|layout| {
                let local = peers.local();
                layout
                    .screens()
                    .iter()
                    .find(|s| s.id.device == local)
                    .map(|s| peers.unreachable_peers(s.id))
            })
            .unwrap_or_default();

        peers
            .peers()
            .map(|(device, peer)| crate::status::PeerStatus {
                id_short: device.short(),
                name: peer.name.to_string(),
                state: match peer.state {
                    PeerState::Connected => "connected".to_owned(),
                    PeerState::Degraded { .. } => "degraded".to_owned(),
                },
                missed_heartbeats: match peer.state {
                    PeerState::Degraded { missed } => missed,
                    PeerState::Connected => 0,
                },
                active: active == Some(*device),
                unreachable: unreachable.contains(device),
                screen_count: peer.screens.len(),
                latency_ms: None,
            })
            .collect()
    }

    /// What the interface polls.
    #[must_use]
    pub fn snapshot(&self) -> EngineSnapshot {
        let trust = self.shared.trust.read().ok();
        let peers = self.shared.peers.lock().ok();

        let discovered = self
            .shared
            .discovered
            .lock()
            .map(|registry| {
                registry
                    .sorted()
                    .into_iter()
                    .map(|peer| DiscoveredView {
                        id_short: peer.device.short(),
                        name: peer.name.to_string(),
                        address: peer.address.to_string(),
                        paired: trust.as_ref().is_some_and(|t| t.get(peer.device).is_some()),
                        connected: peers.as_ref().is_some_and(|p| p.get(peer.device).is_some()),
                        compatible: true,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let pairing = self.shared.pairing.lock().ok().and_then(|session| {
            let session = session.as_ref()?;
            let code = session.code()?;
            let PairingState::AwaitingConfirmation { local, remote } = session.state() else {
                return None;
            };
            Some(PairingView {
                peer_name: session
                    .peer_name()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                code: code.display(),
                confirmed_here: local,
                confirmed_there: remote,
            })
        });

        let destination = peers
            .as_ref()
            .and_then(|p| p.active())
            .map_or_else(|| "local".to_owned(), |d| d.short());

        EngineSnapshot {
            capturing: self.shared.capturing.load(Ordering::SeqCst),
            capture_error: self
                .shared
                .capture_error
                .lock()
                .ok()
                .and_then(|e| e.clone()),
            port: self.shared.port,
            discovered,
            pairing,
            input_destination: destination,
            log: self
                .shared
                .log
                .lock()
                .map(|l| l.clone())
                .unwrap_or_default(),
        }
    }

    /// Begins pairing with a machine seen on the network.
    ///
    /// # Errors
    ///
    /// Returns a message suitable for display if the address is unknown or the
    /// pairing connection could not be made.
    pub fn begin_pairing(&self, id_short: &str) -> Result<(), String> {
        let target = self
            .shared
            .discovered
            .lock()
            .map_err(|_| "discovery state unavailable".to_owned())?
            .sorted()
            .into_iter()
            .find(|p| p.device.short() == id_short)
            .map(|p| (p.device, p.address, p.name.clone()))
            .ok_or_else(|| "that computer is no longer visible".to_owned())?;

        let shared = Arc::clone(&self.shared);
        self.runtime.spawn(async move {
            if let Err(e) = pair_with(shared.clone(), target.1, target.2).await {
                shared.note(format!("pairing failed: {e}"));
            }
        });
        Ok(())
    }

    /// Records the user confirming the codes match.
    pub fn confirm_pairing(&self) {
        let Ok(mut slot) = self.shared.pairing.lock() else {
            return;
        };
        let Some(session) = slot.as_mut() else {
            return;
        };
        session.confirm_local();

        if let Ok(handle) = self.shared.pairing_handle.lock()
            && let Some(handle) = handle.as_ref()
        {
            let _ = handle.send_control(ControlMessage::PairConfirm);
        }
        self.shared.note("you confirmed the code");
        drop(slot);
        finish_pairing_if_ready(&self.shared);
    }

    /// Records the user rejecting the pairing.
    pub fn reject_pairing(&self) {
        if let Ok(mut slot) = self.shared.pairing.lock() {
            if let Some(session) = slot.as_mut() {
                session.reject();
            }
            *slot = None;
        }
        if let Ok(mut handle) = self.shared.pairing_handle.lock() {
            if let Some(handle) = handle.as_ref() {
                let _ = handle.send_control(ControlMessage::PairReject);
            }
            *handle = None;
        }
        self.shared.note("pairing cancelled");
    }

    /// Forgets a paired device.
    ///
    /// # Errors
    ///
    /// Returns a message suitable for display if the trust store cannot be saved.
    pub fn forget(&self, id_short: &str) -> Result<(), String> {
        let mut trust = self
            .shared
            .trust
            .write()
            .map_err(|_| "trust store unavailable".to_owned())?;

        let Some(device) = trust
            .iter()
            .map(|(device, _)| *device)
            .find(|d| d.short() == id_short)
        else {
            return Err("that device is not paired".to_owned());
        };

        trust.forget(device);
        trust
            .save(&self.shared.trust_path)
            .map_err(|e| e.to_string())?;
        drop(trust);

        self.shared.note("device forgotten");
        Ok(())
    }
}

/// The sink the operating system's capture thread calls.
///
/// Wraps the router with the one decision the router cannot make on its own:
/// *which* machine input belongs to. Pointer movement goes to the topology
/// first, and a crossing switches the destination before the event is queued —
/// so the very movement that leaves this screen is already routed to the machine
/// it arrives on.
///
/// # This runs on the capture thread
///
/// Every lock here is a `try_lock`. A contended one costs a single event its
/// handoff evaluation; a blocked one costs the tap, because both platforms
/// disable capture that takes too long.
struct EngineSink {
    shared: Arc<Shared>,
    router: Arc<dyn mb_input::capture::InputSink>,
}

impl mb_input::capture::InputSink for EngineSink {
    fn on_event(&self, event: &InputEvent) -> mb_input::capture::Disposition {
        if let InputEvent::MouseMove { dx, dy } = event {
            self.shared.consider_crossing(*dx, *dy);
        }
        self.router.on_event(event)
    }
}

impl Shared {
    /// Evaluates a movement against the topology and acts on a crossing.
    fn consider_crossing(&self, dx: f64, dy: f64) {
        let Ok(handoff) = self.handoff.try_lock() else {
            return;
        };
        let Some(handoff) = handoff.as_ref() else {
            return;
        };

        match handoff.movement(dx, dy, Instant::now()) {
            HandoffAction::None => {}
            HandoffAction::Leave { to, screen, entry } => self.hand_over(to, screen, entry),
            HandoffAction::Enter { from, .. } => self.take_back(from),
        }
    }

    /// Sends control to another machine.
    fn hand_over(&self, to: DeviceId, screen: GlobalScreenId, entry: (f64, f64)) {
        let Ok(mut peers) = self.peers.try_lock() else {
            return;
        };
        if peers.set_active(Some(to)).is_err() {
            // The peer went away between the topology saying it was there and
            // this moment. Staying local is the safe outcome: the alternative is
            // typing into a machine that is not listening.
            return;
        }

        let held = self.held_snapshot();
        if let Some(handle) = peers.active_handle() {
            let _ = handle.send_control(ControlMessage::CursorEnter {
                screen: screen.screen,
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "a normalised 0..=1 coordinate loses nothing in f32"
                )]
                position: (entry.0 as f32, entry.1 as f32),
                held,
            });
        }
        drop(peers);

        // Release locally *before* forwarding starts, so nothing is left held on
        // the machine the user is leaving.
        let _ = self.inject.send(InputEvent::ReleaseAll);
        self.router.set_destination(Destination::Remote);
        self.note("pointer moved to another computer");
    }

    /// Brings control back to this machine.
    fn take_back(&self, from: DeviceId) {
        if let Ok(mut peers) = self.peers.try_lock() {
            if let Some(peer) = peers.get(from)
                && let Some(handle) = peer.handle()
            {
                let _ = handle.send_control(ControlMessage::CursorLeave);
            }
            let _ = peers.set_active(None);
        }
        self.router.set_destination(Destination::Local);
        self.note("pointer returned to this computer");
    }

    /// What the user is physically holding, for a handoff.
    ///
    /// Empty for now: the sender does not track held state separately from the
    /// receiver's, so a chord in progress is released rather than carried across.
    /// Carrying it needs a tracker on the capture path, which is a change to the
    /// hot path and is deliberately not made here.
    fn held_snapshot(&self) -> mb_input::HeldInput {
        mb_input::HeldInput::default()
    }
}

/// Lets the forwarding task find whichever session is currently active.
pub trait ActiveSession: Send + Sync {
    /// The session input should go to, if any.
    fn active(&self) -> Option<SessionHandle>;

    /// Where the cursor sits on the screen it is currently on, `0.0..=1.0`.
    fn cursor_normalized(&self) -> Option<(f64, f64)>;
}

impl ActiveSession for Shared {
    fn active(&self) -> Option<SessionHandle> {
        self.peers.lock().ok()?.active_handle().cloned()
    }

    fn cursor_normalized(&self) -> Option<(f64, f64)> {
        self.handoff.lock().ok()?.as_ref()?.cursor_normalized()
    }
}

/// Runs platform injection on a thread of its own.
///
/// Injection APIs are not `Sync`, and posting synthetic events wants its own
/// thread so a slow post cannot stall the network task feeding it.
fn spawn_injector(mut events: mpsc::UnboundedReceiver<InputEvent>) {
    std::thread::Builder::new()
        .name("mousebridge-inject".to_owned())
        .spawn(move || {
            let mut injector = match mb_input_native::inject() {
                Ok(injector) => injector,
                Err(e) => {
                    error!(error = %e, "no injection backend; remote input will do nothing");
                    return;
                }
            };
            let mut state = ReceiveState::new();

            while let Some(event) = events.blocking_recv() {
                // Tracked before injecting, so a disconnect can always release
                // exactly what was pressed.
                state.observe(&event);
                if let Err(e) = injector.inject(&event) {
                    debug!(error = %e, "injection failed");
                }
            }

            // The channel closed: the engine is going away. Let go of everything
            // rather than leaving a key down on this machine.
            for release in state.release_all() {
                let _ = injector.inject(&release);
            }
            let _ = injector.release_all();
        })
        .map(|_| ())
        .unwrap_or_else(|e| error!(error = %e, "could not start the injection thread"));
}

/// Rebuilds this device's screens from the platform layer.
fn refresh_local_screens(shared: &Arc<Shared>, core: &Core) {
    let Ok(displays) = core.platform().displays() else {
        return;
    };

    // Remembered in native units, for mapping incoming motion.
    if let Ok(mut slot) = shared.local_native_bounds.lock() {
        *slot = displays.primary().map(|d| d.bounds);
    }
    let os = mb_types::OsKind::current();
    let local = shared.identity.device_id();

    let screens = DeviceScreens::new(
        displays
            .displays()
            .iter()
            .map(|d| {
                ScreenMapping::from_native(GlobalScreenId::new(local, d.id), d.bounds, d.scale, os)
            })
            .collect(),
    );

    let placed = screens.place_at(LogicalPoint::ZERO);
    if let Ok(mut peers) = shared.peers.lock() {
        peers.set_local_screens(placed);
    }
    rebuild_topology(shared);
}

/// Rebuilds the shared topology from every connected machine.
fn rebuild_topology(shared: &Arc<Shared>) {
    let Ok(peers) = shared.peers.lock() else {
        return;
    };
    let Ok(layout) = peers.layout() else {
        // Overlapping screens, usually. The user has to move something in the
        // editor; carrying on with a stale layout would be worse.
        return;
    };
    let local = peers.local();
    let start = layout
        .screens()
        .iter()
        .find(|s| s.id.device == local)
        .map(|s| s.id);
    drop(peers);

    let Some(start) = start else {
        return;
    };

    let Ok(mut slot) = shared.handoff.lock() else {
        return;
    };
    match slot.as_ref() {
        Some(handoff) => {
            handoff.set_layout(layout);
        }
        None => {
            *slot = Handoff::new(local, layout, start, SwitchingRules::default());
        }
    }
}

/// Describes this device's screens for a peer.
fn local_screen_descriptors(shared: &Arc<Shared>) -> Vec<ScreenDescriptor> {
    let Ok(peers) = shared.peers.lock() else {
        return Vec::new();
    };
    let local = peers.local();
    peers
        .layout()
        .map(|layout| {
            layout
                .screens()
                .iter()
                .filter(|s| s.id.device == local)
                .map(|s| ScreenDescriptor {
                    screen: s.id.screen,
                    width: s.bounds.size.width,
                    height: s.bounds.size.height,
                    x: s.bounds.min_x(),
                    y: s.bounds.min_y(),
                    scale: s.scale.get(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Places a peer's advertised screens to the right of everything already placed.
///
/// A first guess only. The user rearranges them in the layout editor, and that
/// arrangement is what persists.
fn place_peer_screens(
    shared: &Arc<Shared>,
    device: DeviceId,
    screens: &[ScreenDescriptor],
) -> Vec<PlacedScreen> {
    let offset = shared
        .peers
        .lock()
        .ok()
        .and_then(|peers| peers.layout().ok())
        .map_or(0.0, |layout| layout.bounding_box().max_x());

    screens
        .iter()
        .filter_map(|descriptor| {
            let bounds = mb_types::LogicalRect::from_parts(
                offset + descriptor.x,
                descriptor.y,
                descriptor.width,
                descriptor.height,
            )?;
            Some(PlacedScreen {
                id: GlobalScreenId::new(device, descriptor.screen),
                bounds,
                scale: mb_types::Scale::new(descriptor.scale)?,
            })
        })
        .collect()
}

/// Accepts incoming connections from paired peers.
///
/// A peer that is not paired never reaches here: the TLS layer refuses it during
/// the handshake, because its certificate is not in the trust store.
async fn accept_loop(shared: Arc<Shared>, endpoint: Endpoint) {
    while let Some(incoming) = endpoint.accept().await {
        match incoming {
            Ok(peer) => {
                shared.note(format!("{} connected", peer.name()));
                tokio::spawn(run_session(Arc::clone(&shared), peer));
            }
            // Routine: an unpaired machine trying its luck, or a half-open
            // connection. Not worth a line in the user-visible log.
            Err(e) => debug!(error = %e, "rejected an incoming connection"),
        }
    }
}

/// Announces this device and listens for others.
async fn discovery_loop(shared: Arc<Shared>) {
    // Same constraint as the QUIC endpoint: binding registers with the reactor.
    // This runs inside a spawned task, so the runtime is already entered.
    let socket = match BeaconSocket::bind(mb_discovery::BEACON_PORT) {
        Ok(socket) => Arc::new(socket),
        Err(e) => {
            // Another instance, or a locked-down network. Manual addressing
            // still works, so this is a degraded mode rather than a failure.
            shared.note(format!("discovery unavailable: {e}"));
            return;
        }
    };

    let announcement = Announcement {
        device: shared.identity.device_id(),
        name: shared.local_name.clone(),
        os: mb_types::OsKind::current(),
        port: shared.port,
        versions: VersionRange::current(),
    };

    // Listener.
    {
        let shared = Arc::clone(&shared);
        let socket = Arc::clone(&socket);
        let local = shared.identity.device_id();
        tokio::spawn(async move {
            loop {
                match socket.recv().await {
                    Ok((heard, from)) => {
                        // Our own broadcast, reflected back.
                        if heard.device == local {
                            continue;
                        }
                        let address = heard.connect_address(from);
                        if let Ok(mut registry) = shared.discovered.lock()
                            && registry.observe(DiscoveredPeer {
                                device: heard.device,
                                name: heard.name.clone(),
                                os: heard.os,
                                address,
                                source: PeerSource::Broadcast,
                                last_seen: Instant::now(),
                            })
                        {
                            shared.note(format!("found {} at {address}", heard.name));
                        }
                    }
                    Err(e) => {
                        shared.note(format!("discovery stopped: {e}"));
                        return;
                    }
                }
            }
        });
    }

    // Announcer.
    let mut ticker = tokio::time::interval(ANNOUNCE_INTERVAL);
    loop {
        ticker.tick().await;
        if let Err(e) = socket
            .broadcast(&announcement, mb_discovery::BEACON_PORT)
            .await
        {
            debug!(error = %e, "could not broadcast");
        }
        if let Ok(mut registry) = shared.discovered.lock() {
            registry.expire(Instant::now());
        }
    }
}

/// Advertises over mDNS and browses for peers, on a thread of its own.
///
/// The second discovery path, and the reason there are two. Broadcast is
/// filtered on many corporate networks; multicast is filtered by access-point
/// client isolation on many home ones. A machine that is invisible over one is
/// often perfectly visible over the other, and the user has no way to know which
/// their network is doing.
///
/// Runs on a thread because the browse channel is blocking, and blocking a tokio
/// worker for the life of the process would starve the tasks sharing it.
fn spawn_mdns(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("mousebridge-mdns".to_owned())
        .spawn(move || {
            let mut responder = match mb_discovery::MdnsResponder::start() {
                Ok(responder) => responder,
                Err(e) => {
                    // Multicast unavailable. Broadcast and manual addressing
                    // still work, so this is a degraded mode, not a failure.
                    shared.note(format!("mDNS unavailable: {e}"));
                    return;
                }
            };

            let announcement = Announcement {
                device: shared.identity.device_id(),
                name: shared.local_name.clone(),
                os: mb_types::OsKind::current(),
                port: shared.port,
                versions: VersionRange::current(),
            };

            let addresses: Vec<std::net::IpAddr> = if_addrs::get_if_addrs()
                .unwrap_or_default()
                .into_iter()
                .filter(|i| !i.is_loopback())
                .map(|i| i.ip())
                .filter(std::net::IpAddr::is_ipv4)
                .collect();

            if let Err(e) = responder.advertise(&announcement, &addresses) {
                shared.note(format!("could not advertise over mDNS: {e}"));
                return;
            }

            let browse = match responder.browse() {
                Ok(browse) => browse,
                Err(e) => {
                    shared.note(format!("could not browse over mDNS: {e}"));
                    return;
                }
            };
            shared.note("mDNS active");

            let local = shared.identity.device_id();
            loop {
                let Ok(event) = browse.recv() else {
                    return;
                };
                let mdns_sd::ServiceEvent::ServiceResolved(service) = event else {
                    continue;
                };
                let Some((heard, address)) = mb_discovery::MdnsResponder::resolve(&service) else {
                    continue;
                };
                if heard.device == local {
                    continue;
                }

                if let Ok(mut registry) = shared.discovered.lock()
                    && registry.observe(DiscoveredPeer {
                        device: heard.device,
                        name: heard.name.clone(),
                        os: heard.os,
                        address,
                        source: PeerSource::Mdns,
                        last_seen: Instant::now(),
                    })
                {
                    shared.note(format!("found {} at {address} (mDNS)", heard.name));
                }
            }
        })
        .map(|_| ())
        .unwrap_or_else(|e| error!(error = %e, "could not start mDNS"));
}

/// Reconnects to paired machines that are visible but not connected.
///
/// Without this, a peer that restarted or briefly lost the network would stay
/// disconnected until the user noticed and did something about it.
async fn reconnect_loop(shared: Arc<Shared>, endpoint: Endpoint) {
    let mut ticker = tokio::time::interval(RECONNECT_INTERVAL);
    loop {
        ticker.tick().await;

        let candidates: Vec<(DeviceId, SocketAddr)> = {
            let Ok(registry) = shared.discovered.lock() else {
                continue;
            };
            let Ok(trust) = shared.trust.read() else {
                continue;
            };
            let Ok(peers) = shared.peers.lock() else {
                continue;
            };
            registry
                .sorted()
                .into_iter()
                .filter(|p| trust.get(p.device).is_some() && peers.get(p.device).is_none())
                .map(|p| (p.device, p.address))
                .collect()
        };

        for (device, address) in candidates {
            // Only one side should dial, or both connect at once and one of the
            // two sessions is immediately redundant. The lower identity dials.
            if shared.identity.device_id() > device {
                continue;
            }
            match endpoint.connect(address).await {
                Ok(peer) => {
                    shared.note(format!("reconnected to {}", peer.name()));
                    tokio::spawn(run_session(Arc::clone(&shared), peer));
                }
                Err(e) => debug!(%address, error = %e, "reconnect attempt failed"),
            }
        }
    }
}

/// Drives one connected peer for the life of the session.
async fn run_session(shared: Arc<Shared>, peer: mb_net::endpoint::PeerConnection) {
    let device = peer.device();
    let name = peer.name().clone();
    let (handle, mut events, session) = Session::split(peer);
    tokio::spawn(session.run());

    // Tell the peer about our screens, and register it with none of its own
    // until it tells us about them.
    let _ = handle.send_control(ControlMessage::Screens {
        screens: local_screen_descriptors(&shared),
    });

    if let Ok(mut peers) = shared.peers.lock() {
        peers.connect(device, name.clone(), Vec::new(), Some(handle.clone()));
    }

    while let Some(event) = events.recv().await {
        match event {
            SessionEvent::Input(message) => {
                let _ = shared.inject.send(to_input_event(&message));
            }
            SessionEvent::Motion(motion) => {
                // The sender gave a fraction of the screen; only this machine
                // knows what that is in its own pixels.
                if let Some(point) = shared.to_local_pixels(motion.normalized()) {
                    let _ = shared.inject.send(InputEvent::MouseMoveTo {
                        x: point.0,
                        y: point.1,
                    });
                }
            }
            SessionEvent::Control(ControlMessage::Screens { screens }) => {
                let placed = place_peer_screens(&shared, device, &screens);
                if let Ok(mut peers) = shared.peers.lock() {
                    peers.set_screens(device, placed);
                }
                rebuild_topology(&shared);
                shared.note(format!("{name} has {} screen(s)", screens.len()));
            }
            SessionEvent::Control(ControlMessage::CursorEnter {
                screen,
                position,
                held,
            }) => {
                // Control is arriving here. Re-assert what the user is holding,
                // then take over locally.
                if let Ok(mut peers) = shared.peers.lock() {
                    let _ = peers.set_active(None);
                }
                shared.router.set_destination(Destination::Local);
                // The wire carries f32 to keep the message small; the topology
                // works in f64. Widening is exact.
                let position = (f64::from(position.0), f64::from(position.1));
                if let Ok(handoff) = shared.handoff.lock()
                    && let Some(handoff) = handoff.as_ref()
                {
                    handoff.place(
                        GlobalScreenId::new(shared.identity.device_id(), screen),
                        position,
                    );
                }
                let _ = shared.inject.send(InputEvent::Enter {
                    screen,
                    position,
                    held,
                });
                shared.note("control returned to this computer");
            }
            SessionEvent::Control(ControlMessage::CursorLeave) => {
                // Whatever we were holding on the peer's behalf, let go.
                let _ = shared.inject.send(InputEvent::ReleaseAll);
            }
            SessionEvent::Control(ControlMessage::PairConfirm) => {
                if let Ok(mut slot) = shared.pairing.lock()
                    && let Some(session) = slot.as_mut()
                {
                    session.confirm_remote();
                }
                finish_pairing_if_ready(&shared);
            }
            SessionEvent::Control(ControlMessage::PairReject) => {
                if let Ok(mut slot) = shared.pairing.lock() {
                    *slot = None;
                }
                shared.note("the other computer cancelled pairing");
            }
            SessionEvent::Degraded { missed } => {
                if let Ok(mut peers) = shared.peers.lock() {
                    peers.set_state(device, PeerState::Degraded { missed });
                }
            }
            SessionEvent::Recovered => {
                if let Ok(mut peers) = shared.peers.lock() {
                    peers.set_state(device, PeerState::Connected);
                }
            }
            SessionEvent::Closed(reason) => {
                on_session_closed(&shared, device, &name, &reason);
                return;
            }
            SessionEvent::Control(_) => {}
        }
    }
    on_session_closed(&shared, device, &name, &ClosedReason::LocalShutdown);
}

/// Cleans up after a session ends, however it ended.
fn on_session_closed(
    shared: &Arc<Shared>,
    device: DeviceId,
    name: &DeviceName,
    reason: &ClosedReason,
) {
    let change = shared
        .peers
        .lock()
        .map_or(PeerChange::LayoutOnly, |mut peers| peers.disconnect(device));

    if let PeerChange::ReclaimControl { .. } = change {
        // The user was typing into a machine that has gone. Bring input back
        // here immediately, and release whatever we were holding for it.
        shared.router.set_destination(Destination::Local);
        let _ = shared.inject.send(InputEvent::ReleaseAll);
        shared.note(format!(
            "{name} disappeared; control returned to this computer"
        ));
    } else {
        shared.note(format!("{name} disconnected"));
    }

    debug!(?reason, "session ended");
    rebuild_topology(shared);
}

/// Accepts pairing requests from machines that are not yet trusted.
async fn pairing_accept_loop(shared: Arc<Shared>, endpoint: Endpoint) {
    while let Some(incoming) = endpoint.accept().await {
        match incoming {
            Ok(peer) => {
                // One at a time. Two simultaneous attempts would each overwrite
                // the other's code, and the user would be comparing digits that
                // no longer mean anything.
                if shared.pairing.lock().is_ok_and(|slot| slot.is_some()) {
                    shared.note("ignoring a pairing request: one is already in progress");
                    continue;
                }
                shared.note(format!("{} wants to pair", peer.name()));
                let name = peer.name().clone();
                let shared = Arc::clone(&shared);
                tokio::spawn(async move {
                    if let Err(e) = run_pairing(Arc::clone(&shared), peer, name).await {
                        shared.note(format!("pairing failed: {e}"));
                    }
                });
            }
            Err(e) => debug!(error = %e, "rejected a pairing connection"),
        }
    }
}

/// Dials another machine's pairing port and starts the exchange.
async fn pair_with(
    shared: Arc<Shared>,
    address: SocketAddr,
    name: DeviceName,
) -> Result<(), String> {
    let endpoint = Endpoint::bind_for_pairing(
        SocketAddr::from(([0, 0, 0, 0], 0)),
        Arc::clone(&shared.identity),
        shared.local_name.clone(),
    )
    .map_err(|e| e.to_string())?;

    // The pairing port, not the transport port. The transport port would refuse
    // us: we are not in its trust store, which is the whole reason we are here.
    let target = SocketAddr::new(
        address.ip(),
        address.port().saturating_add(PAIRING_PORT_OFFSET),
    );

    let peer = endpoint
        .connect(target)
        .await
        .map_err(|e| format!("could not reach {target} for pairing: {e}"))?;

    run_pairing(shared, peer, name).await
}

/// Exchanges nonces and shows the code. Identical on both sides.
///
/// Symmetric by design: whichever machine dialled, both send a nonce, both
/// compute the same code from the same inputs, and both users must confirm.
async fn run_pairing(
    shared: Arc<Shared>,
    peer: mb_net::endpoint::PeerConnection,
    name: DeviceName,
) -> Result<(), String> {
    // Taken from the authenticated handshake, never from a message: a peer must
    // not be able to show the user a code for a certificate it does not hold.
    let peer_certificate = peer.certificate().as_ref().to_vec();

    let nonce = *mb_security::pairing::generate_nonce()
        .map_err(|e| e.to_string())?
        .expose();

    let (handle, mut events, session) = Session::split(peer);
    tokio::spawn(session.run());

    let mut pairing = PairingSession::new(PairingOffer {
        certificate: shared.identity.certificate().as_ref().to_vec(),
        nonce,
        name: shared.local_name.clone(),
    });

    handle
        .send_control(ControlMessage::PairNonce { nonce })
        .map_err(|e| e.to_string())?;

    if let Ok(mut slot) = shared.pairing_handle.lock() {
        *slot = Some(handle.clone());
    }

    let deadline = tokio::time::Instant::now() + PAIRING_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Ok(Some(event)) = tokio::time::timeout(remaining, events.recv()).await else {
            if let Ok(mut slot) = shared.pairing_handle.lock() {
                *slot = None;
            }
            return Err("the other computer did not respond".to_owned());
        };

        if let SessionEvent::Control(ControlMessage::PairNonce { nonce }) = event {
            pairing
                .accept_peer(PairingOffer {
                    certificate: peer_certificate,
                    nonce,
                    name: name.clone(),
                })
                .map_err(|e| e.to_string())?;
            break;
        }
    }

    let code = pairing.code().map(|c| c.display()).unwrap_or_default();
    if let Ok(mut slot) = shared.pairing.lock() {
        *slot = Some(pairing);
    }
    shared.note(format!(
        "code {code} - check it matches on the other screen"
    ));

    // Stay on the connection until the pairing resolves.
    let shared_for_task = Arc::clone(&shared);
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::Control(ControlMessage::PairConfirm) => {
                    if let Ok(mut slot) = shared_for_task.pairing.lock()
                        && let Some(session) = slot.as_mut()
                    {
                        session.confirm_remote();
                    }
                    finish_pairing_if_ready(&shared_for_task);
                }
                SessionEvent::Control(ControlMessage::PairReject) => {
                    if let Ok(mut slot) = shared_for_task.pairing.lock() {
                        *slot = None;
                    }
                    shared_for_task.note("the other computer cancelled pairing");
                    return;
                }
                SessionEvent::Closed(_) => {
                    // Expected once pairing completes: the trust store is written
                    // and the transport takes over on its own port.
                    if let Ok(mut slot) = shared_for_task.pairing.lock() {
                        *slot = None;
                    }
                    return;
                }
                _ => {}
            }
        }
    });
    Ok(())
}

/// Writes the peer into the trust store, once both users have confirmed.
///
/// The gate: nothing reaches the trust store until [`PairingSession`] says both
/// sides agreed the codes matched.
fn finish_pairing_if_ready(shared: &Arc<Shared>) {
    let Ok(mut slot) = shared.pairing.lock() else {
        return;
    };
    let Some(session) = slot.as_ref() else {
        return;
    };
    let (Some(certificate), Some(name)) =
        (session.confirmed_certificate(), session.confirmed_name())
    else {
        return;
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());

    let saved = shared.trust.write().map(|mut trust| {
        trust.trust(name.clone(), &certificate, now);
        trust.save(&shared.trust_path)
    });

    *slot = None;
    drop(slot);

    if let Ok(mut handle) = shared.pairing_handle.lock() {
        *handle = None;
    }

    match saved {
        Ok(Ok(())) => shared.note(format!("paired with {name}")),
        _ => shared.note("paired, but the trust store could not be saved"),
    }
}

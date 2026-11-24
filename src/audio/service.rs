//! The PipeWire thread. PipeWire objects are `!Send`, so they all live on one thread that runs a
//! `MainLoop`; [`PipeWireService`] is the `Send` handle the rest of the app talks to it through.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::thread::{self, JoinHandle};

use pipewire as pw;
use pw::context::ContextRc;
use pw::core::CoreRc;
use pw::link::Link;
use pw::main_loop::MainLoopRc;
use pw::node::{Node, NodeChangeMask, NodeListener};
use pw::registry::{GlobalObject, RegistryRc};
use pw::spa::utils::dict::DictRef;
use pw::stream::{StreamListener, StreamState};
use pw::types::ObjectType;

use super::AudioError;
use super::graph::{Direction, Graph, NodeProps, PortProps, RECORDING_STREAM_CLASS, RecordingApp};
use super::link_plan::{LinkPair, plan_links};
use super::output::{INJECT, LOCAL, OutputStream, Render};

/// What the PipeWire thread reports back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The apps that currently record audio; sent whenever the list changes.
    RecordingApps(Vec<RecordingApp>),
    /// An asynchronous PipeWire error, e.g. a link that couldn't be created.
    Error(String),
}

enum Command {
    LinkTo(u32),
    Unlink,
    Quit,
}

/// Receives the PipeWire thread's [`Event`]s, on that thread; it must not block.
pub type EventSink = Box<dyn Fn(Event) + Send>;

/// What the output streams play; see [`Render`] for the real-time rules they must follow.
pub struct Renders {
    /// Fills `soundboars-inject`, which goes into the linked app.
    pub inject: Render,
    /// Fills `soundboars-local`, which goes to the default sink.
    pub local: Render,
}

/// Handle to the PipeWire thread. Dropping it unlinks everything and stops the thread.
pub struct PipeWireService {
    commands: pw::channel::Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl PipeWireService {
    /// Connects to PipeWire on a new thread, starts both output streams and reports events to
    /// `events`.
    pub fn start(
        renders: Renders,
        events: impl Fn(Event) + Send + 'static,
    ) -> Result<Self, AudioError> {
        let events: EventSink = Box::new(events);
        let (command_tx, command_rx) = pw::channel::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("pipewire".to_owned())
            .spawn(move || run(renders, command_rx, events, ready_tx))?;
        ready_rx.recv().map_err(|_| AudioError::ThreadGone)??;
        Ok(Self {
            commands: command_tx,
            thread: Some(thread),
        })
    }

    /// Links the inject stream into the recording stream `node_id`, replacing any previous target.
    pub fn link_to(&self, node_id: u32) {
        self.send(Command::LinkTo(node_id));
    }

    pub fn unlink(&self) {
        self.send(Command::Unlink);
    }

    fn send(&self, command: Command) {
        // Fails only if the thread is gone, and then there is nothing left to command.
        let _ = self.commands.send(command);
    }
}

impl Drop for PipeWireService {
    fn drop(&mut self) {
        self.send(Command::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(
    renders: Renders,
    commands: pw::channel::Receiver<Command>,
    events: EventSink,
    ready: std::sync::mpsc::SyncSender<Result<(), AudioError>>,
) {
    pw::init();
    // `ready` fails only if `start` gave up waiting, and then nobody is interested.
    match Session::connect(renders, events) {
        Ok(session) => {
            let _ = ready.send(Ok(()));
            session.run(commands);
        }
        Err(error) => {
            let _ = ready.send(Err(error));
        }
    }
}

/// All PipeWire objects of the thread. Fields drop in order: listeners and proxies before the core.
struct Session {
    _registry_listener: pw::registry::Listener,
    _core_listener: pw::core::Listener,
    _inject_listener: StreamListener<()>,
    _local_listener: StreamListener<()>,
    state: Rc<RefCell<State>>,
    _inject: OutputStream,
    _local: OutputStream,
    _registry: RegistryRc,
    _context: ContextRc,
    main_loop: MainLoopRc,
}

impl Session {
    fn connect(renders: Renders, events: EventSink) -> Result<Self, AudioError> {
        let main_loop = MainLoopRc::new(None)?;
        let context = ContextRc::new(&main_loop, None)?;
        let core = context.connect_rc(None)?;
        let registry = core.get_registry_rc()?;
        let inject = OutputStream::connect(&core, INJECT, renders.inject)?;
        let local = OutputStream::connect(&core, LOCAL, renders.local)?;
        let state = Rc::new(RefCell::new(State::new(core.clone(), events)));
        Ok(Self {
            _registry_listener: watch_registry(&registry, &state),
            _core_listener: report_core_errors(&core, &state),
            _inject_listener: watch_stream(&inject, &state, State::set_inject_node)?,
            _local_listener: watch_stream(&local, &state, |_, _| {})?,
            state,
            _inject: inject,
            _local: local,
            _registry: registry,
            _context: context,
            main_loop,
        })
    }

    fn run(self, commands: pw::channel::Receiver<Command>) {
        let state = Rc::clone(&self.state);
        let main_loop = self.main_loop.clone();
        let _commands = commands.attach(self.main_loop.loop_(), move |command| match command {
            Command::LinkTo(node_id) => state.borrow_mut().set_target(Some(node_id)),
            Command::Unlink => state.borrow_mut().set_target(None),
            Command::Quit => main_loop.quit(),
        });
        self.main_loop.run();
    }
}

fn watch_registry(registry: &RegistryRc, state: &Rc<RefCell<State>>) -> pw::registry::Listener {
    let on_global = {
        let state = Rc::clone(state);
        let registry = registry.clone();
        move |global: &GlobalObject<&DictRef>| on_global(&state, &registry, global)
    };
    let on_remove = {
        let state = Rc::clone(state);
        move |id| state.borrow_mut().remove(id)
    };
    registry
        .add_listener_local()
        .global(on_global)
        .global_remove(on_remove)
        .register()
}

fn on_global(state: &Rc<RefCell<State>>, registry: &RegistryRc, global: &GlobalObject<&DictRef>) {
    let Some(props) = global.props else {
        return;
    };
    match global.type_ {
        ObjectType::Node if props.get("media.class") == Some(RECORDING_STREAM_CLASS) => {
            watch_node(state, registry, global);
        }
        ObjectType::Port => {
            if let Some(port) = PortProps::parse(global.id, |key| props.get(key)) {
                state.borrow_mut().add_port(port);
            }
        }
        _ => {}
    }
}

/// Registry globals only carry a few node properties; the app name, binary and media name come
/// with the node's info, so recording nodes are bound and their info is watched.
fn watch_node(state: &Rc<RefCell<State>>, registry: &RegistryRc, global: &GlobalObject<&DictRef>) {
    let node: Node = match registry.bind(global) {
        Ok(node) => node,
        Err(error) => {
            return state
                .borrow()
                .report(format!("binding node {}: {error}", global.id));
        }
    };
    let weak_state: Weak<RefCell<State>> = Rc::downgrade(state);
    let listener = node
        .add_listener_local()
        .info(move |info| {
            // Info updates carry props only when they changed; otherwise they come empty.
            if !info.change_mask().contains(NodeChangeMask::PROPS) {
                return;
            }
            if let (Some(state), Some(props)) = (weak_state.upgrade(), info.props()) {
                let props = NodeProps::parse(|key| props.get(key));
                state.borrow_mut().set_node(info.id(), props);
            }
        })
        .register();
    state
        .borrow_mut()
        .watched_nodes
        .insert(global.id, (node, listener));
}

fn report_core_errors(core: &CoreRc, state: &Rc<RefCell<State>>) -> pw::core::Listener {
    let state = Rc::downgrade(state);
    core.add_listener_local()
        .error(move |id, _seq, res, message| {
            if let Some(state) = state.upgrade() {
                state
                    .borrow()
                    .report(format!("object {id}: {message} ({res})"));
            }
        })
        .register()
}

/// Reports stream errors, and calls `on_node` with the stream's node id once it's connected (links
/// can only be planned after that).
fn watch_stream(
    output: &OutputStream,
    state: &Rc<RefCell<State>>,
    on_node: impl Fn(&mut State, u32) + 'static,
) -> Result<StreamListener<()>, AudioError> {
    let state = Rc::downgrade(state);
    let listener = output
        .stream()
        .add_local_listener::<()>()
        .state_changed(move |stream, _, _old, new| {
            let Some(state) = state.upgrade() else {
                return;
            };
            match new {
                StreamState::Paused | StreamState::Streaming => {
                    on_node(&mut state.borrow_mut(), stream.node_id());
                }
                StreamState::Error(message) => state.borrow().report(message),
                _ => {}
            }
        })
        .register()?;
    Ok(listener)
}

/// The mutable state of the PipeWire thread: the graph model, the link target and the links we own.
struct State {
    core: CoreRc,
    events: EventSink,
    graph: Graph,
    watched_nodes: HashMap<u32, (Node, NodeListener)>,
    published_apps: Vec<RecordingApp>,
    inject_node: Option<u32>,
    target: Option<u32>,
    /// Link proxies; a link (created with `object.linger = false`) lives as long as its proxy.
    links: HashMap<LinkPair, Link>,
}

impl State {
    fn new(core: CoreRc, events: EventSink) -> Self {
        Self {
            core,
            events,
            graph: Graph::default(),
            watched_nodes: HashMap::new(),
            published_apps: Vec::new(),
            inject_node: None,
            target: None,
            links: HashMap::new(),
        }
    }

    fn set_node(&mut self, id: u32, props: NodeProps) {
        self.graph.set_node(id, props);
        self.graph_changed();
    }

    fn add_port(&mut self, port: PortProps) {
        self.graph.add_port(port);
        self.graph_changed();
    }

    fn remove(&mut self, id: u32) {
        self.graph.remove(id);
        self.watched_nodes.remove(&id);
        self.graph_changed();
    }

    fn set_inject_node(&mut self, node_id: u32) {
        self.inject_node = Some(node_id);
        self.sync_links();
    }

    fn set_target(&mut self, target: Option<u32>) {
        self.target = target;
        self.sync_links();
    }

    fn report(&self, message: String) {
        self.send(Event::Error(message));
    }

    fn graph_changed(&mut self) {
        self.publish_recording_apps();
        self.sync_links();
    }

    fn publish_recording_apps(&mut self) {
        let apps = self.graph.recording_apps();
        if apps != self.published_apps {
            self.send(Event::RecordingApps(apps.clone()));
            self.published_apps = apps;
        }
    }

    /// Makes the set of links we own match the plan for the current graph and target.
    fn sync_links(&mut self) {
        let planned = self.planned_links();
        self.links.retain(|pair, _| planned.contains(pair));
        for pair in planned {
            if !self.links.contains_key(&pair) {
                self.create_link(pair);
            }
        }
    }

    fn planned_links(&self) -> Vec<LinkPair> {
        let (Some(source), Some(target)) = (self.inject_node, self.target) else {
            return Vec::new();
        };
        plan_links(
            &self.graph.ports(source, Direction::Out),
            &self.graph.ports(target, Direction::In),
        )
    }

    fn create_link(&mut self, pair: LinkPair) {
        let (Some(source), Some(target)) = (self.inject_node, self.target) else {
            return;
        };
        let props = pw::properties::properties! {
            *pw::keys::LINK_OUTPUT_NODE => source.to_string(),
            *pw::keys::LINK_OUTPUT_PORT => pair.output_port.to_string(),
            *pw::keys::LINK_INPUT_NODE => target.to_string(),
            *pw::keys::LINK_INPUT_PORT => pair.input_port.to_string(),
            *pw::keys::OBJECT_LINGER => "false",
        };
        match self.core.create_object::<Link>("link-factory", &props) {
            Ok(link) => {
                self.links.insert(pair, link);
            }
            Err(error) => self.report(format!("creating link {pair:?}: {error}")),
        }
    }

    fn send(&self, event: Event) {
        (self.events)(event);
    }
}

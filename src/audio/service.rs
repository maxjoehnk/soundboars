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
use pw::metadata::{Metadata, MetadataListener};
use pw::node::{Node, NodeChangeMask, NodeListener};
use pw::proxy::{ProxyListener, ProxyT};
use pw::registry::{GlobalObject, RegistryRc};
use pw::spa::utils::dict::DictRef;
use pw::stream::{StreamListener, StreamState};
use pw::types::ObjectType;

use super::AudioError;
use super::graph::{
    DEFAULT_SOURCE_KEY, Direction, Graph, LinkProps, NodeProps, PortProps, RECORDING_STREAM_CLASS,
    RecordingApp, SOURCE_CLASSES, parse_default_node,
};
use super::link_plan::{NodeLink, choose_microphone, plan_links, plan_virtual_mic};
use super::output::{INJECT, LOCAL, OutputStream, Render};
use super::virtual_mic;

/// What the PipeWire thread reports back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The apps that currently record audio; sent whenever the list changes.
    RecordingApps(Vec<RecordingApp>),
    /// The description of the microphone the virtual microphone mixes in (the default source, see
    /// [`choose_microphone`]), `None` if there is none; sent whenever it changes.
    Microphone(Option<String>),
    /// An asynchronous PipeWire error, e.g. a link that couldn't be created.
    Error(String),
}

/// Where the inject stream goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    None,
    /// Straight into this recording stream.
    App(u32),
    /// Into the virtual microphone, which exists only while this is the target.
    VirtualMic,
}

enum Command {
    SetTarget(Target),
    Quit,
}

/// Receives the PipeWire thread's [`Event`]s, on that thread; it must not block.
pub type EventSink = Box<dyn Fn(Event) + Send>;

/// What the output streams play; see [`Render`] for the real-time rules they must follow.
pub struct Renders {
    /// Fills `soundboars-inject`, which goes into the linked app or the virtual microphone.
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
        self.send(Command::SetTarget(Target::App(node_id)));
    }

    /// Creates the virtual microphone and links the inject stream and the user's microphone into
    /// it, replacing any previous target. The virtual microphone goes away with the next target.
    pub fn use_virtual_mic(&self) {
        self.send(Command::SetTarget(Target::VirtualMic));
    }

    pub fn unlink(&self) {
        self.send(Command::SetTarget(Target::None));
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
        let state = Rc::new_cyclic(|this| RefCell::new(State::new(core.clone(), events, this)));
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
            Command::SetTarget(target) => state.borrow_mut().set_target(target),
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
        ObjectType::Node if props.get("media.class").is_some_and(is_watched_class) => {
            watch_node(state, registry, global);
        }
        ObjectType::Port => {
            if let Some(port) = PortProps::parse(global.id, |key| props.get(key)) {
                state.borrow_mut().add_port(port);
            }
        }
        ObjectType::Link => {
            if let Some(link) = LinkProps::parse(|key| props.get(key)) {
                state.borrow_mut().add_link(global.id, link);
            }
        }
        ObjectType::Metadata if props.get("metadata.name") == Some("default") => {
            watch_default_metadata(state, registry, global);
        }
        _ => {}
    }
}

/// Recording streams (the apps to inject into) and sources (the microphones to mix in).
fn is_watched_class(media_class: &str) -> bool {
    media_class == RECORDING_STREAM_CLASS || SOURCE_CLASSES.contains(&media_class)
}

/// Follows which node is the default source.
fn watch_default_metadata(
    state: &Rc<RefCell<State>>,
    registry: &RegistryRc,
    global: &GlobalObject<&DictRef>,
) {
    let metadata: Metadata = match registry.bind(global) {
        Ok(metadata) => metadata,
        Err(error) => {
            return state
                .borrow()
                .report(format!("binding metadata {}: {error}", global.id));
        }
    };
    let weak_state: Weak<RefCell<State>> = Rc::downgrade(state);
    let listener = metadata
        .add_listener_local()
        .property(move |subject, key, _type, value| {
            // A `None` key clears every property, a `None` value clears the key's.
            let concerns_default_source = key.is_none_or(|key| key == DEFAULT_SOURCE_KEY);
            if let (0, true, Some(state)) = (subject, concerns_default_source, weak_state.upgrade())
            {
                state
                    .borrow_mut()
                    .set_default_source(value.and_then(parse_default_node));
            }
            0
        })
        .register();
    state.borrow_mut().default_metadata = Some((global.id, metadata, listener));
}

/// Registry globals only carry a few node properties; the app name, binary, media name and
/// priority come with the node's info, so these nodes are bound and their info is watched.
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

/// Our virtual microphone node. It (created with `object.linger = false`) lives as long as its
/// proxy.
struct VirtualMic {
    _listener: ProxyListener,
    _node: Node,
}

/// The mutable state of the PipeWire thread: the graph model, the link target and the links and
/// nodes we own.
struct State {
    /// The `Rc` this state lives in, for callbacks of proxies it creates.
    this: Weak<RefCell<State>>,
    core: CoreRc,
    events: EventSink,
    graph: Graph,
    watched_nodes: HashMap<u32, (Node, NodeListener)>,
    default_metadata: Option<(u32, Metadata, MetadataListener)>,
    /// The `node.name` of the default source.
    default_source: Option<String>,
    published_apps: Vec<RecordingApp>,
    published_microphone: Option<String>,
    inject_node: Option<u32>,
    target: Target,
    virtual_mic: Option<VirtualMic>,
    /// The global id of [`Self::virtual_mic`], once PipeWire has bound it.
    virtual_mic_id: Option<u32>,
    /// Link proxies; a link (created with `object.linger = false`) lives as long as its proxy.
    links: HashMap<NodeLink, Link>,
}

impl State {
    fn new(core: CoreRc, events: EventSink, this: &Weak<RefCell<State>>) -> Self {
        Self {
            this: Weak::clone(this),
            core,
            events,
            graph: Graph::default(),
            watched_nodes: HashMap::new(),
            default_metadata: None,
            default_source: None,
            published_apps: Vec::new(),
            published_microphone: None,
            inject_node: None,
            target: Target::None,
            virtual_mic: None,
            virtual_mic_id: None,
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

    fn add_link(&mut self, id: u32, link: LinkProps) {
        self.graph.add_link(id, link);
        self.graph_changed();
    }

    fn remove(&mut self, id: u32) {
        self.graph.remove(id);
        self.watched_nodes.remove(&id);
        if self
            .default_metadata
            .as_ref()
            .is_some_and(|(metadata_id, ..)| *metadata_id == id)
        {
            self.default_metadata = None;
            self.default_source = None;
        }
        self.graph_changed();
    }

    fn set_default_source(&mut self, name: Option<String>) {
        self.default_source = name;
        self.graph_changed();
    }

    fn set_virtual_mic_id(&mut self, id: u32) {
        self.virtual_mic_id = Some(id);
        self.sync_links();
    }

    fn set_inject_node(&mut self, node_id: u32) {
        self.inject_node = Some(node_id);
        self.sync_links();
    }

    /// Switches the target; the virtual microphone exists only while it is the target.
    fn set_target(&mut self, target: Target) {
        self.target = target;
        let wants_virtual_mic = target == Target::VirtualMic;
        if wants_virtual_mic && self.virtual_mic.is_none() {
            self.create_virtual_mic();
        }
        self.sync_links();
        if !wants_virtual_mic {
            self.virtual_mic = None;
            self.virtual_mic_id = None;
        }
    }

    fn create_virtual_mic(&mut self) {
        let node = match virtual_mic::create(&self.core) {
            Ok(node) => node,
            Err(error) => return self.report(format!("creating the virtual microphone: {error}")),
        };
        let this = Weak::clone(&self.this);
        let listener = node
            .upcast_ref()
            .add_listener_local()
            .bound(move |id| {
                if let Some(state) = this.upgrade() {
                    state.borrow_mut().set_virtual_mic_id(id);
                }
            })
            .register();
        self.virtual_mic = Some(VirtualMic {
            _listener: listener,
            _node: node,
        });
    }

    fn report(&self, message: String) {
        self.send(Event::Error(message));
    }

    fn graph_changed(&mut self) {
        self.publish_recording_apps();
        self.publish_microphone();
        self.sync_links();
    }

    fn publish_microphone(&mut self) {
        let sources = self.graph.sources();
        let microphone = choose_microphone(&sources, self.default_source.as_deref())
            .map(|source| source.description.clone());
        if microphone != self.published_microphone {
            self.send(Event::Microphone(microphone.clone()));
            self.published_microphone = microphone;
        }
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

    fn planned_links(&self) -> Vec<NodeLink> {
        match (self.target, self.inject_node, self.virtual_mic_id) {
            (Target::App(target), Some(inject), _) => {
                let pairs = plan_links(
                    &self.graph.ports(inject, Direction::Out),
                    &self.graph.ports(target, Direction::In),
                );
                NodeLink::all(inject, target, pairs).collect()
            }
            (Target::VirtualMic, inject, Some(virtual_mic)) => plan_virtual_mic(
                &self.graph,
                inject,
                virtual_mic,
                self.default_source.as_deref(),
            ),
            _ => Vec::new(),
        }
    }

    fn create_link(&mut self, link: NodeLink) {
        let props = pw::properties::properties! {
            *pw::keys::LINK_OUTPUT_NODE => link.output_node.to_string(),
            *pw::keys::LINK_OUTPUT_PORT => link.ports.output_port.to_string(),
            *pw::keys::LINK_INPUT_NODE => link.input_node.to_string(),
            *pw::keys::LINK_INPUT_PORT => link.ports.input_port.to_string(),
            *pw::keys::OBJECT_LINGER => "false",
        };
        match self.core.create_object::<Link>("link-factory", &props) {
            Ok(proxy) => {
                self.links.insert(link, proxy);
            }
            Err(error) => self.report(format!("creating link {link:?}: {error}")),
        }
    }

    fn send(&self, event: Event) {
        (self.events)(event);
    }
}

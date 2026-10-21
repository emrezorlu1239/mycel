//! The Mycel network node.
//!
//! A node runs in one of three roles:
//!
//! * **Host** – owns a server. Members connect to it; it relays chat between
//!   them and decides who is in.
//! * **Member** – joins a server using an invite code.
//! * **Relay** – a helper with a reachable address that forwards traffic for
//!   peers that cannot reach each other directly (e.g. both behind CGNAT).
//!
//! Connectivity strategy, in order of preference:
//!
//! 1. Direct connection (same LAN, a public address, or a forwarded port).
//! 2. A relayed connection through a relay, immediately followed by a
//!    DCUtR hole-punch attempt that upgrades it to a direct connection.
//! 3. If hole punching fails (symmetric NAT / CGNAT), stay on the relay.
//!
//! The node runs on its own task. Callers interact with it through a
//! [`NodeHandle`]: they send [`Command`]s and receive [`NodeEvent`]s.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr};
use std::num::NonZeroU32;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use libp2p::core::ConnectedPoint;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, OutboundRequestId, ProtocolSupport, ResponseChannel};
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::swarm::{ConnectionId, NetworkBehaviour, SwarmEvent};
use libp2p::{
    dcutr, identify, identity::Keypair, mdns, noise, ping, relay, tcp, yamux, Multiaddr, PeerId,
    Swarm,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::invite::Invite;
use crate::protocol::{sanitize_nick, Request, Response, CHAT_PROTOCOL, MAX_MESSAGE_BYTES};

/// Agent string announced to other peers through identify.
pub const AGENT_VERSION: &str = concat!("mycel/", env!("CARGO_PKG_VERSION"));

/// How long to wait before redialling a host we lost.
const RECONNECT_DELAY: Duration = Duration::from_secs(3);

/// After a successful hole punch, keep the relayed connection this long so
/// messages already travelling over it can arrive, then close it.
const RELAY_DRAIN: Duration = Duration::from_secs(2);

/// Addresses usually arrive in a burst at start-up; wait for them to settle
/// before announcing a new invite so people do not copy a half-ready one.
const ADDRESS_SETTLE: Duration = Duration::from_millis(1500);

/// What the node is for.
#[derive(Debug, Clone)]
pub enum Role {
    Host { server: String },
    Member { invite: Invite },
    Relay,
}

/// Node settings.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub keypair: Keypair,
    pub role: Role,
    /// Display name in chat.
    pub nick: String,
    /// Address to bind on. `0.0.0.0` means every interface.
    pub listen_ip: IpAddr,
    /// TCP and UDP port to listen on. `0` picks a free port.
    pub port: u16,
    /// Host only: relays to keep a reservation on, so members behind other
    /// NATs can reach this host. Each must end in `/p2p/<relay-peer-id>`.
    pub relays: Vec<Multiaddr>,
    /// Addresses this node is known to be reachable at from outside (for
    /// example after port forwarding). Added to invites and announced.
    pub external_addrs: Vec<Multiaddr>,
    /// Discover peers on the local network automatically.
    pub mdns: bool,
    /// Put loopback addresses into invites. Only useful for local testing.
    pub include_loopback_in_invite: bool,
}

impl NodeConfig {
    pub fn new(keypair: Keypair, role: Role, nick: impl Into<String>) -> Self {
        Self {
            keypair,
            role,
            nick: nick.into(),
            listen_ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: 0,
            relays: Vec::new(),
            external_addrs: Vec::new(),
            mdns: true,
            include_loopback_in_invite: false,
        }
    }
}

/// How a connection reaches the other peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionPath {
    /// Straight to the peer: LAN, public address or a punched hole.
    Direct,
    /// Through a relay node.
    Relayed,
}

impl std::fmt::Display for ConnectionPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ConnectionPath::Direct => "DIRECT",
            ConnectionPath::Relayed => "RELAYED",
        })
    }
}

/// Things that happen inside the node, reported to the caller.
#[derive(Debug, Clone)]
pub enum NodeEvent {
    /// The node is listening on a new local address.
    Listening { addr: Multiaddr },
    /// Host: the invite changed (new address or relay reservation).
    InviteUpdated { invite: Invite },
    /// Relay: addresses other nodes can use to reach this relay.
    RelayAddresses { addrs: Vec<Multiaddr> },
    /// Host: a relay accepted our reservation.
    RelayReservation { relay: PeerId },
    /// A connection to a peer was opened.
    Connected {
        peer: PeerId,
        path: ConnectionPath,
        addr: Multiaddr,
    },
    /// The last connection to a peer was closed.
    Disconnected { peer: PeerId },
    /// A peer is now reached directly, so its relayed connection was closed
    /// to free the relay.
    RelayReleased { peer: PeerId },
    /// Result of a hole-punching attempt.
    HolePunch {
        peer: PeerId,
        success: bool,
        detail: String,
    },
    /// Member: the host accepted us.
    Joined {
        server: String,
        members: Vec<String>,
    },
    /// Member: the host refused a request.
    Denied { reason: String },
    /// Host: someone joined.
    MemberJoined { peer: PeerId, nick: String },
    /// Host: someone left.
    MemberLeft { peer: PeerId, nick: String },
    /// A chat message from someone else.
    Message { from: String, text: String },
    /// Something went wrong but the node keeps running.
    Warning { detail: String },
}

/// Instructions for the node.
#[derive(Debug, Clone)]
pub enum Command {
    /// Post a chat message.
    Send(String),
    /// Stop the node.
    Shutdown,
}

/// Handle to a running node.
pub struct NodeHandle {
    pub peer_id: PeerId,
    pub events: mpsc::Receiver<NodeEvent>,
    commands: mpsc::Sender<Command>,
    task: JoinHandle<()>,
}

impl NodeHandle {
    /// Posts a chat message.
    pub async fn send(&self, text: impl Into<String>) -> Result<()> {
        self.commands
            .send(Command::Send(text.into()))
            .await
            .context("node has stopped")
    }

    /// A cloneable sender, for feeding commands from another task.
    pub fn commands(&self) -> mpsc::Sender<Command> {
        self.commands.clone()
    }

    /// Stops the node and waits for it to finish.
    pub async fn shutdown(self) {
        let _ = self.commands.send(Command::Shutdown).await;
        let _ = self.task.await;
    }
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    relay_client: relay::client::Behaviour,
    relay_server: Toggle<relay::Behaviour>,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    dcutr: dcutr::Behaviour,
    mdns: Toggle<mdns::tokio::Behaviour>,
    chat: request_response::json::Behaviour<Request, Response>,
}

/// Starts a node on the current Tokio runtime.
pub async fn start(config: NodeConfig) -> Result<NodeHandle> {
    let peer_id = config.keypair.public().to_peer_id();
    let mut swarm = build_swarm(&config)?;

    let ip_proto = match config.listen_ip {
        IpAddr::V4(ip) => Protocol::Ip4(ip),
        IpAddr::V6(ip) => Protocol::Ip6(ip),
    };
    let quic: Multiaddr = Multiaddr::empty()
        .with(ip_proto.clone())
        .with(Protocol::Udp(config.port))
        .with(Protocol::QuicV1);
    let tcp: Multiaddr = Multiaddr::empty()
        .with(ip_proto)
        .with(Protocol::Tcp(config.port));
    swarm.listen_on(quic).context("listening on QUIC")?;
    swarm.listen_on(tcp).context("listening on TCP")?;

    for addr in &config.external_addrs {
        swarm.add_external_address(addr.clone());
    }

    if let Role::Host { .. } = config.role {
        for relay in &config.relays {
            let circuit = relay.clone().with(Protocol::P2pCircuit);
            swarm
                .listen_on(circuit)
                .with_context(|| format!("reserving a slot on relay {relay}"))?;
        }
    }

    let (cmd_tx, cmd_rx) = mpsc::channel(64);
    let (evt_tx, evt_rx) = mpsc::channel(256);

    let mut state = NodeState::new(config, peer_id, evt_tx);
    if let Role::Member { invite } = &state.config.role {
        let invite = invite.clone();
        state.dial_host(&mut swarm, &invite);
    }

    let task = tokio::spawn(async move { state.run(swarm, cmd_rx).await });

    Ok(NodeHandle {
        peer_id,
        events: evt_rx,
        commands: cmd_tx,
        task,
    })
}

fn build_swarm(config: &NodeConfig) -> Result<Swarm<Behaviour>> {
    let is_relay = matches!(config.role, Role::Relay);
    let mdns_enabled = config.mdns && !is_relay;

    let swarm = libp2p::SwarmBuilder::with_existing_identity(config.keypair.clone())
        .with_tokio()
        .with_tcp(
            tcp::Config::default().nodelay(true),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_quic()
        .with_dns()?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|key, relay_client| {
            let local_peer_id = key.public().to_peer_id();

            let relay_server = is_relay.then(|| {
                // Generous limits: our relays carry long-lived chat sessions,
                // unlike the short-lived defaults meant for hole punching only.
                let cfg = relay::Config {
                    max_circuit_duration: Duration::from_secs(60 * 60),
                    max_circuit_bytes: 1 << 30,
                    ..Default::default()
                }
                .reservation_rate_per_peer(NonZeroU32::new(30).unwrap(), Duration::from_secs(60))
                .circuit_src_per_peer(NonZeroU32::new(30).unwrap(), Duration::from_secs(60));
                relay::Behaviour::new(local_peer_id, cfg)
            });

            let mdns = if mdns_enabled {
                Some(mdns::tokio::Behaviour::new(
                    mdns::Config::default(),
                    local_peer_id,
                )?)
            } else {
                None
            };

            Ok(Behaviour {
                relay_client,
                relay_server: relay_server.into(),
                identify: identify::Behaviour::new(
                    identify::Config::new("/mycel/id/0.1.0".into(), key.public())
                        .with_agent_version(AGENT_VERSION.into()),
                ),
                ping: ping::Behaviour::new(
                    ping::Config::new().with_interval(Duration::from_secs(15)),
                ),
                dcutr: dcutr::Behaviour::new(local_peer_id),
                mdns: mdns.into(),
                chat: request_response::json::Behaviour::new(
                    [(CHAT_PROTOCOL, ProtocolSupport::Full)],
                    request_response::Config::default()
                        .with_request_timeout(Duration::from_secs(20)),
                ),
            })
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(120)))
        .build();

    Ok(swarm)
}

/// Everything the event loop keeps track of.
struct NodeState {
    config: NodeConfig,
    peer_id: PeerId,
    events: mpsc::Sender<NodeEvent>,

    /// Our listen addresses, for building invites.
    listen_addrs: BTreeSet<Multiaddr>,
    /// The last invite we published, to avoid repeating identical ones.
    last_invite: Option<String>,
    /// When our address set last changed and has not been announced yet.
    addresses_dirty_since: Option<tokio::time::Instant>,

    // Host state.
    members: HashMap<PeerId, String>,

    // Member state.
    joined: bool,
    join_in_flight: Option<OutboundRequestId>,
    outbox: VecDeque<String>,
    reconnect_at: Option<tokio::time::Instant>,

    /// Open relayed connections per peer.
    relayed_conns: HashMap<PeerId, HashSet<ConnectionId>>,
    /// Relayed connections to close once in-flight messages have drained.
    relay_drain: Vec<(tokio::time::Instant, PeerId)>,
}

impl NodeState {
    fn new(config: NodeConfig, peer_id: PeerId, events: mpsc::Sender<NodeEvent>) -> Self {
        Self {
            config,
            peer_id,
            events,
            listen_addrs: BTreeSet::new(),
            last_invite: None,
            addresses_dirty_since: None,
            members: HashMap::new(),
            joined: false,
            join_in_flight: None,
            outbox: VecDeque::new(),
            reconnect_at: None,
            relayed_conns: HashMap::new(),
            relay_drain: Vec::new(),
        }
    }

    fn host_peer(&self) -> Option<PeerId> {
        match &self.config.role {
            Role::Member { invite } => Some(invite.host),
            _ => None,
        }
    }

    async fn emit(&self, event: NodeEvent) {
        // The receiver may have been dropped by a caller that only wants the
        // node to run; that is not an error.
        let _ = self.events.send(event).await;
    }

    async fn run(mut self, mut swarm: Swarm<Behaviour>, mut commands: mpsc::Receiver<Command>) {
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! {
                event = swarm.select_next_some() => self.on_swarm_event(&mut swarm, event).await,
                command = commands.recv() => match command {
                    Some(Command::Send(text)) => self.on_send(&mut swarm, text).await,
                    Some(Command::Shutdown) | None => break,
                },
                _ = tick.tick() => self.on_tick(&mut swarm).await,
            }
        }
        info!("node stopped");
    }

    async fn on_tick(&mut self, swarm: &mut Swarm<Behaviour>) {
        let now = tokio::time::Instant::now();
        let due: Vec<PeerId> = self
            .relay_drain
            .iter()
            .filter(|(at, _)| *at <= now)
            .map(|(_, p)| *p)
            .collect();
        self.relay_drain.retain(|(at, _)| *at > now);
        for peer in due {
            let ids: Vec<ConnectionId> = self
                .relayed_conns
                .get(&peer)
                .map(|set| set.iter().copied().collect())
                .unwrap_or_default();
            if !ids.is_empty() {
                for id in ids {
                    swarm.close_connection(id);
                }
                info!(%peer, "relayed connection released");
                self.emit(NodeEvent::RelayReleased { peer }).await;
            }
        }
        if let Some(since) = self.addresses_dirty_since {
            if since.elapsed() >= ADDRESS_SETTLE {
                self.addresses_dirty_since = None;
                self.publish_addresses(swarm).await;
            }
        }
        if let Some(at) = self.reconnect_at {
            if tokio::time::Instant::now() >= at {
                self.reconnect_at = None;
                if let Role::Member { invite } = &self.config.role {
                    let invite = invite.clone();
                    if !swarm.is_connected(&invite.host) {
                        info!("reconnecting to host");
                        self.dial_host(swarm, &invite);
                    }
                }
            }
        }
    }

    fn dial_host(&mut self, swarm: &mut Swarm<Behaviour>, invite: &Invite) {
        // Talk to the invite's relays directly first. Besides making the
        // circuit faster, a QUIC connection to a relay tells us our public
        // UDP address, which is what hole punching needs.
        for (relay, addr) in relays_in(&invite.addrs) {
            if !swarm.is_connected(&relay) {
                let _ = swarm.dial(DialOpts::peer_id(relay).addresses(vec![addr]).build());
            }
        }
        let opts = DialOpts::peer_id(invite.host)
            .addresses(invite.addrs.clone())
            .extend_addresses_through_behaviour()
            .build();
        if let Err(e) = swarm.dial(opts) {
            warn!("could not dial host: {e}");
            self.reconnect_at = Some(tokio::time::Instant::now() + RECONNECT_DELAY);
        }
    }

    // ----- commands -------------------------------------------------------

    async fn on_send(&mut self, swarm: &mut Swarm<Behaviour>, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        if text.len() > MAX_MESSAGE_BYTES {
            self.emit(NodeEvent::Warning {
                detail: format!("message too long (max {MAX_MESSAGE_BYTES} bytes)"),
            })
            .await;
            return;
        }
        match &self.config.role {
            Role::Host { .. } => {
                let from = self.config.nick.clone();
                self.broadcast(swarm, None, &from, &text);
            }
            Role::Member { invite } => {
                if self.joined {
                    swarm
                        .behaviour_mut()
                        .chat
                        .send_request(&invite.host, Request::Post { text });
                } else {
                    self.outbox.push_back(text);
                    self.emit(NodeEvent::Warning {
                        detail: "not connected yet; message queued".into(),
                    })
                    .await;
                }
            }
            Role::Relay => {}
        }
    }

    /// Host: delivers a message to every member except `skip`.
    fn broadcast(
        &self,
        swarm: &mut Swarm<Behaviour>,
        skip: Option<PeerId>,
        from: &str,
        text: &str,
    ) {
        for peer in self.members.keys().filter(|p| Some(**p) != skip) {
            swarm.behaviour_mut().chat.send_request(
                peer,
                Request::Deliver {
                    from: from.to_string(),
                    text: text.to_string(),
                },
            );
        }
    }

    // ----- swarm events ---------------------------------------------------

    async fn on_swarm_event(
        &mut self,
        swarm: &mut Swarm<Behaviour>,
        event: SwarmEvent<BehaviourEvent>,
    ) {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!(%address, "listening");
                self.listen_addrs.insert(address.clone());
                // A relay only accepts reservations once it knows an address
                // others can reach it at. Without explicit --external
                // addresses, assume our listen addresses are reachable.
                if matches!(self.config.role, Role::Relay)
                    && self.config.external_addrs.is_empty()
                    && (self.config.include_loopback_in_invite || !is_loopback(&address))
                {
                    swarm.add_external_address(address.clone());
                }
                self.emit(NodeEvent::Listening { addr: address }).await;
                self.mark_addresses_changed();
            }
            SwarmEvent::ExpiredListenAddr { address, .. } => {
                self.listen_addrs.remove(&address);
                self.mark_addresses_changed();
            }
            SwarmEvent::ConnectionEstablished {
                peer_id,
                connection_id,
                endpoint,
                ..
            } => {
                let path = path_of(&endpoint);
                if path == ConnectionPath::Relayed {
                    self.relayed_conns
                        .entry(peer_id)
                        .or_default()
                        .insert(connection_id);
                }
                let addr = endpoint.get_remote_address().clone();
                info!(%peer_id, %path, %addr, "connection established");
                self.emit(NodeEvent::Connected {
                    peer: peer_id,
                    path,
                    addr,
                })
                .await;
                if Some(peer_id) == self.host_peer()
                    && !self.joined
                    && self.join_in_flight.is_none()
                {
                    let nick = self.config.nick.clone();
                    let id = swarm
                        .behaviour_mut()
                        .chat
                        .send_request(&peer_id, Request::Join { nick });
                    self.join_in_flight = Some(id);
                }
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                connection_id,
                num_established,
                cause,
                ..
            } => {
                debug!(%peer_id, ?cause, num_established, "connection closed");
                if let Some(set) = self.relayed_conns.get_mut(&peer_id) {
                    set.remove(&connection_id);
                    if set.is_empty() {
                        self.relayed_conns.remove(&peer_id);
                    }
                }
                if num_established == 0 {
                    self.emit(NodeEvent::Disconnected { peer: peer_id }).await;
                    if let Some(nick) = self.members.remove(&peer_id) {
                        self.emit(NodeEvent::MemberLeft {
                            peer: peer_id,
                            nick: nick.clone(),
                        })
                        .await;
                        self.broadcast(swarm, None, "mycel", &format!("{nick} left"));
                    }
                    if Some(peer_id) == self.host_peer() {
                        self.joined = false;
                        self.join_in_flight = None;
                        self.reconnect_at = Some(tokio::time::Instant::now() + RECONNECT_DELAY);
                    }
                }
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                if let Some(host) = peer_id.filter(|p| Some(*p) == self.host_peer()) {
                    if swarm.is_connected(&host) {
                        // An extra path (e.g. a hole punch) failed, but we
                        // are still connected some other way.
                        debug!("additional dial to host failed: {error}");
                    } else {
                        info!("could not reach host: {error}");
                        self.emit(NodeEvent::Warning {
                            detail: format!("could not reach host: {error}"),
                        })
                        .await;
                        self.reconnect_at = Some(tokio::time::Instant::now() + RECONNECT_DELAY);
                    }
                } else {
                    debug!(?peer_id, "outgoing connection error: {error}");
                }
            }
            SwarmEvent::ExternalAddrConfirmed { address } => {
                info!(%address, "external address confirmed");
                self.mark_addresses_changed();
            }
            SwarmEvent::Behaviour(event) => self.on_behaviour_event(swarm, event).await,
            _ => {}
        }
    }

    async fn on_behaviour_event(&mut self, swarm: &mut Swarm<Behaviour>, event: BehaviourEvent) {
        match event {
            BehaviourEvent::RelayClient(relay::client::Event::ReservationReqAccepted {
                relay_peer_id,
                renewal,
                ..
            }) => {
                if !renewal {
                    info!(%relay_peer_id, "relay reservation accepted");
                    self.emit(NodeEvent::RelayReservation {
                        relay: relay_peer_id,
                    })
                    .await;
                }
            }
            BehaviourEvent::Dcutr(dcutr::Event {
                remote_peer_id,
                result,
            }) => {
                let (success, detail) = match result {
                    Ok(_) => (true, "upgraded to a direct connection".to_string()),
                    Err(e) => (false, e.to_string()),
                };
                if success {
                    // Messages are spread over all open connections; drop the
                    // relayed one so traffic stops costing the relay anything.
                    self.relay_drain
                        .push((tokio::time::Instant::now() + RELAY_DRAIN, remote_peer_id));
                }
                info!(%remote_peer_id, success, %detail, "hole punch");
                self.emit(NodeEvent::HolePunch {
                    peer: remote_peer_id,
                    success,
                    detail,
                })
                .await;
            }
            BehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. }) => {
                debug!(%peer_id, agent = %info.agent_version, observed = %info.observed_addr, "identified");
            }
            BehaviourEvent::Mdns(mdns::Event::Discovered(found)) => {
                for (peer, addr) in found {
                    debug!(%peer, %addr, "found on local network");
                    if Some(peer) == self.host_peer() && !swarm.is_connected(&peer) {
                        let _ = swarm.dial(DialOpts::peer_id(peer).addresses(vec![addr]).build());
                    }
                }
            }
            BehaviourEvent::Chat(event) => self.on_chat_event(swarm, event).await,
            _ => {}
        }
    }

    async fn on_chat_event(
        &mut self,
        swarm: &mut Swarm<Behaviour>,
        event: request_response::Event<Request, Response>,
    ) {
        use request_response::{Event, Message};
        match event {
            Event::Message {
                peer,
                message:
                    Message::Request {
                        request, channel, ..
                    },
                ..
            } => self.on_request(swarm, peer, request, channel).await,
            Event::Message {
                peer,
                message:
                    Message::Response {
                        request_id,
                        response,
                    },
                ..
            } => self.on_response(swarm, peer, request_id, response).await,
            Event::OutboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                warn!(%peer, "request failed: {error}");
                if Some(request_id) == self.join_in_flight {
                    self.join_in_flight = None;
                }
                self.emit(NodeEvent::Warning {
                    detail: format!("could not deliver to {peer}: {error}"),
                })
                .await;
            }
            Event::InboundFailure { peer, error, .. } => {
                debug!(%peer, "inbound request failed: {error}");
            }
            Event::ResponseSent { .. } => {}
        }
    }

    async fn on_request(
        &mut self,
        swarm: &mut Swarm<Behaviour>,
        peer: PeerId,
        request: Request,
        channel: ResponseChannel<Response>,
    ) {
        let response = match (&self.config.role, request) {
            (Role::Host { server }, Request::Join { nick }) => match sanitize_nick(&nick) {
                Some(nick) => {
                    let server = server.clone();
                    let mut members: Vec<String> = self.members.values().cloned().collect();
                    members.push(self.config.nick.clone());
                    members.sort();
                    let is_new = self.members.insert(peer, nick.clone()).is_none();
                    if is_new {
                        self.emit(NodeEvent::MemberJoined {
                            peer,
                            nick: nick.clone(),
                        })
                        .await;
                        self.broadcast(swarm, Some(peer), "mycel", &format!("{nick} joined"));
                    }
                    Response::Welcome { server, members }
                }
                None => Response::Denied {
                    reason: "nickname is empty".into(),
                },
            },
            (Role::Host { .. }, Request::Post { text }) => match self.members.get(&peer).cloned() {
                Some(from) if text.len() <= MAX_MESSAGE_BYTES => {
                    self.emit(NodeEvent::Message {
                        from: from.clone(),
                        text: text.clone(),
                    })
                    .await;
                    self.broadcast(swarm, Some(peer), &from, &text);
                    Response::Ack
                }
                Some(_) => Response::Denied {
                    reason: "message too long".into(),
                },
                None => Response::Denied {
                    reason: "join the server first".into(),
                },
            },
            (Role::Member { invite }, Request::Deliver { from, text }) if peer == invite.host => {
                self.emit(NodeEvent::Message { from, text }).await;
                Response::Ack
            }
            _ => Response::Denied {
                reason: "unexpected request".into(),
            },
        };
        if swarm
            .behaviour_mut()
            .chat
            .send_response(channel, response)
            .is_err()
        {
            debug!(%peer, "peer went away before we could answer");
        }
    }

    async fn on_response(
        &mut self,
        swarm: &mut Swarm<Behaviour>,
        peer: PeerId,
        request_id: OutboundRequestId,
        response: Response,
    ) {
        if Some(request_id) == self.join_in_flight {
            self.join_in_flight = None;
        }
        match response {
            Response::Welcome { server, members } if Some(peer) == self.host_peer() => {
                self.joined = true;
                self.emit(NodeEvent::Joined { server, members }).await;
                while let Some(text) = self.outbox.pop_front() {
                    swarm
                        .behaviour_mut()
                        .chat
                        .send_request(&peer, Request::Post { text });
                }
            }
            Response::Denied { reason } => {
                self.emit(NodeEvent::Denied { reason }).await;
            }
            _ => {}
        }
    }

    // ----- addresses and invites -----------------------------------------

    fn mark_addresses_changed(&mut self) {
        self.addresses_dirty_since = Some(tokio::time::Instant::now());
    }

    /// Re-announces how to reach this node, if anything changed.
    async fn publish_addresses(&mut self, swarm: &mut Swarm<Behaviour>) {
        match &self.config.role {
            Role::Host { server } => {
                let invite = Invite::new(server.clone(), self.peer_id, self.invite_addrs(swarm));
                if invite.addrs.is_empty() {
                    return;
                }
                let code = invite.encode();
                if self.last_invite.as_deref() != Some(code.as_str()) {
                    self.last_invite = Some(code);
                    self.emit(NodeEvent::InviteUpdated { invite }).await;
                }
            }
            Role::Relay => {
                let addrs: Vec<Multiaddr> = self
                    .invite_addrs(swarm)
                    .into_iter()
                    .map(|a| a.with(Protocol::P2p(self.peer_id)))
                    .collect();
                if addrs.is_empty() {
                    return;
                }
                let key = format!("{addrs:?}");
                if self.last_invite.as_deref() != Some(key.as_str()) {
                    self.last_invite = Some(key);
                    self.emit(NodeEvent::RelayAddresses { addrs }).await;
                }
            }
            Role::Member { .. } => {}
        }
    }

    /// Addresses worth sharing, best first: confirmed external addresses,
    /// then LAN addresses, then relayed circuit addresses.
    fn invite_addrs(&self, swarm: &Swarm<Behaviour>) -> Vec<Multiaddr> {
        let mut external: Vec<Multiaddr> = swarm.external_addresses().cloned().collect();
        let mut direct = Vec::new();
        let mut relayed = Vec::new();
        for addr in &self.listen_addrs {
            if is_relayed(addr) {
                // Make sure the circuit address names us as the destination.
                let addr = if addr.iter().last() == Some(Protocol::P2p(self.peer_id)) {
                    addr.clone()
                } else {
                    addr.clone().with(Protocol::P2p(self.peer_id))
                };
                relayed.push(addr);
            } else if self.config.include_loopback_in_invite || !is_loopback(addr) {
                direct.push(addr.clone());
            }
        }
        external.retain(|a| !direct.contains(a) && !is_relayed(a));
        // QUIC before TCP: QUIC connections can be hole-punched, TCP ones
        // cannot, so a QUIC path is worth more. The sort is stable.
        for group in [&mut external, &mut direct, &mut relayed] {
            group.sort_by_key(|a| !is_quic(a));
        }
        external.extend(direct);
        external.extend(relayed);
        external
    }
}

fn is_quic(addr: &Multiaddr) -> bool {
    // For circuit addresses, what matters is how we reach the relay, i.e.
    // the part before /p2p-circuit.
    addr.iter()
        .take_while(|p| *p != Protocol::P2pCircuit)
        .any(|p| p == Protocol::QuicV1)
}

/// The relays mentioned in circuit addresses, preferring QUIC, one per relay.
fn relays_in(addrs: &[Multiaddr]) -> Vec<(PeerId, Multiaddr)> {
    let mut found: Vec<(PeerId, Multiaddr)> = Vec::new();
    for addr in addrs.iter().filter(|a| is_relayed(a)) {
        let mut relay_addr = Multiaddr::empty();
        let mut relay_id = None;
        for p in addr.iter() {
            if p == Protocol::P2pCircuit {
                break;
            }
            if let Protocol::P2p(id) = p {
                relay_id = Some(id);
            }
            relay_addr.push(p);
        }
        let Some(id) = relay_id else { continue };
        match found.iter_mut().find(|(r, _)| *r == id) {
            Some(entry) if !is_quic(&entry.1) && is_quic(&relay_addr) => entry.1 = relay_addr,
            Some(_) => {}
            None => found.push((id, relay_addr)),
        }
    }
    found
}

fn path_of(endpoint: &ConnectedPoint) -> ConnectionPath {
    if endpoint.is_relayed() {
        ConnectionPath::Relayed
    } else {
        ConnectionPath::Direct
    }
}

fn is_relayed(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| p == Protocol::P2pCircuit)
}

fn is_loopback(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| match p {
        Protocol::Ip4(ip) => ip.is_loopback(),
        Protocol::Ip6(ip) => ip.is_loopback(),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_addresses() {
        let lan: Multiaddr = "/ip4/192.168.1.5/udp/4001/quic-v1".parse().unwrap();
        let lo: Multiaddr = "/ip4/127.0.0.1/tcp/4001".parse().unwrap();
        let circuit: Multiaddr =
            "/ip4/203.0.113.10/tcp/4001/p2p/12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN/p2p-circuit"
                .parse()
                .unwrap();
        assert!(!is_relayed(&lan) && !is_loopback(&lan));
        assert!(is_loopback(&lo));
        assert!(is_relayed(&circuit));
        assert!(is_quic(&lan) && !is_quic(&lo) && !is_quic(&circuit));
    }

    #[test]
    fn finds_relays_and_prefers_quic() {
        let relay = "12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN";
        let tcp: Multiaddr = format!("/ip4/203.0.113.10/tcp/4001/p2p/{relay}/p2p-circuit")
            .parse()
            .unwrap();
        let quic: Multiaddr = format!("/ip4/203.0.113.10/udp/4001/quic-v1/p2p/{relay}/p2p-circuit")
            .parse()
            .unwrap();
        let lan: Multiaddr = "/ip4/192.168.1.5/tcp/4001".parse().unwrap();
        let found = relays_in(&[tcp, lan, quic]);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].1,
            format!("/ip4/203.0.113.10/udp/4001/quic-v1/p2p/{relay}")
                .parse::<Multiaddr>()
                .unwrap()
        );
    }
}

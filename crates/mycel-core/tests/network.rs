//! End-to-end tests that run real nodes on this machine (loopback only).

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use libp2p::identity::Keypair;
use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;
use mycel_core::{start, ConnectionPath, Invite, NodeConfig, NodeEvent, NodeHandle, Role};

const TIMEOUT: Duration = Duration::from_secs(20);

fn config(role: Role, nick: &str) -> NodeConfig {
    config_with_key(Keypair::generate_ed25519(), role, nick)
}

fn config_with_key(key: Keypair, role: Role, nick: &str) -> NodeConfig {
    let mut c = NodeConfig::new(key, role, nick);
    c.listen_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    c.mdns = false;
    c.include_loopback_in_invite = true;
    c
}

fn host_role(name: &str) -> Role {
    Role::Host {
        server: name.into(),
    }
}

/// Waits until `pick` returns `Some` for an event, failing after a timeout.
async fn wait_for<T>(
    node: &mut NodeHandle,
    what: &str,
    mut pick: impl FnMut(&NodeEvent) -> Option<T>,
) -> T {
    let result = tokio::time::timeout(TIMEOUT, async {
        loop {
            let event = node.events.recv().await.expect("node stopped");
            if let Some(found) = pick(&event) {
                return found;
            }
        }
    })
    .await;
    result.unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

async fn invite_of(host: &mut NodeHandle) -> Invite {
    wait_for(host, "invite", |e| match e {
        NodeEvent::InviteUpdated { invite } => Some(invite.clone()),
        _ => None,
    })
    .await
}

async fn joined(member: &mut NodeHandle) -> Vec<String> {
    wait_for(member, "join", |e| match e {
        NodeEvent::Joined { members, .. } => Some(members.clone()),
        NodeEvent::Denied { reason } => panic!("join denied: {reason}"),
        _ => None,
    })
    .await
}

async fn message(node: &mut NodeHandle) -> (String, String) {
    wait_for(node, "message", |e| match e {
        NodeEvent::Message { from, text } => Some((from.clone(), text.clone())),
        _ => None,
    })
    .await
}

#[tokio::test]
async fn member_and_host_chat_both_ways() {
    let mut host = start(config(host_role("Test"), "emre")).await.unwrap();
    let invite = invite_of(&mut host).await;

    let mut ali = start(config(Role::Member { invite }, "ali")).await.unwrap();
    assert_eq!(joined(&mut ali).await, vec!["emre".to_string()]);

    ali.send("selam").await.unwrap();
    assert_eq!(message(&mut host).await, ("ali".into(), "selam".into()));

    host.send("hoş geldin").await.unwrap();
    assert_eq!(
        message(&mut ali).await,
        ("emre".into(), "hoş geldin".into())
    );

    ali.shutdown().await;
    host.shutdown().await;
}

#[tokio::test]
async fn host_fans_messages_out_to_everyone_else() {
    let mut host = start(config(host_role("Fan-out"), "emre")).await.unwrap();
    let invite = invite_of(&mut host).await;

    let mut ali = start(config(
        Role::Member {
            invite: invite.clone(),
        },
        "ali",
    ))
    .await
    .unwrap();
    joined(&mut ali).await;
    let mut ayse = start(config(Role::Member { invite }, "ayse"))
        .await
        .unwrap();
    let members = joined(&mut ayse).await;
    assert!(members.contains(&"ali".to_string()) && members.contains(&"emre".to_string()));

    // Ali is told that Ayse arrived.
    assert_eq!(
        message(&mut ali).await,
        ("mycel".into(), "ayse joined".into())
    );

    ayse.send("herkese merhaba").await.unwrap();
    assert_eq!(
        message(&mut host).await,
        ("ayse".into(), "herkese merhaba".into())
    );
    assert_eq!(
        message(&mut ali).await,
        ("ayse".into(), "herkese merhaba".into())
    );

    // The sender must not get her own message echoed back.
    let echo = tokio::time::timeout(Duration::from_millis(800), async {
        loop {
            if let Some(NodeEvent::Message { .. }) = ayse.events.recv().await {
                return;
            }
        }
    })
    .await;
    assert!(echo.is_err(), "sender received an echo of her own message");

    for n in [ali, ayse, host] {
        n.shutdown().await;
    }
}

#[tokio::test]
async fn messages_typed_before_joining_are_delivered() {
    let mut host = start(config(host_role("Queue"), "emre")).await.unwrap();
    let invite = invite_of(&mut host).await;

    let ali = start(config(Role::Member { invite }, "ali")).await.unwrap();
    // Sent immediately, before the connection exists.
    ali.send("early bird").await.unwrap();
    assert_eq!(
        message(&mut host).await,
        ("ali".into(), "early bird".into())
    );

    ali.shutdown().await;
    host.shutdown().await;
}

#[tokio::test]
async fn member_reaches_host_through_a_relay() {
    // 1. A relay.
    let mut relay = start(config(Role::Relay, "relay")).await.unwrap();
    let relay_addr: Multiaddr = wait_for(&mut relay, "relay address", |e| match e {
        NodeEvent::RelayAddresses { addrs } => addrs
            .iter()
            .find(|a| a.iter().any(|p| matches!(p, Protocol::QuicV1)))
            .cloned(),
        _ => None,
    })
    .await;

    // 2. A host that reserves a slot on it.
    let mut host_cfg = config(host_role("Relayed"), "emre");
    host_cfg.relays = vec![relay_addr];
    let mut host = start(host_cfg).await.unwrap();
    wait_for(&mut host, "reservation", |e| {
        matches!(e, NodeEvent::RelayReservation { .. }).then_some(())
    })
    .await;
    let invite = wait_for(&mut host, "invite with circuit address", |e| match e {
        NodeEvent::InviteUpdated { invite }
            if invite
                .addrs
                .iter()
                .any(|a| a.iter().any(|p| p == Protocol::P2pCircuit)) =>
        {
            Some(invite.clone())
        }
        _ => None,
    })
    .await;

    // 3. A member that only knows the relayed address, as if the host were
    //    unreachable directly.
    let circuit_only = Invite::new(
        invite.server.clone(),
        invite.host,
        invite
            .addrs
            .into_iter()
            .filter(|a| a.iter().any(|p| p == Protocol::P2pCircuit))
            .collect(),
    );
    let mut ali = start(config(
        Role::Member {
            invite: circuit_only.clone(),
        },
        "ali",
    ))
    .await
    .unwrap();

    let first_path = wait_for(&mut ali, "connection to host", |e| match e {
        NodeEvent::Connected { peer, path, .. } if *peer == circuit_only.host => Some(*path),
        _ => None,
    })
    .await;
    assert_eq!(first_path, ConnectionPath::Relayed);

    joined(&mut ali).await;
    ali.send("through the relay").await.unwrap();
    assert_eq!(
        message(&mut host).await,
        ("ali".into(), "through the relay".into())
    );

    for n in [ali, host, relay] {
        n.shutdown().await;
    }
}

#[tokio::test]
async fn member_reconnects_when_host_comes_back() {
    let port = free_port();
    let host_key = Keypair::generate_ed25519();

    let mut cfg = config_with_key(host_key.clone(), host_role("Comeback"), "emre");
    cfg.port = port;
    let mut host = start(cfg.clone()).await.unwrap();
    let invite = invite_of(&mut host).await;

    let mut ali = start(config(Role::Member { invite }, "ali")).await.unwrap();
    joined(&mut ali).await;

    // The host's computer goes away...
    host.shutdown().await;
    wait_for(&mut ali, "disconnect", |e| {
        matches!(e, NodeEvent::Disconnected { .. }).then_some(())
    })
    .await;

    // ...and comes back with the same identity and port. The OS may need a
    // moment to release the old sockets.
    let mut host = start_retrying(cfg).await;
    joined(&mut ali).await;
    ali.send("you're back").await.unwrap();
    assert_eq!(
        message(&mut host).await,
        ("ali".into(), "you're back".into())
    );

    ali.shutdown().await;
    host.shutdown().await;
}

#[tokio::test]
async fn member_refuses_an_impostor_host() {
    // An invite that names one identity but points at a node with another
    // must not connect: the address is right, the key is wrong.
    let mut real = start(config(host_role("Real"), "emre")).await.unwrap();
    let invite = invite_of(&mut real).await;
    let forged = Invite::new(
        "Real",
        Keypair::generate_ed25519().public().to_peer_id(),
        invite.addrs,
    );

    let mut ali = start(config(Role::Member { invite: forged }, "ali"))
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            match ali.events.recv().await {
                Some(NodeEvent::Joined { .. }) => return "joined",
                Some(NodeEvent::Warning { .. }) => return "refused",
                Some(_) => {}
                None => return "stopped",
            }
        }
    })
    .await
    .unwrap_or("timeout");
    assert_eq!(outcome, "refused");

    ali.shutdown().await;
    real.shutdown().await;
}

async fn start_retrying(cfg: NodeConfig) -> NodeHandle {
    for _ in 0..50 {
        if let Ok(node) = start(cfg.clone()).await {
            return node;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("port never became free again");
}

fn free_port() -> u16 {
    // UDP and TCP must both be free on the same number.
    loop {
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = udp.local_addr().unwrap().port();
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
}

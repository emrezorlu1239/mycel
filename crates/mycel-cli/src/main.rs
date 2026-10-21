//! `mycel` – Phase 0 command-line client.
//!
//! ```text
//! mycel host  --server "Friday Night" --nick emre       # start a server, prints an invite
//! mycel join  <INVITE> --nick ali                       # join it from another computer
//! mycel relay --external /ip4/<public-ip>/udp/4001/quic-v1   # optional helper node
//! ```
//!
//! Everything you type is sent to the server. Output lines start with a tag in
//! square brackets so scripts and tests can follow what happens.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use libp2p::Multiaddr;
use mycel_core::identity;
use mycel_core::node::Command as NodeCommand;
use mycel_core::{start, Invite, NodeConfig, NodeEvent, Role};
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Parser)]
#[command(
    name = "mycel",
    version,
    about = "Peer-to-peer chat that nobody in between can switch off."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Host a server on this computer and print an invite code.
    Host {
        /// Server name shown to people who join.
        #[arg(long, default_value = "My Mycel")]
        server: String,
        /// Relay to reserve a slot on, so friends behind other routers can
        /// reach you. Repeatable. Format: /ip4/<ip>/udp/<port>/quic-v1/p2p/<id>
        #[arg(long = "relay")]
        relays: Vec<Multiaddr>,
        /// Also write the invite code to this file whenever it changes.
        #[arg(long)]
        invite_file: Option<PathBuf>,
        #[command(flatten)]
        common: Common,
    },
    /// Join a server with an invite code (or `@path` to read it from a file).
    Join {
        invite: String,
        #[command(flatten)]
        common: Common,
    },
    /// Run a relay that helps peers who cannot reach each other directly.
    Relay {
        /// Also write this relay's addresses to this file.
        #[arg(long)]
        addr_file: Option<PathBuf>,
        #[command(flatten)]
        common: Common,
    },
    /// Show this device's identity.
    Id {
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
}

#[derive(Args)]
struct Common {
    /// Your display name.
    #[arg(long, default_value = "anonymous")]
    nick: String,
    /// Port for both TCP and QUIC (UDP). 0 picks a free one.
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// Interface address to bind.
    #[arg(long, default_value = "0.0.0.0")]
    listen_ip: IpAddr,
    /// Where the identity key and data live. Use different folders to run
    /// several nodes on one computer.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// An address this computer is reachable at from the internet (e.g. after
    /// port forwarding). Repeatable.
    #[arg(long = "external")]
    external_addrs: Vec<Multiaddr>,
    /// Disable automatic discovery on the local network.
    #[arg(long)]
    no_mdns: bool,
    /// Include 127.0.0.1 in invites (for testing on one machine).
    #[arg(long)]
    loopback: bool,
    /// Quit automatically after this many seconds (for scripted tests).
    #[arg(long)]
    exit_after: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Id { data_dir } => {
            let dir = resolve_data_dir(data_dir)?;
            let (keypair, created) = identity::load_or_create(&dir)?;
            println!("[id] {}", keypair.public().to_peer_id());
            println!(
                "[id] data dir: {}{}",
                dir.display(),
                if created { " (new)" } else { "" }
            );
            Ok(())
        }
        Command::Host {
            server,
            relays,
            invite_file,
            common,
        } => {
            for relay in &relays {
                if !relay
                    .iter()
                    .any(|p| matches!(p, libp2p::multiaddr::Protocol::P2p(_)))
                {
                    bail!("relay address must end with /p2p/<relay-id>: {relay}");
                }
            }
            run(Role::Host { server }, relays, common, invite_file).await
        }
        Command::Join { invite, common } => {
            let code = match invite.strip_prefix('@') {
                Some(path) => std::fs::read_to_string(path)
                    .with_context(|| format!("reading invite file {path}"))?,
                None => invite,
            };
            let invite = Invite::decode(&code)?;
            run(Role::Member { invite }, Vec::new(), common, None).await
        }
        Command::Relay { addr_file, common } => {
            run(Role::Relay, Vec::new(), common, addr_file).await
        }
    }
}

fn resolve_data_dir(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(dir) = explicit {
        return Ok(dir);
    }
    let dirs = directories::ProjectDirs::from("org", "Mycel", "Mycel")
        .context("could not find a home directory; pass --data-dir")?;
    Ok(dirs.data_dir().to_path_buf())
}

async fn run(
    role: Role,
    relays: Vec<Multiaddr>,
    common: Common,
    output_file: Option<PathBuf>,
) -> Result<()> {
    let data_dir = resolve_data_dir(common.data_dir)?;
    let (keypair, _) = identity::load_or_create(&data_dir)?;

    let mut config = NodeConfig::new(keypair, role.clone(), common.nick.clone());
    config.listen_ip = common.listen_ip;
    config.port = common.port;
    config.relays = relays;
    config.external_addrs = common.external_addrs;
    config.mdns = !common.no_mdns;
    config.include_loopback_in_invite = common.loopback;

    let mut node = start(config).await?;
    println!("[id] {}", node.peer_id);
    match &role {
        Role::Host { server } => println!("[ready] hosting \"{server}\" as {}", common.nick),
        Role::Member { invite } => println!(
            "[ready] joining \"{}\" ({} addresses to try)",
            invite.server,
            invite.addrs.len()
        ),
        Role::Relay => println!("[ready] relay running"),
    }

    let commands = node.commands();
    let scripted = common.exit_after.is_some();
    tokio::spawn(async move {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) if line.trim() == "/quit" => break,
                Ok(Some(line)) => {
                    if commands.send(NodeCommand::Send(line)).await.is_err() {
                        return;
                    }
                }
                // End of input: in scripted runs keep going until the timer.
                Ok(None) | Err(_) if scripted => return,
                Ok(None) | Err(_) => break,
            }
        }
        let _ = commands.send(NodeCommand::Shutdown).await;
    });

    let deadline = async {
        match common.exit_after {
            Some(secs) => tokio::time::sleep(Duration::from_secs(secs)).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            event = node.events.recv() => match event {
                Some(event) => print_event(event, output_file.as_deref()),
                None => break,
            },
            _ = &mut deadline => {
                println!("[exit] time limit reached");
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                println!("[exit] interrupted");
                break;
            }
        }
    }
    node.shutdown().await;
    Ok(())
}

fn print_event(event: NodeEvent, output_file: Option<&Path>) {
    match event {
        NodeEvent::Listening { addr } => println!("[listen] {addr}"),
        NodeEvent::InviteUpdated { invite } => {
            println!("[invite] {invite}");
            for addr in &invite.addrs {
                println!("[invite-addr] {addr}");
            }
            write_output(output_file, &invite.encode());
        }
        NodeEvent::RelayAddresses { addrs } => {
            for addr in &addrs {
                println!("[relay-addr] {addr}");
            }
            let joined: Vec<String> = addrs.iter().map(|a| a.to_string()).collect();
            write_output(output_file, &joined.join("\n"));
        }
        NodeEvent::RelayReservation { relay } => {
            println!("[relay] reservation accepted by {relay}")
        }
        NodeEvent::Connected { peer, path, addr } => {
            println!("[path] {path} peer={peer} addr={addr}")
        }
        NodeEvent::Disconnected { peer } => println!("[disconnected] {peer}"),
        NodeEvent::RelayReleased { peer } => {
            println!("[path] relay released, now DIRECT only peer={peer}")
        }
        NodeEvent::HolePunch {
            peer,
            success,
            detail,
        } => println!(
            "[holepunch] {} peer={peer} {detail}",
            if success { "OK" } else { "FAILED" }
        ),
        NodeEvent::Joined { server, members } => {
            println!("[joined] \"{server}\" members: {}", members.join(", "))
        }
        NodeEvent::Denied { reason } => println!("[denied] {reason}"),
        NodeEvent::MemberJoined { nick, .. } => println!("[member+] {nick}"),
        NodeEvent::MemberLeft { nick, .. } => println!("[member-] {nick}"),
        NodeEvent::Message { from, text } => println!("[msg] {from}: {text}"),
        NodeEvent::Warning { detail } => println!("[warn] {detail}"),
    }
}

fn write_output(path: Option<&Path>, contents: &str) {
    if let Some(path) = path {
        // Write to a temp file first so readers never see half a code.
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, contents)
            .and_then(|_| std::fs::rename(&tmp, path))
            .is_err()
        {
            eprintln!("could not write {}", path.display());
        }
    }
}

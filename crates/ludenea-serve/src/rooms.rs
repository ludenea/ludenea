//! `ludenea rooms`: the signaling listener of online matches, and optionally a web build
//! served read-only beside it.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::time::Duration;

use ludenea_signal::Limits;

pub struct Options {
    pub page: Option<PathBuf>,
    /// The only address to listen on; by default the machine's network address and
    /// `127.0.0.1`.
    pub listen: Option<IpAddr>,
    pub port: u16,
    pub allowed_origins: Vec<String>,
    pub idle_seconds: u64,
    pub max_rooms: usize,
}

/// Listens, prints where, then serves until the process ends.
pub async fn run(options: Options) -> Result<(), String> {
    if options.port == 0 && options.listen.is_none() {
        return Err(
            "--port 0 needs --listen: the default listens on two addresses, which share one \
             port"
                .into(),
        );
    }
    if let Some(page) = &options.page
        && !page.join("game.json").is_file()
    {
        let dir = page.display();
        return Err(format!(
            "{dir} holds no web build: write one with `ludenea build <project> --web --out {dir}`"
        ));
    }
    let addresses = match options.listen {
        Some(address) => vec![address],
        None => match network_address() {
            Some(address) => vec![address, IpAddr::V4(Ipv4Addr::LOCALHOST)],
            None => {
                eprintln!("no network address found: listening on 127.0.0.1 only");
                vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]
            }
        },
    };
    let mut listeners = Vec::new();
    for address in addresses {
        let listener = tokio::net::TcpListener::bind((address, options.port))
            .await
            .map_err(|error| {
                let address = SocketAddr::new(address, options.port);
                format!("cannot listen on {address}: {error}")
            })?;
        listeners.push(listener);
    }
    let bound = listeners
        .iter()
        .map(|listener| listener.local_addr().map_err(|error| error.to_string()))
        .collect::<Result<Vec<SocketAddr>, String>>()?;
    for address in &bound {
        println!("signaling ws://{address}/rooms");
    }
    if options.page.is_some() {
        for address in &bound {
            println!("page http://{address}/");
        }
    }
    for address in bound.iter().filter(|address| !address.ip().is_loopback()) {
        eprintln!(
            "warning: listening on {address}, which anyone on the network can reach: there is \
             no authentication, and anyone with a room's code can join or disrupt that room"
        );
    }
    let limits = Limits {
        idle: Duration::from_secs(options.idle_seconds),
        max_rooms: options.max_rooms,
        ..Limits::default()
    };
    ludenea_signal::serve(
        listeners,
        ludenea_signal::Options {
            page: options.page,
            limits,
            allowed_origins: options.allowed_origins,
        },
    )
    .await
}

/// The address of this machine on its network: the local address of a UDP socket connected
/// to a documentation address (`192.0.2.1`, which routes like any other). Connecting a UDP
/// socket sends nothing.
fn network_address() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_loopback() && !address.is_unspecified()).then_some(address)
}

use anyhow::{bail, ensure, Context, Result};
use bomberman_net::{invitation::random_token, Invitation, ServerProfile};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    listen: SocketAddr,
    advertised_endpoint: String,
    max_connections: usize,
    max_rooms: usize,
}

#[tokio::main(worker_threads = 2)]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Bomberman relay: {error:#}");
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "--help".into());
    if command == "--help" || command == "-h" {
        println!("bomberman-relay {}\n\nCommands: init, serve, export-invite, rotate-token\n\n  --state <directory>    Private persistent state outside the checkout (required)\n  --advertise <host:port> Destination entered locally (init only; default TLS port 443)\n  --listen <ip:port>     Bind address (init only; default 0.0.0.0:443)\n\ninit creates persistent TLS keys and an access token. export-invite prints a private\nserver invitation; share it privately. rotate-token requires a service restart.\nNo endpoint is built into this program.", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let mut state = None;
    let mut advertise = None;
    let mut listen = "0.0.0.0:443".parse::<SocketAddr>()?;
    while let Some(arg) = args.next() {
        let value = args.next().context("option needs a value")?;
        match arg.as_str() {
            "--state" => state = Some(PathBuf::from(value)),
            "--advertise" if command == "init" => advertise = Some(value),
            "--listen" if command == "init" => {
                listen = value.parse().context("invalid listen address")?
            }
            _ => bail!("unknown option (use --help)"),
        }
    }
    let state = state.context("--state is required")?;
    match command.as_str() {
        "init" => initialize(
            &state,
            advertise.context("init requires --advertise <host:port>")?,
            listen,
        )?,
        "export-invite" => {
            let config = load_config(&state)?;
            let cert = fs::read(state.join("certificate.der"))?;
            let invitation = Invitation {
                server: ServerProfile {
                    endpoint: config.advertised_endpoint,
                    fingerprint: Sha256::digest(cert).into(),
                    token: load_token(&state)?,
                },
                room: None,
            };
            println!("{}", invitation.encode()?);
        }
        "rotate-token" => {
            load_config(&state)?;
            let temporary = state.join(format!(".token-{}", random_token()));
            private_write(&temporary, random_token().as_bytes())?;
            fs::rename(temporary, state.join("access-token"))?;
            fs::File::open(&state)?.sync_all()?;
            println!("Access token rotated. Restart the service and export a new invitation.");
        }
        "serve" => {
            let config = load_config(&state)?;
            let _ = rustls::crypto::ring::default_provider().install_default();
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(
                state.join("certificate.pem"),
                state.join("private-key.pem"),
            )
            .await
            .context("load persistent TLS identity")?;
            let relay = bomberman_relay::Relay::new(
                load_token(&state)?,
                config.max_connections,
                config.max_rooms,
            );
            let maintenance = relay.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(60));
                loop {
                    interval.tick().await;
                    maintenance.broker.lock().unwrap().expire();
                    let (rooms, connections) = maintenance.counts();
                    eprintln!("relay: rooms={rooms} connections={connections}");
                }
            });
            let handle = axum_server::Handle::new();
            let shutdown = handle.clone();
            tokio::spawn(async move {
                #[cfg(unix)]
                {
                    let mut term =
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                            .expect("SIGTERM handler");
                    tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
                }
                #[cfg(not(unix))]
                {
                    let _ = tokio::signal::ctrl_c().await;
                }
                shutdown.graceful_shutdown(Some(Duration::from_secs(2)));
            });
            eprintln!("relay: ready (TLS enabled; private rooms in memory)");
            let acceptor = axum_server::tls_rustls::RustlsAcceptor::new(tls)
                .handshake_timeout(Duration::from_secs(5))
                .acceptor(bomberman_relay::accept::ConnectionLimit::new(
                    config.max_connections,
                ));
            axum_server::bind(config.listen)
                .acceptor(acceptor)
                .handle(handle)
                .serve(
                    relay
                        .router()
                        .into_make_service_with_connect_info::<SocketAddr>(),
                )
                .await?;
        }
        _ => bail!("unknown command; use --help"),
    }
    Ok(())
}
fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn load_config(state: &Path) -> Result<Config> {
    ensure!(
        fs::symlink_metadata(state)?.is_dir(),
        "state must be a real directory"
    );
    let config: Config = serde_json::from_slice(&fs::read(state.join("config.json"))?)
        .map_err(|_| anyhow::anyhow!("invalid private relay configuration"))?;
    ensure!(
        (1..=1024).contains(&config.max_connections) && (1..=256).contains(&config.max_rooms),
        "invalid resource limits"
    );
    Ok(config)
}
fn load_token(state: &Path) -> Result<String> {
    let token = fs::read_to_string(state.join("access-token"))?;
    ensure!(token.len() == 43, "invalid access token file");
    Ok(token)
}
fn initialize(state: &Path, advertise: String, listen: SocketAddr) -> Result<()> {
    ensure!(
        !state.exists(),
        "state directory already exists; refusing to replace persistent keys"
    );
    let key = rcgen::KeyPair::generate()?;
    let params = rcgen::CertificateParams::new(vec!["atomic-relay.invalid".into()])?;
    let cert = params.self_signed(&key)?;
    let config = Config {
        listen,
        advertised_endpoint: format!("wss://{advertise}/v1/ws"),
        max_connections: 128,
        max_rooms: 64,
    };
    let token = random_token();
    Invitation {
        server: ServerProfile {
            endpoint: config.advertised_endpoint.clone(),
            fingerprint: Sha256::digest(cert.der()).into(),
            token: token.clone(),
        },
        room: None,
    }
    .validate()?;
    let parent = state.parent().context("state needs a parent directory")?;
    fs::create_dir_all(parent)?;
    let stage = parent.join(format!(".relay-init-{}", random_token()));
    fs::create_dir(&stage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&stage, fs::Permissions::from_mode(0o700))?;
    }
    let result = (|| -> Result<()> {
        private_write(
            &stage.join("config.json"),
            &serde_json::to_vec_pretty(&config)?,
        )?;
        private_write(&stage.join("certificate.pem"), cert.pem().as_bytes())?;
        private_write(&stage.join("certificate.der"), cert.der())?;
        private_write(
            &stage.join("private-key.pem"),
            key.serialize_pem().as_bytes(),
        )?;
        private_write(&stage.join("access-token"), token.as_bytes())?;
        fs::rename(&stage, state)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(stage);
    }
    result?;
    println!("Initialized persistent TLS identity and access token. Use export-invite to get the private invitation.");
    Ok(())
}

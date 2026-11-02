use std::collections::HashSet;
use std::future::Future;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use axum_server::tls_openssl::OpenSSLAcceptor;
use clap::{Args, Parser, Subcommand, ValueEnum};
use futures::{stream::FuturesUnordered, StreamExt};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use one_kvm::config;
use one_kvm::instances::{self, InstanceHost, InstanceId};
use one_kvm::platform::PlatformCapabilities;
use one_kvm::runtime::WebConfigOverrides;
use one_kvm::state::ShutdownAction;
use one_kvm::utils::bind_tcp_listener;
use one_kvm::utils::tls::server_config_from_pem_file;

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

#[derive(Parser, Debug)]
#[command(name = "one-kvm")]
#[command(version, about = "XSCS-authorized hardware KVM controlled host for XSCC", long_about = None)]
struct CliArgs {
    /// Local XSCS instance management commands
    #[command(subcommand)]
    command: Option<CliCommand>,

    /// Local controlled instance selected by management commands.
    #[arg(long, global = true, default_value = "default")]
    instance: InstanceId,

    /// Listen address (overrides database config)
    #[arg(short = 'a', long, value_name = "ADDRESS")]
    address: Option<String>,

    /// HTTP port (overrides database config)
    #[arg(
        short = 'p',
        long = "port",
        visible_alias = "http-port",
        value_name = "PORT"
    )]
    http_port: Option<u16>,

    /// HTTPS port (overrides database config)
    #[arg(long, value_name = "PORT")]
    https_port: Option<u16>,

    /// Enable HTTPS (overrides database config)
    #[arg(long)]
    enable_https: bool,

    /// Path to SSL certificate file (generates self-signed if not provided)
    #[arg(long, value_name = "FILE", requires = "ssl_key")]
    ssl_cert: Option<PathBuf>,

    /// Path to SSL private key file
    #[arg(long, value_name = "FILE", requires = "ssl_cert")]
    ssl_key: Option<PathBuf>,

    /// Data directory path
    #[arg(short = 'd', long, value_name = "DIR")]
    data_dir: Option<PathBuf>,

    /// Log level (error, warn, info, debug, trace)
    #[arg(short = 'l', long, value_name = "LEVEL", default_value = "info")]
    log_level: LogLevel,
}

#[derive(Subcommand, Debug)]
enum CliCommand {
    /// Bind or maintain this XSCC KVM controlled instance locally.
    Management(ManagementCommand),
}

#[derive(Args, Debug)]
struct ManagementCommand {
    #[command(subcommand)]
    action: ManagementAction,
}

#[derive(Subcommand, Debug)]
enum ManagementAction {
    /// Create an independent controlled instance, then restart the host to load it.
    Create {
        #[arg(long)]
        native_port: Option<u16>,
    },
    /// Change an instance native listener port while the host is stopped.
    SetPort {
        #[arg(long)]
        native_port: u16,
    },
    /// List all locally configured instances and their Web/native endpoints.
    List,
    /// Bind the controlled instance using an XSCS authorization code file.
    Bind {
        #[arg(long)]
        server: String,
        #[arg(long)]
        code_file: PathBuf,
        /// Reachable native XSCC endpoint, for example kvm.example:21118.
        #[arg(long = "address", required = true)]
        addresses: Vec<String>,
    },
    Unbind,
    Status,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = CliArgs::parse();

    init_logging(args.log_level);

    tracing::info!("Starting One-KVM v{}", env!("CARGO_PKG_VERSION"));
    let platform = PlatformCapabilities::current();
    tracing::info!(
        "Platform mode: {:?} ({})",
        platform.mode,
        platform.mode_label
    );

    let data_dir = args.data_dir.clone().unwrap_or_else(get_data_dir);
    tracing::info!("Data directory: {}", data_dir.display());

    if let Some(command) = args.command {
        run_cli_command(command, data_dir, args.instance).await?;
        return Ok(());
    }

    let overrides = WebConfigOverrides {
        address: args.address,
        http_port: args.http_port,
        https_port: args.https_port,
        enable_https: args.enable_https,
        ssl_cert: args.ssl_cert,
        ssl_key: args.ssl_key,
    };
    let mut host = InstanceHost::build(&data_dir, overrides).await?;
    let config = host.default_runtime().config().clone();

    let bind_ips = resolve_bind_addresses(&config.web)?;
    let scheme = if config.web.https_enabled {
        "https"
    } else {
        "http"
    };
    let bind_port = if config.web.https_enabled {
        config.web.https_port
    } else {
        config.web.http_port
    };

    for ip in &bind_ips {
        let addr = SocketAddr::new(*ip, bind_port);
        tracing::info!("Server will listen on: {}://{}", scheme, addr);
    }

    let app = host.router();

    let listeners = bind_tcp_listeners(&bind_ips, bind_port)?;

    let shutdown_signal = {
        let mut requests = FuturesUnordered::new();
        for mut receiver in host.shutdown_receivers() {
            requests.push(async move { receiver.recv().await });
        }
        async move {
            tokio::select! {
                result = shutdown_signal() => {
                    if let Err(error) = result {
                        tracing::error!(%error, "Failed while waiting for shutdown signal");
                    }
                    ShutdownAction::Exit
                }
                request = requests.next() => {
                    match request {
                        Some(Ok(action)) => action,
                        _ => ShutdownAction::Exit,
                    }
                }
            }
        }
    };

    let shutdown_action = if config.web.https_enabled {
        let tls_config = if let (Some(cert_path), Some(key_path)) =
            (&config.web.ssl_cert_path, &config.web.ssl_key_path)
        {
            server_config_from_pem_file(cert_path, key_path).await?
        } else {
            let cert_dir = data_dir.join("certs");
            let cert_path = cert_dir.join("server.crt");
            let key_path = cert_dir.join("server.key");

            if !cert_path.exists() || !key_path.exists() {
                tracing::info!("Generating new self-signed TLS certificate");
                let cert = generate_self_signed_cert()?;
                tokio::fs::create_dir_all(&cert_dir).await?;
                tokio::fs::write(&cert_path, cert.cert.pem()).await?;
                tokio::fs::write(&key_path, cert.signing_key.serialize_pem()).await?;
            } else {
                tracing::info!("Using existing TLS certificate from {}", cert_dir.display());
            }

            server_config_from_pem_file(&cert_path, &key_path).await?
        };

        let servers = FuturesUnordered::new();
        for listener in listeners {
            let local_addr = listener.local_addr()?;
            tracing::info!("Starting HTTPS server on {}", local_addr);

            let server = axum_server::from_tcp(listener)?
                .acceptor(OpenSSLAcceptor::new(tls_config.clone()))
                .serve(app.clone().into_make_service());
            servers.push(server);
        }

        run_servers_until_shutdown(servers, shutdown_signal, "HTTPS").await
    } else {
        let servers = FuturesUnordered::new();
        for listener in listeners {
            let local_addr = listener.local_addr()?;
            tracing::info!("Starting HTTP server on {}", local_addr);

            let listener = tokio::net::TcpListener::from_std(listener)?;
            let server = axum::serve(listener, app.clone());
            servers.push(async move { server.await });
        }

        run_servers_until_shutdown(servers, shutdown_signal, "HTTP").await
    };

    host.shutdown().await;
    drop(host);
    tracing::info!("Server shutdown complete");
    if let ShutdownAction::Restart { exe_path } = shutdown_action {
        restart_current_process(exe_path)?;
    }
    Ok(())
}

fn init_logging(level: LogLevel) {
    let app_level = match level {
        LogLevel::Error => "error",
        LogLevel::Warn => "warn",
        LogLevel::Info => "info",
        LogLevel::Debug => "debug",
        LogLevel::Trace => "trace",
    };
    let env_filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| app_level.into());

    if let Err(err) = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .try_init()
    {
        eprintln!("failed to initialize tracing: {}", err);
    }
}

fn get_data_dir() -> PathBuf {
    if let Ok(path) = std::env::var("ONE_KVM_DATA_DIR") {
        return PathBuf::from(path);
    }

    #[cfg(windows)]
    {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                return exe_dir.join("one-kvm");
            }
        }
        return std::env::current_dir()
            .map(|dir| dir.join("one-kvm"))
            .unwrap_or_else(|_| PathBuf::from("one-kvm"));
    }

    #[cfg(not(windows))]
    PathBuf::from("/etc/one-kvm")
}

#[cfg(unix)]
async fn shutdown_signal() -> anyhow::Result<()> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result?,
        _ = terminate.recv() => {},
    }
    Ok(())
}

#[cfg(not(unix))]
async fn shutdown_signal() -> anyhow::Result<()> {
    tokio::signal::ctrl_c().await?;
    Ok(())
}

async fn run_servers_until_shutdown<F, E>(
    mut servers: FuturesUnordered<F>,
    shutdown_signal: impl Future<Output = ShutdownAction>,
    protocol: &'static str,
) -> ShutdownAction
where
    F: Future<Output = Result<(), E>> + Send,
    E: std::fmt::Display,
{
    tokio::select! {
        action = shutdown_signal => {
            action
        }
        result = servers.next() => {
            if let Some(Err(e)) = result {
                tracing::error!("{} server error: {}", protocol, e);
            }
            ShutdownAction::Exit
        }
    }
}

fn restart_current_process(exe_path: Option<PathBuf>) -> anyhow::Result<()> {
    let exe = exe_path.unwrap_or(std::env::current_exe()?);
    let args: Vec<String> = std::env::args().skip(1).collect();

    tracing::info!("Restarting: {:?} {:?}", exe, args);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&exe).args(&args).exec();
        Err(anyhow::anyhow!("Failed to restart: {}", err))
    }

    #[cfg(not(unix))]
    {
        std::process::Command::new(&exe).args(&args).spawn()?;
        std::process::exit(0);
    }
}

async fn run_cli_command(
    command: CliCommand,
    root: PathBuf,
    instance: InstanceId,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(&root)?;
    let CliCommand::Management(command) = command;
    if matches!(&command.action, ManagementAction::List) {
        println!(
            "{}",
            serde_json::to_string_pretty(&instances::summaries(&root).await?)?
        );
        return Ok(());
    }
    let _host_lock = instances::lock_host(&root).map_err(|error| {
        anyhow::anyhow!(
            "Stop the One-KVM host before changing or opening instance management: {error}"
        )
    })?;
    let action = match command.action {
        ManagementAction::Create { native_port } => {
            let created = instances::create_instance(&root, &instance, native_port).await?;
            println!("{}", serde_json::to_string_pretty(&created)?);
            return Ok(());
        }
        ManagementAction::SetPort { native_port } => {
            let updated = instances::set_native_port(&root, &instance, native_port).await?;
            println!("{}", serde_json::to_string_pretty(&updated)?);
            return Ok(());
        }
        ManagementAction::List => {
            println!(
                "{}",
                serde_json::to_string_pretty(&instances::summaries(&root).await?)?
            );
            return Ok(());
        }
        action => action,
    };
    let data_dir = instance.data_dir(&root);
    if instance != InstanceId::default()
        && !instances::discover(&root)?
            .iter()
            .any(|entry| entry.id == instance)
    {
        anyhow::bail!("Instance {instance} does not exist; create it with management create first");
    }
    let managed = one_kvm_management::ManagedKvm::open(&data_dir).await?;
    let result: anyhow::Result<()> = async {
        match action {
            ManagementAction::Bind {
                server,
                code_file,
                addresses,
            } => {
                let mut code = String::new();
                std::fs::File::open(code_file)?
                    .take(128)
                    .read_to_string(&mut code)?;
                let addresses = addresses
                    .iter()
                    .map(|value| native_address(value))
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let status = managed.bind_device(&server, code.trim(), addresses).await?;
                println!("{}", serde_json::to_string_pretty(&status)?);
            }
            ManagementAction::Create { .. }
            | ManagementAction::SetPort { .. }
            | ManagementAction::List => {
                unreachable!("invariant: catalog commands returned before opening management")
            }
            ManagementAction::Unbind => managed.unbind_device().await?,
            ManagementAction::Status => println!(
                "{}",
                serde_json::to_string_pretty(&managed.device_status())?
            ),
        }
        Ok(())
    }
    .await;
    let stopped = managed.shutdown().await;
    result?;
    stopped?;
    Ok(())
}

fn native_address(value: &str) -> anyhow::Result<one_kvm_management::HostAddress> {
    let (host, port) = match value.parse::<SocketAddr>() {
        Ok(address) => (address.ip().to_string(), address.port()),
        Err(_) => {
            let (host, port) = value
                .rsplit_once(':')
                .ok_or_else(|| anyhow::anyhow!("Native address requires host:port"))?;
            (host.to_ascii_lowercase(), port.parse()?)
        }
    };
    let address = one_kvm_management::HostAddress { host, port };
    address
        .validate()
        .map_err(|_| anyhow::anyhow!("Invalid native XSCC address"))?;
    Ok(address)
}

fn resolve_bind_addresses(web: &config::WebConfig) -> anyhow::Result<Vec<IpAddr>> {
    let raw_addrs = if !web.bind_addresses.is_empty() {
        web.bind_addresses.as_slice()
    } else {
        std::slice::from_ref(&web.bind_address)
    };

    let mut seen = HashSet::new();
    let mut addrs = Vec::new();
    for addr in raw_addrs {
        let ip: IpAddr = addr
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid bind address: {}", addr))?;
        if seen.insert(ip) {
            addrs.push(ip);
        }
    }

    Ok(addrs)
}

fn bind_tcp_listeners(addrs: &[IpAddr], port: u16) -> anyhow::Result<Vec<std::net::TcpListener>> {
    let mut listeners = Vec::new();
    for ip in addrs {
        let addr = SocketAddr::new(*ip, port);
        match bind_tcp_listener(addr) {
            Ok(listener) => listeners.push(listener),
            Err(err) => {
                tracing::warn!("Failed to bind {}: {}", addr, err);
            }
        }
    }

    if listeners.is_empty() {
        anyhow::bail!("Failed to bind any addresses on port {}", port);
    }

    Ok(listeners)
}

fn generate_self_signed_cert() -> anyhow::Result<rcgen::CertifiedKey<rcgen::KeyPair>> {
    use rcgen::generate_simple_self_signed;

    let subject_alt_names = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];

    let certified_key = generate_simple_self_signed(subject_alt_names)?;
    Ok(certified_key)
}

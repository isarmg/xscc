use std::sync::atomic::{AtomicBool, Ordering};

use base::config::keys;

use hbb_common::{
    allow_err,
    config::{self, Config},
    log, sleep, tokio,
};

use crate::server::{check_zombie, new as new_server, ConnectionMeta, ServerPtr};

static RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);
static DIRECT_SERVER_RUNNING: AtomicBool = AtomicBool::new(false);
const DEFAULT_DIRECT_PORT: u16 = 21118;

pub struct DirectServer;

impl DirectServer {
    pub fn restart() {
        RESTART_REQUESTED.store(true, Ordering::SeqCst);
        log::info!("Direct server restart requested");
    }

    pub async fn start_all() {
        if config::is_outgoing_only() {
            std::future::pending::<()>().await;
            return;
        }
        #[cfg(all(target_os = "windows", not(feature = "gpui-host")))]
        if crate::platform::is_installed() && crate::is_server() {
            crate::updater::start_auto_update();
        }
        check_zombie();
        let server = new_server();
        #[cfg(not(target_os = "ios"))]
        std::thread::spawn(move || {
            allow_err!(super::lan::start_listening());
        });
        scrap::codec::test_av1();
        #[cfg(all(feature = "experimental-quic", not(any(target_os = "android", target_os = "ios"))))]
        tokio::spawn(crate::quic_host::run(server.clone()));
        direct_server(server).await;
    }
}

pub(crate) fn get_direct_port() -> u16 {
    Config::get_option(keys::OPTION_DIRECT_ACCESS_PORT)
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .unwrap_or(DEFAULT_DIRECT_PORT)
}

pub fn direct_server_running() -> bool {
    DIRECT_SERVER_RUNNING.load(Ordering::Acquire)
}

async fn direct_server(server: ServerPtr) {
    let mut listener = None;
    #[cfg(feature = "udp-media")]
    let mut _udp_listener = None;
    let mut port = 0;
    loop {
        #[cfg(feature = "udp-media")]
        if _udp_listener.as_ref().is_some_and(crate::udp_host::HostListener::is_finished) {
            log::warn!("Restarting direct listener after UDP media receive task stopped");
            listener = None;
            _udp_listener = None;
            DIRECT_SERVER_RUNNING.store(false, Ordering::Release);
        }
        let disabled = config::is_outgoing_only()
            || config::is_disable_tcp_listen()
            || Config::get_bool_option("stop-service")
            || crate::platform::installing_service();
        let requested_port = get_direct_port();
        let restarted = RESTART_REQUESTED.swap(false, Ordering::SeqCst);
        if disabled || port != requested_port || restarted {
            listener = None;
            #[cfg(feature = "udp-media")]
            { _udp_listener = None; }
            DIRECT_SERVER_RUNNING.store(false, Ordering::Release);
        }
        if disabled {
            server.write().unwrap().close_connections();
            sleep(1.).await;
            continue;
        }
        if listener.is_none() {
            port = requested_port;
            match hbb_common::tcp::listen_any(port).await {
                Ok(bound) => {
                    #[cfg(feature = "udp-media")]
                    match crate::udp_host::bind(port) {
                        Ok(udp) => {
                            match udp.local_addr() {
                                Ok(address) => log::info!("Encrypted UDP media listening on {address}"),
                                Err(error) => log::warn!("Unable to inspect UDP listener address: {error}"),
                            }
                            _udp_listener = Some(udp);
                        },
                        Err(error) => {
                            hbb_common::throttled_log!(
                                std::time::Duration::from_secs(60),
                                error,
                                "Unable to bind authenticated UDP media on port {port}: {error}"
                            );
                            drop(bound);
                            sleep(1.).await;
                            continue;
                        }
                    }
                    log::info!("Direct server listening on port {}", port);
                    listener = Some(bound);
                    DIRECT_SERVER_RUNNING.store(true, Ordering::Release);
                }
                Err(err) => {
                    hbb_common::throttled_log!(
                        std::time::Duration::from_secs(60),
                        error,
                        "Failed to start direct server on port {}: {}",
                        port,
                        err
                    );
                    sleep(1.).await;
                    continue;
                }
            }
        }
        let Some(bound) = listener.as_mut() else {
            continue;
        };
        match hbb_common::timeout(1000, bound.accept()).await {
            Ok(Ok((stream, addr))) => {
                allow_err!(stream.set_nodelay(true));
                let local_addr = match stream.local_addr() {
                    Ok(addr) => addr,
                    Err(err) => {
                        log::trace!("Failed to get direct connection's local address: {}", err);
                        continue;
                    }
                };
                let server = server.clone();
                tokio::spawn(async move {
                    if let Err(err) = crate::server::create_tcp_connection(
                        server,
                        hbb_common::Stream::from(stream, local_addr),
                        addr,
                        ConnectionMeta::default(),
                    )
                    .await
                    {
                        hbb_common::throttled_log!(
                            std::time::Duration::from_secs(60),
                            warn,
                            "Direct connection from {} failed: {}",
                            addr,
                            err
                        );
                    }
                });
            }
            Ok(Err(err)) => {
                hbb_common::throttled_log!(
                    std::time::Duration::from_secs(60),
                    warn,
                    "Direct server accept failed: {}",
                    err
                );
                sleep(0.1).await;
            }
            Err(_) => {}
        }
    }
}

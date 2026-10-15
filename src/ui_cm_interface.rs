#[cfg(not(any(target_os = "android", target_os = "ios")))]
use crate::ipc::Connection;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use crate::ipc::{self, Data};
#[cfg(target_os = "windows")]
use crate::ipc::ClipboardNonFile;
use base::config::keys::OPTION_ENABLE_PERM_CHANGE_IN_ACCEPT_WINDOW;
use hbb_common::{
    allow_err,
    config::option2bool,
    log,
    tokio::{
        self,
        sync::mpsc::{self, UnboundedSender},
    },
};
use serde_derive::Serialize;
#[cfg(any(target_os = "android", target_os = "ios", feature = "flutter"))]
use std::iter::FromIterator;
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicI64, Ordering},
        RwLock,
    },
};

#[derive(Serialize, Clone)]
pub struct Client {
    pub id: i32,
    pub authorized: bool,
    pub disconnected: bool,
    pub name: String,
    pub avatar: String,
    pub peer_id: String,
    pub keyboard: bool,
    pub clipboard: bool,
    pub audio: bool,
    pub restart: bool,
    pub block_input: bool,
    pub privacy_mode: bool,
    #[serde(skip)]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    tx: UnboundedSender<Data>,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
struct IpcTaskRunner<T: InvokeUiCM> {
    stream: Connection,
    cm: ConnectionManager<T>,
    tx: mpsc::UnboundedSender<Data>,
    rx: mpsc::UnboundedReceiver<Data>,
    close: bool,
    running: bool,
    conn_id: i32,

}

lazy_static::lazy_static! {
    static ref CLIENTS: RwLock<HashMap<i32, Client>> = Default::default();
}

static CLICK_TIME: AtomicI64 = AtomicI64::new(0);

#[derive(Clone)]
pub struct ConnectionManager<T: InvokeUiCM> {
    pub ui_handler: T,
}

pub trait InvokeUiCM: Send + Clone + 'static + Sized {
    fn add_connection(&self, client: &Client);

    fn remove_connection(&self, id: i32, close: bool);


    fn change_theme(&self, dark: String);

    fn change_language(&self);

    fn show_elevation(&self, show: bool);


}

impl<T: InvokeUiCM> Deref for ConnectionManager<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.ui_handler
    }
}

impl<T: InvokeUiCM> DerefMut for ConnectionManager<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ui_handler
    }
}

impl<T: InvokeUiCM> ConnectionManager<T> {
    fn add_connection(
        &self,
        id: i32,
        peer_id: String,
        name: String,
        avatar: String,
        authorized: bool,
        keyboard: bool,
        clipboard: bool,
        audio: bool,
        restart: bool,
        block_input: bool,
        privacy_mode: bool,
        #[cfg(not(any(target_os = "android", target_os = "ios")))] tx: mpsc::UnboundedSender<Data>,
    ) {
        let client = Client {
            id,
            authorized,
            disconnected: false,
            name: name.clone(),
            avatar,
            peer_id: peer_id.clone(),
            keyboard,
            clipboard,
            audio,
            restart,
            block_input,
            privacy_mode,
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            tx,
        };
        CLIENTS
            .write()
            .unwrap()
            .retain(|_, c| !(c.disconnected && c.peer_id == client.peer_id));
        CLIENTS.write().unwrap().insert(id, client.clone());
        self.ui_handler.add_connection(&client);
    }

    fn remove_connection(&self, id: i32, close: bool) {
        if close {
            CLIENTS.write().unwrap().remove(&id);
        } else {
            CLIENTS
                .write()
                .unwrap()
                .get_mut(&id)
                .map(|c| c.disconnected = true);
        }

        self.ui_handler.remove_connection(id, close);
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn show_elevation(&self, show: bool) {
        self.ui_handler.show_elevation(show);
    }

}

#[inline]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn check_click_time(id: i32) {
    if let Some(client) = CLIENTS.read().unwrap().get(&id) {
        allow_err!(client.tx.send(Data::ClickTime(0)));
    };
}

#[inline]
pub fn get_click_time() -> i64 {
    CLICK_TIME.load(Ordering::SeqCst)
}

#[inline]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn authorize(id: i32) {
    if let Some(client) = CLIENTS.write().unwrap().get_mut(&id) {
        client.authorized = true;
        allow_err!(client.tx.send(Data::Authorize));
    };
}

#[inline]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn close(id: i32) {
    if let Some(client) = CLIENTS.read().unwrap().get(&id) {
        allow_err!(client.tx.send(Data::Close));
    };
}

/// Like `close`, but says the CM's WINDOW closed rather than a person disconnecting this peer.
/// See `ipc::Data::CmWindowClosed`.
#[cfg(target_os = "linux")]
pub fn close_window(id: i32) {
    if let Some(client) = CLIENTS.read().unwrap().get(&id) {
        allow_err!(client.tx.send(Data::CmWindowClosed));
    };
}

#[inline]
pub fn remove(id: i32) {
    CLIENTS.write().unwrap().remove(&id);
}

#[inline]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn switch_permission(id: i32, name: String, enabled: bool) {
    #[cfg(target_os = "android")]
    let is_keyboard_permission = name == "keyboard";
    #[cfg(not(target_os = "android"))]
    let is_keyboard_permission = false;
    if !option2bool(
        OPTION_ENABLE_PERM_CHANGE_IN_ACCEPT_WINDOW,
        &crate::get_builtin_option(OPTION_ENABLE_PERM_CHANGE_IN_ACCEPT_WINDOW),
    ) && !is_keyboard_permission
    {
        log::info!(
            "blocked cm switch_permission by policy, conn_id={}, permission={}, enabled={}",
            id,
            name,
            enabled
        );
        return;
    }
    if let Some(client) = CLIENTS.read().unwrap().get(&id) {
        allow_err!(client.tx.send(Data::SwitchPermission { name, enabled }));
    };
}

#[cfg(any(target_os = "android", target_os = "ios", feature = "flutter"))]
#[inline]
pub fn get_clients_state() -> String {
    let clients = CLIENTS.read().unwrap();
    let res = Vec::from_iter(clients.values().cloned());
    serde_json::to_string(&res).unwrap_or("".into())
}

#[inline]
pub fn get_clients_length() -> usize {
    let clients = CLIENTS.read().unwrap();
    clients.len()
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl<T: InvokeUiCM> IpcTaskRunner<T> {
    async fn run(&mut self) {
        use hbb_common::config::LocalConfig;
        self.running = false;
        loop {
            tokio::select! {
                res = self.stream.next() => {
                    match res {
                        Err(err) => {
                            log::info!("cm ipc connection closed: {}", err);
                            break;
                        }
                        Ok(Some(data)) => {
                            match data {
                                Data::Login{id, peer_id, name, avatar, authorized, keyboard, clipboard, audio, restart, block_input, privacy_mode} => {
                                    log::debug!("conn_id: {}", id);
                                    self.cm.add_connection(id, peer_id, name, avatar, authorized, keyboard, clipboard, audio, restart, block_input, privacy_mode, self.tx.clone());
                                    self.conn_id = id;
                                    self.running = true;
                                    break;
                                }
                                Data::Close => {
                                    log::info!("cm ipc connection closed from connection request");
                                    break;
                                }
                                Data::Disconnected => {
                                    self.close = false;
                                    log::info!("cm ipc connection disconnect");
                                    break;
                                }
                                Data::PrivacyModeState((_id, _, _)) => {
                                    #[cfg(windows)]
                                    cm_inner_send(_id, data);
                                }
                                Data::ClickTime(ms) => {
                                    CLICK_TIME.store(ms, Ordering::SeqCst);
                                }

                                Data::SwitchPermission { name, enabled } => {
                                    // Keep this branch scoped to privacy mode rollback.
                                    // Other CM permission toggles are updated optimistically by the UI itself.
                                    // The backend currently sends SwitchPermission back to CM only when
                                    // privacy-mode turn-off fails and the UI state must be restored.
                                    if name == "privacy_mode" {
                                        let client = {
                                            let mut clients = CLIENTS.write().unwrap();
                                            clients.get_mut(&self.conn_id).map(|c| {
                                                c.privacy_mode = enabled;
                                                c.clone()
                                            })
                                        };
                                        if let Some(client) = client {
                                            // This reuses add_connection(), and cm.tis only selectively updates
                                            // existing rows (authorized/privacy_mode) for this fallback path.
                                            self.cm.ui_handler.add_connection(&client);
                                        }
                                    }
                                }

                                Data::Theme(dark) => {
                                    self.cm.change_theme(dark);
                                }
                                Data::Language(lang) => {
                                    LocalConfig::set_option("lang".to_owned(), lang);
                                    self.cm.change_language();
                                }
                                Data::DataPortableService(ipc::DataPortableService::CmShowElevation(show)) => {
                                    self.cm.show_elevation(show);
                                }

                                #[cfg(target_os = "windows")]
                                Data::ClipboardNonFile(_) => {
                                    match crate::clipboard::check_clipboard_cm() {
                                        Ok(multi_clipoards) => {
                                            let mut raw_contents = bytes::BytesMut::new();
                                            let mut main_data = vec![];
                                            for c in multi_clipoards.clipboards.into_iter() {
                                                let content_len = c.content.len();
                                                let (content, next_raw) = {
                                                    // TODO: find out a better threshold
                                                    if content_len > 1024 * 3 {
                                                        raw_contents.extend(c.content);
                                                        (bytes::Bytes::new(), true)
                                                    } else {
                                                        (c.content, false)
                                                    }
                                                };
                                                main_data.push(ClipboardNonFile {
                                                    compress: c.compress,
                                                    content,
                                                    content_len,
                                                    next_raw,
                                                    width: c.width,
                                                    height: c.height,
                                                    format: c.format.value(),
                                                    special_name: c.special_name,
                                                });
                                            }
                                            allow_err!(self.stream.send(&Data::ClipboardNonFile(Some(("".to_owned(), main_data)))).await);
                                            if !raw_contents.is_empty() {
                                                allow_err!(self.stream.send_raw(raw_contents.into()).await);
                                            }
                                        }
                                        Err(e) => {
                                            log::debug!("Failed to get clipboard content. {}", e);
                                            allow_err!(self.stream.send(&Data::ClipboardNonFile(Some((format!("{}", e), vec![])))).await);
                                        }
                                    }
                                }
                                _ => {

                                }
                            }
                        }
                        _ => {}
                    }
                }
                Some(data) = self.rx.recv() => {
                    if let Err(e) = self.stream.send(&data).await {
                        log::error!("error encountered in IPC task, quitting: {}", e);
                        break;
                    }
                    match &data {
                        Data::Authorize => {
                            self.running = true;
                            break;
                        }
                        _ => {
                        }
                    }
                },

            }
        }
    }

    async fn ipc_task(stream: Connection, cm: ConnectionManager<T>) {
        log::debug!("ipc task begin");
        let (tx, rx) = mpsc::unbounded_channel::<Data>();
        let mut task_runner = Self {
            stream,
            cm,
            tx,
            rx,
            close: true,
            running: true,
            conn_id: 0,
        };

        while task_runner.running {
            task_runner.run().await;
        }
        if task_runner.conn_id > 0 {
            task_runner
                .cm
                .remove_connection(task_runner.conn_id, task_runner.close);
        }
        log::debug!("ipc task end");
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tokio::main(flavor = "current_thread")]
pub async fn start_ipc<T: InvokeUiCM>(cm: ConnectionManager<T>) {
    match ipc::new_listener("_cm").await {
        Ok(mut incoming) => {
            while let Some(result) = incoming.next().await {
                match result {
                    Ok(stream) => {
                        log::debug!("Got new connection");
                        tokio::spawn(IpcTaskRunner::<T>::ipc_task(
                            Connection::new(stream),
                            cm.clone(),
                        ));
                    }
                    Err(err) => {
                        log::error!("Couldn't get cm client: {:?}", err);
                    }
                }
            }
        }
        Err(err) => {
            log::error!("Failed to start cm ipc server: {}", err);
        }
    }
    quit_cm();
}

#[cfg(windows)]
fn cm_inner_send(id: i32, data: Data) {
    let lock = CLIENTS.read().unwrap();
    if id != 0 {
        if let Some(s) = lock.get(&id) {
            allow_err!(s.tx.send(data));
        }
    } else {
        for s in lock.values() {
            allow_err!(s.tx.send(data.clone()));
        }
    }
}

pub fn can_elevate() -> bool {
    #[cfg(windows)]
    return !crate::platform::is_installed();
    #[cfg(not(windows))]
    return false;
}

pub fn elevate_portable(_id: i32) {
    #[cfg(windows)]
    {
        let lock = CLIENTS.read().unwrap();
        if let Some(s) = lock.get(&_id) {
            allow_err!(s.tx.send(ipc::Data::DataPortableService(
                ipc::DataPortableService::RequestStart
            )));
        }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn quit_cm() {
    // in case of std::process::exit not work
    log::info!("quit cm");
    CLIENTS.write().unwrap().clear();
    // `quit_gui()` ends the process on Windows and macOS, but on Linux it calls
    // `gtk_main_quit()`, which has no effect in the Flutter connection manager:
    // `flutter/linux/main.cc` runs `g_application_run()` (GtkApplication), so
    // `gtk_main()` is never called. Exit directly instead, otherwise this
    // process keeps running while no longer serving the `_cm` ipc endpoint, so
    // the server can't reuse it and spawns one more connection manager.
    //
    // NOTE: a client merely disconnecting does not come here, the Flutter side
    // closes the window then, so this is a fallback rather than an explanation
    // for the stale processes of #15698.
    #[cfg(all(target_os = "linux", feature = "flutter"))]
    std::process::exit(0);
    #[cfg(not(all(target_os = "linux", feature = "flutter")))]
    crate::platform::quit_gui();
}

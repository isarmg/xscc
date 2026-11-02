use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use std::{fmt::Display, future::Future, sync::Arc, time::Duration};
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::{debug, info, warn};

use crate::events::SystemEvent;
use crate::state::AppState;

const EVENT_QUEUE_CAPACITY: usize = 256;
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn send_while_authorized<E: Display>(
    grant: &crate::management::AccessGrant,
    send: impl Future<Output = Result<(), E>>,
) -> bool {
    send_until_cancelled(send, grant.cancelled()).await
}

async fn send_until_cancelled<E: Display>(
    send: impl Future<Output = Result<(), E>>,
    cancelled: impl Future<Output = ()>,
) -> bool {
    tokio::select! {
        biased;
        _ = cancelled => false,
        result = tokio::time::timeout(SEND_TIMEOUT, send) => match result {
            Ok(Ok(())) => true,
            Ok(Err(error)) => {
                debug!("WebSocket send failed: {error}");
                false
            }
            Err(_) => {
                warn!("WebSocket send exceeded its deadline; disconnecting slow client");
                false
            }
        },
    }
}

fn event_channel() -> (mpsc::Sender<BusMessage>, mpsc::Receiver<BusMessage>) {
    mpsc::channel(EVENT_QUEUE_CAPACITY)
}

enum BusMessage {
    Event(SystemEvent),
    Lagged { topic: String, count: u64 },
}

fn normalize_topics(topics: &[String]) -> Vec<String> {
    let mut normalized = topics.to_vec();
    normalized.sort();
    normalized.dedup();

    if normalized.iter().any(|topic| topic == "*") {
        return vec!["*".to_string()];
    }

    normalized
        .into_iter()
        .filter(|topic| {
            if topic.ends_with(".*") {
                return true;
            }

            let Some((prefix, _)) = topic.split_once('.') else {
                return true;
            };

            let wildcard = format!("{}.*", prefix);
            !topics.iter().any(|candidate| candidate == &wildcard)
        })
        .collect()
}

fn is_device_info_topic(topic: &str) -> bool {
    matches!(topic, "*" | "system.*" | "system.device_info")
}

fn is_stream_state_topic(topic: &str) -> bool {
    matches!(topic, "*" | "stream.*" | "stream.state_changed")
}

fn rebuild_event_tasks(
    state: &Arc<AppState>,
    topics: &[String],
    replay_stream_state: bool,
    event_tx: &mpsc::Sender<BusMessage>,
    event_tasks: &mut Vec<JoinHandle<()>>,
) -> bool {
    for task in event_tasks.drain(..) {
        task.abort();
    }

    let topics = normalize_topics(topics);
    let mut device_info_task_added = false;
    let mut stream_state_snapshot_added = false;
    for topic in topics {
        if replay_stream_state && is_stream_state_topic(&topic) && !stream_state_snapshot_added {
            if let Some(snapshot) = state.events.latest_video_stream_state() {
                if event_tx.try_send(BusMessage::Event(snapshot)).is_err() {
                    return false;
                }
            }
            stream_state_snapshot_added = true;
        }

        if is_device_info_topic(&topic) && !device_info_task_added {
            let state = state.clone();
            let mut rx = state.subscribe_device_info();
            let event_tx = event_tx.clone();
            event_tasks.push(tokio::spawn(async move {
                let snapshot = state.get_device_info().await;
                if event_tx.send(BusMessage::Event(snapshot)).await.is_err() {
                    return;
                }

                loop {
                    if rx.changed().await.is_err() {
                        break;
                    }

                    let snapshot = rx.borrow().clone();
                    if let Some(snapshot) = snapshot {
                        if event_tx.send(BusMessage::Event(snapshot)).await.is_err() {
                            break;
                        }
                    }
                }
            }));
            device_info_task_added = true;
        }

        if is_device_info_topic(&topic) && topic != "*" {
            continue;
        }

        let Some(mut rx) = state.events.subscribe_topic(&topic) else {
            warn!("Client subscribed to unknown topic: {}", topic);
            continue;
        };

        let event_tx = event_tx.clone();
        let topic_name = topic.clone();
        event_tasks.push(tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if event_tx.send(BusMessage::Event(event)).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                        if event_tx
                            .send(BusMessage::Lagged {
                                topic: topic_name.clone(),
                                count,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }));
    }
    true
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "payload")]
enum ClientMessage {
    #[serde(rename = "subscribe")]
    Subscribe { topics: Vec<String> },

    #[serde(rename = "unsubscribe")]
    Unsubscribe { topics: Vec<String> },

    #[serde(rename = "ping")]
    Ping,
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    axum::Extension(grant): axum::Extension<crate::management::AccessGrant>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state, grant))
}

async fn handle_socket(
    socket: WebSocket,
    state: Arc<AppState>,
    grant: crate::management::AccessGrant,
) {
    let (mut sender, mut receiver) = socket.split();
    let (event_tx, mut event_rx) = event_channel();
    let mut event_tasks: Vec<JoinHandle<()>> = Vec::new();

    let mut subscribed_topics: Vec<String> = vec![];

    info!("WebSocket client connected");

    let mut heartbeat_interval = tokio::time::interval(tokio::time::Duration::from_secs(30));

    loop {
        tokio::select! {
            biased;
            _ = grant.cancelled() => break,
            msg = receiver.next() => {
                if !grant.is_valid() { break; }
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let had_stream_state = normalize_topics(&subscribed_topics)
                            .iter()
                            .any(|topic| is_stream_state_topic(topic));
                        if let Err(e) = handle_client_message(&text, &mut subscribed_topics).await {
                            warn!("Failed to handle client message: {}", e);
                        } else {
                            let has_stream_state = normalize_topics(&subscribed_topics)
                                .iter()
                                .any(|topic| is_stream_state_topic(topic));
                            if !rebuild_event_tasks(
                                &state,
                                &subscribed_topics,
                                !had_stream_state && has_stream_state,
                                &event_tx,
                                &mut event_tasks,
                            ) {
                                warn!("WebSocket event queue is full; disconnecting slow client");
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Ping(_))) => {
                        debug!("Received ping from client");
                    }
                    Some(Ok(Message::Pong(_))) => {
                        debug!("Received pong from client");
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        info!("WebSocket client disconnected");
                        break;
                    }
                    Some(Err(e)) => {
                        warn!("WebSocket receive error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }

            event = event_rx.recv() => {
                match event {
                    Some(BusMessage::Event(event)) => {
                        if let Ok(json) = serialize_event(&event) {
                            if !send_while_authorized(&grant, sender.send(Message::Text(json.into()))).await {
                                warn!("Failed to send event to client, disconnecting");
                                break;
                            }
                        }
                    }
                    Some(BusMessage::Lagged { topic, count }) => {
                        warn!(
                            "WebSocket client lagged by {} events on topic {}",
                            count, topic
                        );
                        let error_event = SystemEvent::Error {
                            message: format!("Lagged by {} events", count),
                        };
                        if let Ok(json) = serialize_event(&error_event) {
                            if !send_while_authorized(&grant, sender.send(Message::Text(json.into()))).await {
                                break;
                            }
                        }
                    }
                    None => {
                        warn!("Event bus closed");
                        break;
                    }
                }
            }

            _ = heartbeat_interval.tick() => {
                if !send_while_authorized(&grant, sender.send(Message::Ping(vec![].into()))).await {
                    warn!("Failed to send ping, disconnecting");
                    break;
                }
            }
        }
    }

    for task in event_tasks {
        task.abort();
    }

    info!("WebSocket handler exiting");
}

async fn handle_client_message(
    text: &str,
    topics: &mut Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let msg: ClientMessage = serde_json::from_str(text)?;

    match msg {
        ClientMessage::Subscribe { topics: new_topics } => {
            *topics = new_topics.clone();
            info!("Client subscribed to topics: {:?}", new_topics);
        }
        ClientMessage::Unsubscribe {
            topics: remove_topics,
        } => {
            topics.retain(|t| !remove_topics.contains(t));
            info!("Client unsubscribed from topics: {:?}", remove_topics);
        }
        ClientMessage::Ping => {
            debug!("Received ping from client");
        }
    }

    Ok(())
}

fn serialize_event(event: &SystemEvent) -> Result<String, serde_json::Error> {
    serde_json::to_string(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stalled_websocket_send_is_cancelled_without_blocking_cleanup() {
        use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};

        let (server_io, client_io) = tokio::io::duplex(64);
        let mut server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
        let mut client = WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
        let message = tokio_tungstenite::tungstenite::Message::Text("ready".into());
        assert!(send_until_cancelled(server.send(message.clone()), std::future::pending()).await);
        assert_eq!(client.next().await.unwrap().unwrap(), message);

        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        let message = tokio_tungstenite::tungstenite::Message::Binary(vec![0; 1024 * 1024].into());
        let blocked = send_until_cancelled(server.send(message), async {
            cancelled.await.unwrap();
        });
        tokio::pin!(blocked);
        assert!(futures::poll!(&mut blocked).is_pending());
        cancel.send(()).unwrap();
        assert!(!tokio::time::timeout(Duration::from_secs(1), blocked)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn stalled_websocket_send_has_a_finite_deadline() {
        use tokio_tungstenite::{tungstenite::protocol::Role, WebSocketStream};

        let (server_io, _client_io) = tokio::io::duplex(64);
        let mut server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
        let message = tokio_tungstenite::tungstenite::Message::Binary(vec![0; 1024 * 1024].into());
        let blocked = send_until_cancelled(server.send(message), std::future::pending());
        tokio::pin!(blocked);
        assert!(futures::poll!(&mut blocked).is_pending());
        assert!(
            !tokio::time::timeout(SEND_TIMEOUT + Duration::from_secs(1), blocked)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn queued_events_backpressure_producers_and_release_them_on_disconnect() {
        let (sender, mut receiver) = event_channel();
        for number in 0..EVENT_QUEUE_CAPACITY {
            sender
                .send(BusMessage::Lagged {
                    topic: "stream.*".into(),
                    count: number as u64,
                })
                .await
                .unwrap();
        }
        let blocked = sender.send(BusMessage::Lagged {
            topic: "stream.*".into(),
            count: EVENT_QUEUE_CAPACITY as u64,
        });
        tokio::pin!(blocked);
        assert!(futures::poll!(&mut blocked).is_pending());
        let Some(BusMessage::Lagged { count, .. }) = receiver.recv().await else {
            panic!("expected oldest event");
        };
        assert_eq!(count, 0);
        assert!(blocked.await.is_ok());
        let next = sender.send(BusMessage::Lagged {
            topic: "stream.*".into(),
            count: 999,
        });
        tokio::pin!(next);
        assert!(futures::poll!(&mut next).is_pending());
        drop(receiver);
        assert!(next.await.is_err());
    }

    #[test]
    fn test_normalize_topics_dedupes_and_sorts() {
        let topics = vec![
            "stream.state_changed".to_string(),
            "stream.state_changed".to_string(),
            "system.device_info".to_string(),
        ];

        assert_eq!(
            normalize_topics(&topics),
            vec![
                "stream.state_changed".to_string(),
                "system.device_info".to_string()
            ]
        );
    }

    #[test]
    fn test_normalize_topics_wildcard_wins() {
        let topics = vec!["*".to_string(), "stream.state_changed".to_string()];
        assert_eq!(normalize_topics(&topics), vec!["*".to_string()]);
    }

    #[test]
    fn test_normalize_topics_drops_exact_when_prefix_exists() {
        let topics = vec![
            "stream.*".to_string(),
            "stream.state_changed".to_string(),
            "system.device_info".to_string(),
        ];

        assert_eq!(
            normalize_topics(&topics),
            vec!["stream.*".to_string(), "system.device_info".to_string()]
        );
    }

    #[test]
    fn test_is_device_info_topic_matches_expected_topics() {
        assert!(is_device_info_topic("system.device_info"));
        assert!(is_device_info_topic("system.*"));
        assert!(is_device_info_topic("*"));
        assert!(!is_device_info_topic("stream.*"));
    }

    #[test]
    fn test_is_stream_state_topic_matches_stateful_subscriptions() {
        assert!(is_stream_state_topic("*"));
        assert!(is_stream_state_topic("stream.*"));
        assert!(is_stream_state_topic("stream.state_changed"));
        assert!(!is_stream_state_topic("stream.stats_update"));
        assert!(!is_stream_state_topic("system.device_info"));
    }
}

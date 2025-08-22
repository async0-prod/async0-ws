pub mod config;
pub mod middlewares;
pub mod state;
pub mod test;

use axum::{
    Router,
    body::Bytes,
    extract::{
        ConnectInfo, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    middleware,
    response::IntoResponse,
    routing::get,
};

use anyhow::{Context, Result};
use axum_extra::{TypedHeader, headers};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use thiserror::Error;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    middlewares::request_logger,
    state::{AppState, AppStateError},
};

const ADDR: &str = "0.0.0.0:8081";

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum UserMessage {
    Register {
        user_id: Uuid,
    },
    Deregister {
        user_id: Uuid,
    },
    Message {
        publisher_id: Uuid,
        data: Value,
    },
    Subscribe {
        subscriber_id: Uuid,
        publisher_id: Uuid,
    },
    Unsubscribe {
        subscriber_id: Uuid,
        publisher_id: Uuid,
    },
    GetStats,
}

#[derive(Debug, Serialize)]
pub struct ServerResponse {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct StatsResponse {
    pub publishers: Vec<PublisherStats>,
    pub total_subscribers: usize,
}

#[derive(Debug, Serialize)]
pub struct PublisherStats {
    pub id: Uuid,
    pub subscriber_count: usize,
    pub created_at: String,
}

impl ServerResponse {
    pub fn success(message: &str) -> Self {
        Self {
            success: true,
            message: message.to_string(),
            data: None,
        }
    }

    pub fn success_with_data(message: &str, data: Value) -> Self {
        Self {
            success: true,
            message: message.to_string(),
            data: Some(data),
        }
    }

    pub fn error(message: &str) -> Self {
        Self {
            success: false,
            message: message.to_string(),
            data: None,
        }
    }
}

#[derive(Error, Debug)]
enum WebSocketError {
    #[error("JSON parsing error: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("WebSocket error: {0}")]
    WebSocket(#[from] axum::Error),

    #[error("Application state error: {0}")]
    AppState(#[from] AppStateError),

    #[error("Invalid message: {0}")]
    InvalidMessage(String),

    #[error("Unauthorized action: {0}")]
    Unauthorized(String),
}

#[derive(Debug, Clone)]
struct ClientInfo {
    id: Uuid,
    role: ClientRole,
}

#[derive(Debug, Clone, PartialEq)]
enum ClientRole {
    Publisher,
    Subscriber { subscribed_to: Uuid },
}

type ClientStore = Arc<Mutex<HashMap<SocketAddr, ClientInfo>>>;

#[tokio::main]
async fn main() -> Result<()> {
    let listener = tokio::net::TcpListener::bind(ADDR)
        .await
        .context("Failed to bind to address")?;

    println!("Server running on {}", ADDR);

    axum::serve(
        listener,
        app().into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("Server error")
}

pub fn app() -> Router {
    let state = Arc::new(AppState::new());
    let client_store: ClientStore = Arc::new(Mutex::new(HashMap::new()));
    return Router::new()
        .fallback(fallback)
        .route("/health", get(|| async { "Ok" }))
        .route(
            "/test",
            get(|| async { "Test" }).route_layer(middleware::from_fn(request_logger)),
        )
        .route("/ws", get(ws_handler))
        .with_state((state, client_store));
}

pub async fn fallback(uri: axum::http::Uri) -> impl axum::response::IntoResponse {
    (axum::http::StatusCode::NOT_FOUND, uri.to_string())
}

pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    user_agent: Option<TypedHeader<headers::UserAgent>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State((state, client_store)): State<(Arc<AppState>, ClientStore)>,
) -> impl IntoResponse {
    let user_agent = user_agent
        .map(|TypedHeader(agent)| agent.to_string())
        .unwrap_or_else(|| "Unknown browser".to_string());

    println!("`{user_agent}` at {addr} connected.");

    ws.on_upgrade(move |socket| handle_socket(socket, addr, state, client_store))
}

// every request/user gets their own handle_socket function
async fn handle_socket(
    mut socket: WebSocket,
    addr: SocketAddr,
    state: Arc<AppState>,
    client_store: ClientStore,
) {
    if let Err(e) = socket
        .send(Message::Ping(Bytes::from_static(&[1, 2, 3])))
        .await
    {
        eprintln!("Failed to send ping to {addr}: {e}");
        return;
    }

    while let Some(msg_result) = socket.recv().await {
        let msg = match msg_result {
            Ok(msg) => msg,
            Err(e) => {
                eprintln!("Error receiving message from {addr}: {e}");
                break;
            }
        };

        if let Err(e) = handle_message(&mut socket, addr, &msg, &state, &client_store).await {
            eprintln!("Error handling message from {addr}: {e}");
            send_error_response(&mut socket, &format!("Error: {}", e)).await;

            // break on critical errors, continue on non-critical ones
            match e.downcast_ref::<WebSocketError>() {
                Some(WebSocketError::WebSocket(_)) => break,
                _ => continue,
            }
        }
    }

    // cleanup when client disconnects
    cleanup_client_from_store(addr, &state, &client_store).await;
    println!("Client {addr} disconnected");
}

async fn handle_message(
    socket: &mut WebSocket,
    addr: SocketAddr,
    msg: &Message,
    state: &Arc<AppState>,
    client_store: &ClientStore,
) -> Result<()> {
    match msg {
        Message::Text(text) => {
            let user_message: UserMessage =
                serde_json::from_str(text).context("Failed to parse user message")?;

            handle_user_message(socket, addr, user_message, state, client_store).await?;
        }
        Message::Close(_) => {
            println!("Client {addr} closed connection");
        }
        Message::Pong(_) => {
            println!("Received pong from {addr}");
        }
        Message::Binary(data) => {
            println!("Received {} bytes of binary data from {addr}", data.len());
        }
        _ => {}
    }
    Ok(())
}

async fn handle_user_message(
    socket: &mut WebSocket,
    addr: SocketAddr,
    message: UserMessage,
    state: &Arc<AppState>,
    client_store: &ClientStore,
) -> Result<()> {
    match message {
        UserMessage::Register { user_id } => {
            handle_register(socket, addr, user_id, state, client_store).await?;
        }
        UserMessage::Deregister { user_id } => {
            handle_deregister(socket, addr, user_id, state, client_store).await?;
        }
        UserMessage::Message { publisher_id, data } => {
            handle_publish_message(socket, addr, publisher_id, data, state, client_store).await?;
        }
        UserMessage::Subscribe {
            subscriber_id,
            publisher_id,
        } => {
            handle_subscribe(
                socket,
                addr,
                subscriber_id,
                publisher_id,
                state,
                client_store,
            )
            .await?;
        }
        UserMessage::Unsubscribe {
            subscriber_id,
            publisher_id,
        } => {
            handle_unsubscribe(
                socket,
                addr,
                subscriber_id,
                publisher_id,
                state,
                client_store,
            )
            .await?;
        }
        UserMessage::GetStats => {
            handle_get_stats(socket, state).await?;
        }
    }
    Ok(())
}

async fn handle_register(
    socket: &mut WebSocket,
    addr: SocketAddr,
    user_id: Uuid,
    state: &Arc<AppState>,
    client_store: &ClientStore,
) -> Result<()> {
    state.add_publisher(user_id, addr)?;

    let client_info = ClientInfo {
        id: user_id,
        role: ClientRole::Publisher,
    };

    client_store.lock().await.insert(addr, client_info);

    println!("Registered publisher: {user_id}");
    send_success_response(socket, &format!("Registered as publisher: {}", user_id)).await;

    return Ok(());
}

async fn handle_deregister(
    socket: &mut WebSocket,
    addr: SocketAddr,
    user_id: Uuid,
    state: &Arc<AppState>,
    client_store: &ClientStore,
) -> Result<()> {
    // first check if the incoming client is the same publisher who registered
    let client_info = client_store.lock().await.get(&addr).cloned();

    match client_info {
        Some(info) if info.id == user_id => {
            state.remove_publisher(user_id)?;
            client_store.lock().await.remove(&addr);

            println!("Deregistered publisher: {user_id}");
            send_success_response(socket, &format!("Deregistered publisher: {}", user_id)).await;
        }
        Some(_) => {
            return Err(WebSocketError::Unauthorized(
                "Cannot deregister publisher owned by another client".to_string(),
            )
            .into());
        }
        None => {
            return Err(WebSocketError::InvalidMessage("Client not registered".to_string()).into());
        }
    }

    Ok(())
}

async fn handle_publish_message(
    socket: &mut WebSocket,
    addr: SocketAddr,
    publisher_id: Uuid,
    data: Value,
    state: &Arc<AppState>,
    client_store: &ClientStore,
) -> Result<()> {
    // check if the incoming client is the same publisher who registered
    let client_info = client_store.lock().await.get(&addr).cloned();

    match client_info {
        Some(info) if info.id == publisher_id && matches!(info.role, ClientRole::Publisher) => {
            let subscriber_count = state.publish_message(publisher_id, data)?;
            println!("Publisher {publisher_id} sent message to {subscriber_count} subscribers");

            send_success_response(
                socket,
                &format!("Message sent to {} subscribers", subscriber_count),
            )
            .await;
        }
        Some(_) => {
            return Err(WebSocketError::Unauthorized(
                "Can only publish to your own publisher ID".to_string(),
            )
            .into());
        }
        None => {
            return Err(WebSocketError::InvalidMessage(
                "Client not registered as publisher".to_string(),
            )
            .into());
        }
    }

    Ok(())
}

async fn handle_subscribe(
    socket: &mut WebSocket,
    addr: SocketAddr,
    subscriber_id: Uuid,
    publisher_id: Uuid,
    state: &Arc<AppState>,
    client_store: &ClientStore,
) -> Result<()> {
    let max_subscriber_count = 100;
    state.add_subscriber(
        subscriber_id,
        publisher_id,
        addr,
        Some(max_subscriber_count),
    )?;

    // get publishers broadcast sender
    let sender = state.get_publisher(publisher_id)?;
    let mut receiver = sender.subscribe();

    // add client as subscriber to our client store
    let client_info = ClientInfo {
        id: subscriber_id,
        role: ClientRole::Subscriber {
            subscribed_to: publisher_id,
        },
    };

    client_store.lock().await.insert(addr, client_info);

    // send total subscriber count back to user
    let subscriber_count = state.get_subscriber_count_by_publisher_id(publisher_id)?;
    println!(
        "Subscriber {subscriber_id} subscribed to publisher {publisher_id} (total: {subscriber_count} subscribers)"
    );

    send_success_response(
        socket,
        &format!(
            "Subscribed to publisher: {} (total subscribers: {})",
            publisher_id, subscriber_count
        ),
    )
    .await;

    // we start listening for messages to the publisher's sender channel
    //  this will block and handle all incoming messages
    while let Ok(data) = receiver.recv().await {
        let message =
            serde_json::to_string(&data).context("Failed to serialize message for subscriber")?;

        if socket.send(Message::Text(message.into())).await.is_err() {
            println!("Failed to send message to subscriber {subscriber_id}, connection closed");
            break;
        }
    }

    // if we reach here, the publisher channel was closed or subscriber disconnected
    // Remove subscriber from state
    if let Err(e) = state.remove_subscriber(subscriber_id) {
        eprintln!("Error removing subscriber {}: {}", subscriber_id, e);
    }

    println!("Subscription ended for subscriber {subscriber_id}");

    Ok(())
}

async fn handle_unsubscribe(
    socket: &mut WebSocket,
    addr: SocketAddr,
    subscriber_id: Uuid,
    publisher_id: Uuid,
    state: &Arc<AppState>,
    client_store: &ClientStore,
) -> Result<()> {
    // check if the incoming client is the same publisher who registered
    let client_info = client_store.lock().await.get(&addr).cloned();

    match client_info {
        Some(info) if info.id == subscriber_id => {
            if let ClientRole::Subscriber { subscribed_to } = info.role {
                if subscribed_to == publisher_id {
                    state.remove_subscriber(subscriber_id)?;

                    client_store.lock().await.remove(&addr);

                    let remaining_count =
                        state.get_subscriber_count_by_publisher_id(publisher_id)?;
                    println!(
                        "Unsubscribed {subscriber_id} from publisher {publisher_id} (remaining: {remaining_count})"
                    );

                    send_success_response(
                        socket,
                        &format!(
                            "Unsubscribed from publisher: {} (remaining subscribers: {})",
                            publisher_id, remaining_count
                        ),
                    )
                    .await;
                } else {
                    return Err(WebSocketError::InvalidMessage(
                        "Not subscribed to this publisher".to_string(),
                    )
                    .into());
                }
            } else {
                return Err(WebSocketError::InvalidMessage(
                    "Client is not a subscriber".to_string(),
                )
                .into());
            }
        }
        Some(_) => {
            return Err(WebSocketError::Unauthorized(
                "Cannot unsubscribe for another client".to_string(),
            )
            .into());
        }
        None => {
            return Err(WebSocketError::InvalidMessage("Client not registered".to_string()).into());
        }
    }

    Ok(())
}

async fn handle_get_stats(socket: &mut WebSocket, state: &Arc<AppState>) -> Result<()> {
    let publishers = state.list_publishers()?;
    let all_subscribers = state.list_all_subscribers()?;

    let publisher_stats: Vec<PublisherStats> = publishers
        .into_iter()
        .map(|p| PublisherStats {
            id: p.id,
            subscriber_count: p.subscriber_count,
            created_at: p
                .created_at
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .to_string(),
        })
        .collect();

    let stats = StatsResponse {
        publishers: publisher_stats,
        total_subscribers: all_subscribers.len(),
    };

    let response =
        ServerResponse::success_with_data("System statistics", serde_json::to_value(stats)?);

    let json = serde_json::to_string(&response)?;
    socket.send(Message::Text(json.into())).await?;

    Ok(())
}

async fn cleanup_client_from_store(
    addr: SocketAddr,
    state: &Arc<AppState>,
    client_store: &ClientStore,
) {
    if let Some(client_info) = client_store.lock().await.remove(&addr) {
        match client_info.role {
            ClientRole::Publisher => {
                if let Err(e) = state.remove_publisher(client_info.id) {
                    eprintln!("Error removing publisher {}: {}", client_info.id, e);
                } else {
                    println!("Cleaned up publisher: {}", client_info.id);
                }
            }
            ClientRole::Subscriber { subscribed_to } => {
                if let Err(e) = state.remove_subscriber(client_info.id) {
                    eprintln!("Error removing subscriber {}: {}", client_info.id, e);
                } else {
                    let remaining = state
                        .get_subscriber_count_by_publisher_id(subscribed_to)
                        .unwrap_or(0);
                    println!(
                        "Cleaned up subscriber {} (publisher: {}, remaining: {})",
                        client_info.id, subscribed_to, remaining
                    );
                }
            }
        }
    }
}

async fn send_success_response(socket: &mut WebSocket, message: &str) {
    let response = ServerResponse::success(message);
    if let Ok(json) = serde_json::to_string(&response) {
        let _ = socket.send(Message::Text(json.into())).await;
    }
}

async fn send_error_response(socket: &mut WebSocket, message: &str) {
    let response = ServerResponse::error(message);
    if let Ok(json) = serde_json::to_string(&response) {
        let _ = socket.send(Message::Text(json.into())).await;
    }
}

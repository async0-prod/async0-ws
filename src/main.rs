pub mod state;

use axum::{
    Router,
    body::Bytes,
    extract::{
        ConnectInfo, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};

use axum_extra::{TypedHeader, headers};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc};

use crate::state::AppState;

const ADDR: &str = "0.0.0.0:8081";

#[derive(Deserialize)]
struct RegisterMsg {
    role: String,                 // "publisher" or "subscriber"
    id: Option<String>,           // for publishers: their id
    subscribe_to: Option<String>, // for subscribers: whose events they want
}

// #[derive(Deserialize)]
// struct PublishMsg {
//     publisher_id: String,
//     message: String,
// }

#[derive(Serialize)]
struct Response {
    success: bool,
    message: String,
}

#[tokio::main]
async fn main() {
    let state = Arc::new(AppState::new());

    let app = Router::new()
        .route("/", get(|| async { "Hello, World!" }))
        .route("/ws", get(ws_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(ADDR).await.unwrap();
    println!("Server running on {}", ADDR);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    user_agent: Option<TypedHeader<headers::UserAgent>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let user_agent = if let Some(TypedHeader(user_agent)) = user_agent {
        user_agent.to_string()
    } else {
        String::from("Unknown browser")
    };
    println!("`{user_agent}` at {addr} connected.");

    ws.on_upgrade(move |socket| handle_socket(socket, addr, state))
}

async fn handle_socket(mut socket: WebSocket, addr: SocketAddr, state: Arc<AppState>) {
    if socket
        .send(Message::Ping(Bytes::from_static(&[1, 2, 3])))
        .await
        .is_ok()
    {
        println!("Ping sent to {addr}");
    } else {
        println!("Failed to send ping to {addr}");
        return;
    }

    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(message) => {
                println!("Client says: {message}");
                let Ok(register_message) = serde_json::from_str::<RegisterMsg>(&message) else {
                    eprintln!("Invalid registration message");
                    return;
                };

                match register_message.role.as_str() {
                    "publisher" => {
                        let publisher_id = match register_message.id {
                            Some(id) => id,
                            None => {
                                send_error(&mut socket, "Publisher ID is required").await;
                                continue;
                            }
                        };

                        match state.add_publisher(publisher_id.clone()) {
                            Ok(()) => {
                                println!("Registered as publisher with id {publisher_id}");
                                send_success(
                                    &mut socket,
                                    &format!("Registered as publisher: {}", publisher_id),
                                )
                                .await;
                            }
                            Err(e) => {
                                send_error(&mut socket, &e).await;
                            }
                        }
                    }

                    "subscriber" => {
                        let subscriber_id = register_message
                            .id
                            .unwrap_or_else(|| format!("sub_{}", addr.port()));
                        let publisher_id = match register_message.subscribe_to {
                            Some(id) => id,
                            None => {
                                send_error(&mut socket, "Publisher ID to subscribe to is required")
                                    .await;
                                continue;
                            }
                        };

                        match state.get_publisher(&publisher_id) {
                            Ok(publisher) => {
                                println!(
                                    "Subscriber {subscriber_id} subscribed to publisher {publisher_id}"
                                );
                                send_success(
                                    &mut socket,
                                    &format!("Subscribed to publisher: {}", publisher_id),
                                )
                                .await;

                                // listening for messages from the subscribed publisher
                                let mut receiver = publisher.subscribe();

                                while let Ok(msg) = receiver.recv().await {
                                    if socket.send(Message::Text(msg.into())).await.is_err() {
                                        println!(
                                            "Failed to send message to subscriber {subscriber_id}"
                                        );
                                        break;
                                    }
                                }

                                return;
                            }
                            Err(error) => {
                                send_error(
                                    &mut socket,
                                    &format!(
                                        "No publisher found with id {}: {}",
                                        publisher_id, error
                                    ),
                                )
                                .await;
                            }
                        }
                    }
                    _ => {
                        send_error(&mut socket, "Invalid role. Use 'publisher' or 'subscriber'")
                            .await;
                    }
                }
            }
            Message::Binary(bin) => {
                println!("Client sent {} bytes of binary data", bin.len());
            }
            Message::Close(_) => {
                println!("Client closed connection");
                break;
            }
            _ => {}
        }
    }
}

async fn send_success(socket: &mut WebSocket, message: &str) {
    let response = Response {
        success: true,
        message: message.to_string(),
    };
    if let Ok(json) = serde_json::to_string(&response) {
        let _ = socket.send(Message::Text(json.into())).await;
    }
}

async fn send_error(socket: &mut WebSocket, message: &str) {
    let response = Response {
        success: false,
        message: message.to_string(),
    };
    if let Ok(json) = serde_json::to_string(&response) {
        let _ = socket.send(Message::Text(json.into())).await;
    }
}

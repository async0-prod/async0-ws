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
use serde::Deserialize;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast;

const ADDR: &str = "0.0.0.0:8081";

#[derive(Debug, Default)]
struct AppState {
    publishers: Mutex<HashMap<String, broadcast::Sender<String>>>,
}

#[derive(Deserialize)]
struct RegisterMsg {
    role: String,                 // "publisher" or "subscriber"
    id: Option<String>,           // for publishers: their id
    subscribe_to: Option<String>, // for subscribers: whose events they want
}

#[tokio::main]
async fn main() {
    let state = Arc::new(AppState::default());

    let app = Router::new()
        .route("/", get(|| async { "Hello, World!" }))
        .route("/ws", get(ws_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(ADDR).await.unwrap();
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
                        let publisher_id = register_message.id.unwrap_or_else(|| return "0".into());
                        println!("Registered as publisher with id {publisher_id}");
                    }
                    "subscriber" => {
                        let subscriber_id =
                            register_message.id.unwrap_or_else(|| return "0".into());
                        println!("Registered as subscriber with id {subscriber_id}");
                    }
                    _ => {
                        eprintln!("Invalid role");
                        return;
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

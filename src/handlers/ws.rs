use crate::{
    UserMessage,
    core::state::AppState,
    store::ws::{
        handle_user_deregister, handle_user_message, handle_user_register, handle_user_subscribe,
        handle_user_unsubscribe,
    },
};
use anyhow::{self, Context};
use axum::{
    extract::{
        ConnectInfo, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use axum_extra::{TypedHeader, headers};
use futures_util::{
    sink::SinkExt,
    stream::{SplitSink, SplitStream, StreamExt},
};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::mpsc;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    user_agent: Option<TypedHeader<headers::UserAgent>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let user_agent = user_agent
        .map(|TypedHeader(agent)| agent.to_string())
        .unwrap_or_else(|| "Unknown browser".to_string());

    println!("`{user_agent}` at {addr} connected.");

    ws.on_upgrade(move |socket| handle_socket(socket, addr, state))
}

// every request/user gets their own handle_socket function
async fn handle_socket(
    socket: WebSocket,
    addr: SocketAddr,
    state: Arc<AppState>,
    // _client_store: ClientStore,
) {
    let (ws_sender, ws_receiver) = socket.split();

    let (tx, rx) = mpsc::channel::<Message>(1000);
    tokio::spawn(write(ws_sender, rx));
    tokio::spawn(read(state, ws_receiver, tx));

    // TODO
    // cleanup when client disconnects
    // cleanup_client_from_store(addr, &state).await;
    println!("Client {addr} disconnected");
}

async fn read(
    state: Arc<AppState>,
    mut receiver: SplitStream<WebSocket>,
    tx: mpsc::Sender<Message>,
) {
    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Binary(data) => {
                println!("Received {} bytes of binary data from ", data.len());
            }
            Message::Close(_) => {
                println!("Client  closed connection");
            }
            Message::Pong(_) => {
                println!("Received pong from ");
            }
            Message::Ping(_) => {
                println!("Received ping from ");
            }
            Message::Text(text) => {
                let parsed =
                    serde_json::from_str::<UserMessage>(&text).context("Error parsing message");

                let user_message = match parsed {
                    Ok(msg) => msg,
                    Err(e) => {
                        eprintln!("Failed to parse user message: {e}");
                        // let _ = send_response(&mut socket, ServerResponse::error()).await;
                        break;
                    }
                };

                println!("{:?}", user_message);

                let result = match user_message {
                    UserMessage::Register { user_id } => {
                        handle_user_register(user_id, &state).await
                    }
                    UserMessage::Deregister { user_id } => {
                        handle_user_deregister(user_id, &&state, tx.clone()).await
                    }
                    UserMessage::Subscribe {
                        publisher_id,
                        subscriber_id,
                    } => {
                        handle_user_subscribe(subscriber_id, publisher_id, &state, tx.clone()).await
                    }
                    UserMessage::Unsubscribe { subscriber_id } => {
                        handle_user_unsubscribe(subscriber_id, &state, tx.clone()).await
                    }
                    UserMessage::Message { publisher_id, data } => {
                        handle_user_message(publisher_id, data, &state).await
                    }
                    UserMessage::GetStats => todo!(),
                };

                if let Err(e) = result {
                    eprintln!("Error handling message: {e}");
                    // let _ = send_response(&mut socket, ServerResponse::error()).await;
                    break;
                }
            }
        }

        // Send success response back through channel
        let response = Message::Text("Success".into());
        let _ = tx.send(response).await;
    }
}

async fn write(mut sender: SplitSink<WebSocket, Message>, mut rx: mpsc::Receiver<Message>) {
    while let Some(msg) = rx.recv().await {
        sender.send(msg).await.ok();
    }
}

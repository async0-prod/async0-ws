use anyhow::{self, Context, Result};
use axum::{
    body::Bytes,
    extract::{
        ConnectInfo, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use axum_extra::{TypedHeader, headers};
use std::{net::SocketAddr, sync::Arc};

use crate::{
    ClientStore, UserMessage,
    core::state::AppState,
    store::ws::{
        handle_get_stats, handle_user_deregister, handle_user_message, handle_user_register,
        handle_user_subscribe, handle_user_unsubscribe,
    },
    utils::utils::send_error_response,
};

pub async fn ws_handler(
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
    // Send initial ping
    if let Err(e) = socket
        .send(Message::Ping(Bytes::from_static(&[1, 2, 3])))
        .await
        .with_context(|| format!("Failed to send ping to {addr}"))
    {
        eprintln!("Failed to send ping to {addr}: {e}");
        return;
    }

    while let Some(msg_result) = socket.recv().await {
        let ws_message = match msg_result {
            Ok(msg) => msg,
            Err(e) => {
                eprintln!("Error receiving message from {addr}: {e}");
                break;
            }
        };

        match ws_message {
            Message::Text(text) => {
                let parsed: Result<UserMessage> =
                    serde_json::from_str(&text).context("Invalid message");

                let user_message = match parsed {
                    Ok(msg) => msg,
                    Err(e) => {
                        eprintln!("Failed to parse user message from {addr}: {e}");
                        send_error_response(&mut socket).await;
                        break;
                    }
                };

                let result = match user_message {
                    UserMessage::Register { user_id } => {
                        handle_user_register(&mut socket, addr, user_id, &state, &client_store)
                            .await
                    }

                    UserMessage::Deregister { user_id } => {
                        handle_user_deregister(&mut socket, addr, user_id, &state, &client_store)
                            .await
                    }

                    UserMessage::Message { publisher_id, data } => {
                        handle_user_message(
                            &mut socket,
                            addr,
                            publisher_id,
                            data,
                            &state,
                            &client_store,
                        )
                        .await
                    }

                    UserMessage::Subscribe {
                        publisher_id,
                        subscriber_id,
                    } => {
                        handle_user_subscribe(
                            &mut socket,
                            addr,
                            subscriber_id,
                            publisher_id,
                            &state,
                            &client_store,
                        )
                        .await
                    }

                    UserMessage::Unsubscribe {
                        publisher_id,
                        subscriber_id,
                    } => {
                        handle_user_unsubscribe(
                            &mut socket,
                            addr,
                            subscriber_id,
                            publisher_id,
                            &state,
                            &client_store,
                        )
                        .await
                    }

                    UserMessage::GetStats => handle_get_stats(&mut socket, &state).await,
                };

                if let Err(e) = result {
                    eprintln!("Error handling message from {addr}: {e}");
                    send_error_response(&mut socket).await;
                    break;
                }
            }
            Message::Binary(data) => {
                println!("Received {} bytes of binary data from {addr}", data.len());
            }
            Message::Close(_) => {
                println!("Client {addr} closed connection");
            }
            Message::Pong(_) => {
                println!("Received pong from {addr}");
            }
            Message::Ping(_) => {
                println!("Received ping from {addr}");
            }
        }
    }

    // TODO
    // cleanup when client disconnects
    // cleanup_client_from_store(addr, &state, &client_store).await;
    println!("Client {addr} disconnected");
}

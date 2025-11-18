use crate::core::state::{AppState, AppStateError};
use axum::extract::ws::Message;

use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

pub async fn handle_user_register(
    user_id: Uuid,
    state: &Arc<AppState>,
) -> Result<(), AppStateError> {
    state.add_publisher(user_id).await?;
    Ok(())
}

pub async fn handle_user_deregister(
    user_id: Uuid,
    state: &Arc<AppState>,
    tx: mpsc::Sender<Message>,
) -> Result<(), AppStateError> {
    state.remove_publisher(user_id).await?;
    let _ = tx.send(Message::Close(None)).await;
    Ok(())
}

pub async fn handle_user_message(
    publisher_id: Uuid,
    data: Value,
    state: &Arc<AppState>,
) -> Result<(), AppStateError> {
    state.publish_message(publisher_id, data).await?;
    Ok(())
}

pub async fn handle_user_subscribe(
    subscriber_id: Uuid,
    publisher_id: Uuid,
    state: &Arc<AppState>,
    tx: mpsc::Sender<Message>,
) -> Result<(), AppStateError> {
    // const MAX_SUBSCRIBER_COUNT: usize = 100;
    state
        .add_subscriber(subscriber_id, publisher_id, tx.clone())
        .await?;
    Ok(())
}

pub async fn handle_user_unsubscribe(
    subscriber_id: Uuid,
    state: &Arc<AppState>,
    tx: mpsc::Sender<Message>,
) -> Result<(), AppStateError> {
    state.remove_subscriber(subscriber_id).await?;
    let _ = tx.send(Message::Close(None)).await;
    Ok(())
}

// pub async fn handle_get_stats(
//     _socket: &mut WebSocket,
//     _state: &Arc<AppState>,
// ) -> Result<(), AppStateError> {
// let publishers = state.list_publishers()?;
// let subscribers = state.list_subscribers()?;

// let publisher_stats: Vec<PublisherStats> = publishers
//     .into_iter()
//     .map(|p| PublisherStats {
//         id: p.id,
//         subscriber_count: p.subscriber_count,
//         created_at: p
//             .created_at
//             .duration_since(std::time::UNIX_EPOCH)
//             .unwrap_or_default()
//             .as_secs()
//             .to_string(),
//     })
//     .collect();

// let stats = StatsResponse {
//     publishers: publishers.len() as u32,
//     total_subscribers: subscribers.len() as u32,
// };

// let value = serde_json::to_value(stats).map_err(|_| AppStateError::GenericError)?;
// let response = ServerResponse::success_with_data(value);
// let json = serde_json::to_string(&response).map_err(|_| AppStateError::GenericError)?;

// socket
//     .send(Message::Text(json.into()))
//     .await
//     .map_err(|_| AppStateError::GenericError)?;

//     Ok(())
// }

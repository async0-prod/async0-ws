use axum::extract::ws::Message;
use futures_util::StreamExt;
use redis::AsyncTypedCommands;
use serde_json::Value;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Mutex, MutexGuard},
};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum AppStateError {
    #[error("Something went wrong")]
    GenericError,

    #[error("Publisher '{id}' already exists")]
    PublisherAlreadyExists { id: Uuid },

    #[error("Publisher '{id}' not found")]
    PublisherNotFound { id: Uuid },

    #[error("Subscriber '{id}' already exists")]
    SubscriberAlreadyExists { id: Uuid },

    #[error("Subscriber '{id}' not found")]
    SubscriberNotFound { id: Uuid },

    #[error("Maximum subscribers ({max}) reached for publisher '{publisher_id}'")]
    MaxSubscribersReached { publisher_id: Uuid, max: usize },

    #[error("Subscriber '{id}' is not subscribed to publisher '{publisher_id}'")]
    NotSubscribed { id: Uuid, publisher_id: Uuid },

    #[error("Failed to send message to publisher '{id}': {reason}")]
    MessageSendFailed { id: Uuid, reason: String },

    #[error("Failed to acquire lock")]
    LockError,
}

#[derive(Debug, Clone)]
pub struct SubscriberInfo {
    pub id: Uuid,
    pub publisher_id: Uuid,
    pub addr: SocketAddr,
    pub connected_at: std::time::SystemTime,
}

#[derive(Debug, Clone)]
pub struct PublisherInfo {
    pub id: Uuid,
    // pub addr: SocketAddr
    pub created_at: std::time::SystemTime,
    pub subscriber_count: usize,
}

#[derive(Debug)]
struct _PublisherData {
    pub info: PublisherInfo,
    pub sender: broadcast::Sender<Value>,
    pub subscriber_ids: Vec<Uuid>,
}

#[derive(Debug)]
struct AppStateInner {
    redis: redis::Client,
    // publishers: HashMap<Uuid, PublisherData>,
    // subscribers: HashMap<Uuid, SubscriberInfo>,
}

impl AppStateInner {
    fn new(redis: redis::Client) -> Self {
        Self {
            redis,
            // publishers: HashMap::new(),
            // subscribers: HashMap::new(),
        }
    }
}

#[derive(Debug)]
pub struct AppState {
    inner: Mutex<AppStateInner>,
}

impl AppState {
    pub fn new(redis: redis::Client) -> Self {
        let inner = AppStateInner::new(redis);

        Self {
            inner: Mutex::new(inner),
        }
    }

    fn _lock_inner(&self) -> Result<MutexGuard<'_, AppStateInner>, AppStateError> {
        self.inner.lock().map_err(|_| AppStateError::LockError)
    }

    pub async fn add_publisher(&self, id: Uuid) -> Result<(), AppStateError> {
        let redis = {
            let inner = self._lock_inner()?;
            inner.redis.clone()
        };
        let mut conn = redis
            .get_multiplexed_async_connection()
            .await
            .map_err(|_| AppStateError::GenericError)?;

        let key = format!("publisher:{}", id);
        let _: () = redis::cmd("SET")
            .arg(&key)
            .arg("online")
            .arg("EX")
            .arg(20) // expires after 20s unless refreshed by publisher
            .query_async(&mut conn)
            .await
            .map_err(|_| AppStateError::GenericError)?;

        // let result = conn
        //     .publish(id.to_string(), "Hello")
        //     .await
        //     .map_err(|_| AppStateError::GenericError)?;

        Ok(())
    }

    pub async fn add_subscriber(
        &self,
        id: Uuid,
        publisher_id: Uuid,
        tx: mpsc::Sender<Message>,
    ) -> Result<(), AppStateError> {
        let redis = {
            let inner = self._lock_inner()?;
            inner.redis.clone()
        };

        {
            let mut conn = redis
                .get_multiplexed_async_connection()
                .await
                .map_err(|_| AppStateError::GenericError)?;
            let key = format!("publisher:{}", publisher_id);
            let exists: Option<String> = redis::cmd("GET")
                .arg(&key)
                .query_async(&mut conn)
                .await
                .map_err(|_| AppStateError::GenericError)?;

            if exists.is_none() {
                return Err(AppStateError::GenericError);
            }
        }

        println!("Subscriber {} listening to publisher {}", id, publisher_id);

        tokio::spawn(async move {
            let mut pubsub = match redis.get_async_pubsub().await {
                Ok(ps) => ps,
                Err(e) => {
                    eprintln!("Failed to create async pubsub for {}: {}", id, e);
                    return;
                }
            };

            if let Err(e) = pubsub.subscribe(publisher_id.to_string()).await {
                eprintln!(
                    "Failed to subscribe subscriber {} to publisher {}: {}",
                    id, publisher_id, e
                );
                return;
            }

            loop {
                let mut stream = pubsub.on_message();

                let msg = match stream.next().await {
                    Some(m) => m,
                    None => {
                        eprintln!("Pubsub ended for subscriber {}", id);
                        break;
                    }
                };

                let payload: String = match msg.get_payload() {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Payload parse error: {}", e);
                        continue;
                    }
                };

                println!("Message payload received: {}", payload);

                if payload == "__END__" {
                    println!("Publisher {} ended stream.", publisher_id);
                    break;
                }

                if tx.send(Message::Text(payload.into())).await.is_err() {
                    println!("WebSocket closed for subscriber {}", id);
                    break;
                }
            }

            let _ = pubsub.unsubscribe(publisher_id.to_string());
            println!("Subscriber {} unsubscribed from {}", id, publisher_id);
        });

        Ok(())
    }

    pub async fn publish_message(&self, id: Uuid, data: Value) -> Result<(), AppStateError> {
        let redis = {
            let inner = self._lock_inner()?;
            inner.redis.clone()
        };
        let mut conn = redis
            .get_multiplexed_async_connection()
            .await
            .map_err(|_| AppStateError::GenericError)?;

        {
            let mut conn = redis
                .get_multiplexed_async_connection()
                .await
                .map_err(|_| AppStateError::GenericError)?;
            let key = format!("publisher:{}", id);
            let exists: Option<String> = redis::cmd("GET")
                .arg(&key)
                .query_async(&mut conn)
                .await
                .map_err(|_| AppStateError::GenericError)?;

            if exists.is_none() {
                return Err(AppStateError::GenericError);
            }
        }

        let _result = conn
            .publish(id.to_string(), data.to_string())
            .await
            .map_err(|_| AppStateError::GenericError)?;

        Ok(())
    }

    // pub fn remove_subscriber(&self, id: Uuid) -> Result<SubscriberInfo, AppStateError> {
    //     let mut inner = self._lock_inner()?;

    //     let subscriber = inner
    //         .subscribers
    //         .remove(&id)
    //         .ok_or(AppStateError::SubscriberNotFound { id })?;

    //     if let Some(publisher) = inner.publishers.get_mut(&subscriber.publisher_id) {
    //         publisher.subscriber_ids.retain(|sub_id| *sub_id != id);
    //     }

    //     return Ok(subscriber);
    // }

    // pub fn remove_publisher(&self, id: Uuid) -> Result<(), AppStateError> {
    //     let mut inner = self._lock_inner()?;

    //     let publisher = inner
    //         .publishers
    //         .remove(&id)
    //         .ok_or(AppStateError::PublisherNotFound { id })?;

    //     // send a notif that im closing
    //     let notification = serde_json::json!({
    //         "kind": "response",
    //         "success": false,
    //         "message": format!("Publisher {} has been deregistered", id),
    //         "data": null
    //     });
    //     let _ = publisher.sender.send(notification);

    //     // remove all subscribers of this publisher
    //     for subscriber_id in publisher.subscriber_ids {
    //         inner.subscribers.remove(&subscriber_id);
    //     }

    //     return Ok(());
    // }
}

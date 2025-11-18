use axum::extract::ws::Message;
use futures_util::StreamExt;
use redis::{AsyncTypedCommands, RedisError};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum AppStateError {
    #[error("Something went wrong. Error: {error}")]
    GenericError { error: &'static str },

    #[error("Error getting redis connection: {error}")]
    RedisConnectionError { error: RedisError },

    #[error("Error executing redis command: {error}")]
    RedisExecutionError { error: RedisError },

    // #[error("Error getting db connection")]
    // DatabaseConnectionError,

    // #[error("Publisher '{id}' already exists")]
    // PublisherAlreadyExists { id: Uuid },

    // #[error("Publisher '{id}' not found")]
    // PublisherNotFound { id: Uuid },

    // #[error("Subscriber '{id}' already exists")]
    // SubscriberAlreadyExists { id: Uuid },

    // #[error("Subscriber '{id}' not found")]
    // SubscriberNotFound { id: Uuid },

    // #[error("Maximum subscribers ({max}) reached for publisher '{publisher_id}'")]
    // MaxSubscribersReached { publisher_id: Uuid, max: usize },

    // #[error("Subscriber '{id}' is not subscribed to publisher '{publisher_id}'")]
    // NotSubscribed { id: Uuid, publisher_id: Uuid },

    // #[error("Failed to send message to publisher '{id}': {reason}")]
    // MessageSendFailed { id: Uuid, reason: String },
    #[error("Failed to acquire lock")]
    LockError,
}

// #[derive(Debug, Clone)]
// pub struct SubscriberInfo {
//     pub id: Uuid,
//     pub publisher_id: Uuid,
//     pub addr: SocketAddr,
//     pub connected_at: std::time::SystemTime,
// }

// #[derive(Debug, Clone)]
// pub struct PublisherInfo {
//     pub id: Uuid,
//     // pub addr: SocketAddr
//     pub created_at: std::time::SystemTime,
//     pub subscriber_count: usize,
// }

#[derive(Debug)]
struct _PublisherData {
    // pub info: PublisherInfo,
    pub sender: broadcast::Sender<Value>,
    pub subscriber_ids: Vec<Uuid>,
}

#[derive(Debug)]
struct AppStateInner {
    redis: redis::Client,
    subscriber_cancel: HashMap<Uuid, CancellationToken>,
    // publishers: HashMap<Uuid, PublisherData>,
    // subscribers: HashMap<Uuid, SubscriberInfo>,
}

impl AppStateInner {
    fn new(redis: redis::Client) -> Self {
        Self {
            redis,
            subscriber_cancel: HashMap::new(),
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
            .map_err(|e| AppStateError::RedisConnectionError { error: e })?;

        let key = format!("publisher:{}", id);
        let _: () = redis::cmd("SET")
            .arg(&key)
            .arg("online")
            .arg("EX")
            .arg(300) // expires after 300s unless refreshed by publisher
            .query_async(&mut conn)
            .await
            .map_err(|e| AppStateError::RedisExecutionError { error: e })?;

        Ok(())
    }

    pub async fn remove_publisher(&self, id: Uuid) -> Result<(), AppStateError> {
        let redis = {
            let inner = self._lock_inner()?;
            inner.redis.clone()
        };

        // Check if the publisher exists -> has a key "publisher:<id>"
        {
            let mut conn = redis
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| AppStateError::RedisConnectionError { error: e })?;
            let key = format!("publisher:{}", id);
            let exists: Option<String> =
                redis::cmd("GET")
                    .arg(&key)
                    .query_async(&mut conn)
                    .await
                    .map_err(|e| AppStateError::RedisExecutionError { error: e })?;

            if exists.is_none() {
                return Err(AppStateError::GenericError {
                    error: "Publisher doesn't exist",
                });
            }
        }

        let key = format!("publisher:{}", id);

        {
            let mut conn = redis
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| AppStateError::RedisConnectionError { error: e })?;

            let _result = conn.publish(id.to_string(), "__END__").await.map_err(|_| {
                AppStateError::GenericError {
                    error: "Error publishing __END__ message in remove_publisher",
                }
            })?;

            println!("Published __END__ for publisher {}", id);
        }

        {
            let mut conn = redis
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| AppStateError::RedisConnectionError { error: e })?;

            let _: () = redis::cmd("DEL")
                .arg(&key)
                .query_async(&mut conn)
                .await
                .map_err(|e| AppStateError::RedisExecutionError { error: e })?;

            println!("Deleted publisher key {}", key);
        }

        println!("Publisher {} successfully deregistered", id);

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

        let token = CancellationToken::new();

        {
            let mut inner = self._lock_inner()?;
            inner.subscriber_cancel.insert(id, token.clone());
        }

        // Check if the publisher exists -> has a key "publisher:<id>"
        {
            let mut conn = redis
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| AppStateError::RedisConnectionError { error: e })?;
            let key = format!("publisher:{}", publisher_id);
            let exists: Option<String> =
                redis::cmd("GET")
                    .arg(&key)
                    .query_async(&mut conn)
                    .await
                    .map_err(|e| AppStateError::RedisExecutionError { error: e })?;

            if exists.is_none() {
                return Err(AppStateError::GenericError {
                    error: "Publisher doesn't exist",
                });
            }
        }

        tokio::spawn(async move {
            tokio::select! {
                _ = subscriber_task(redis, id, publisher_id, tx.clone(), token.clone()) => {
                    println!("Subscriber task {} finished", id);
                }
                _ = token.cancelled() => {
                    println!("Subscriber {} cancelled manually", id);
                }
            }
        });

        Ok(())
    }

    pub async fn remove_subscriber(&self, subscriber_id: Uuid) -> Result<(), AppStateError> {
        let token = {
            let mut inner = self._lock_inner()?;
            inner.subscriber_cancel.remove(&subscriber_id)
        };

        if let Some(t) = token {
            t.cancel();
            println!(
                "Unsubscribed subscriber {} and cancelled task",
                subscriber_id
            );
        } else {
            println!(
                "Unsubscribe requested, but no task found for {}",
                subscriber_id
            );
        }

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
            .map_err(|e| AppStateError::RedisConnectionError { error: e })?;

        // Check if the publisher exists -> has a key "publisher:<id>"
        {
            let mut conn = redis
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| AppStateError::RedisConnectionError { error: e })?;

            let key = format!("publisher:{}", id);

            let exists: Option<String> =
                redis::cmd("GET")
                    .arg(&key)
                    .query_async(&mut conn)
                    .await
                    .map_err(|e| AppStateError::RedisExecutionError { error: e })?;

            if exists.is_none() {
                return Err(AppStateError::GenericError {
                    error: "Publisher doesn't exist",
                });
            }

            // Extend the ttl by 300s
            let _: () = redis::cmd("EXPIRE")
                .arg(&key)
                .arg(300)
                .query_async(&mut conn)
                .await
                .map_err(|e| AppStateError::RedisExecutionError { error: e })?;
        }

        let _result = conn
            .publish(id.to_string(), data.to_string())
            .await
            .map_err(|e| AppStateError::RedisExecutionError { error: e })?;

        Ok(())
    }
}

pub async fn subscriber_task(
    redis: redis::Client,
    subscriber_id: Uuid,
    publisher_id: Uuid,
    tx: mpsc::Sender<Message>,
    cancel_token: CancellationToken,
) {
    println!(
        "Starting subscriber task: subscriber={} → publisher={}",
        subscriber_id, publisher_id
    );

    let mut pubsub = match redis.get_async_pubsub().await {
        Ok(ps) => ps,
        Err(e) => {
            eprintln!(
                "Subscriber {} failed to create pubsub connection: {}",
                subscriber_id, e
            );
            return;
        }
    };

    if let Err(e) = pubsub.subscribe(publisher_id.to_string()).await {
        eprintln!(
            "Subscriber {} failed to subscribe to {}: {}",
            subscriber_id, publisher_id, e
        );
        return;
    }

    println!(
        "Subscriber {} is now listening to publisher {}",
        subscriber_id, publisher_id
    );

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                println!("CancellationToken triggered → stopping subscriber {}", subscriber_id);
                break;
            }

            msg = async {
                let mut stream = pubsub.on_message();
                stream.next().await
            } => {
                let msg = match msg {
                    Some(m) => m,
                    None => {
                        eprintln!("Redis pubsub ended for subscriber {}", subscriber_id);
                        break;
                    }
                };

                let payload: String = match msg.get_payload() {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Subscriber {}: failed to parse payload: {}", subscriber_id, e);
                        continue;
                    }
                };

                if payload == "__END__" {
                    println!(
                        "Publisher {} sent __END__, stopping subscriber {}",
                        publisher_id, subscriber_id
                    );
                    break;
                }

                if tx.send(Message::Text(payload.into())).await.is_err() {
                    println!("WebSocket closed → stopping subscriber {}", subscriber_id);
                    break;
                }
            }
        }
    }

    let _ = pubsub.unsubscribe(publisher_id.to_string()).await;
    let _ = tx.send(Message::Close(None)).await;

    println!(
        "Subscriber {} unsubscribed cleanly from publisher {}",
        subscriber_id, publisher_id
    );
}

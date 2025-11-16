use serde_json::Value;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Mutex, MutexGuard},
};
use thiserror::Error;
use tokio::sync::broadcast;
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
    pub addr: SocketAddr,
    pub created_at: std::time::SystemTime,
    pub subscriber_count: usize,
}

#[derive(Debug)]
struct PublisherData {
    pub info: PublisherInfo,
    pub sender: broadcast::Sender<Value>,
    pub subscriber_ids: Vec<Uuid>,
}

#[derive(Debug, Default)]
struct AppStateInner {
    publishers: HashMap<Uuid, PublisherData>,
    subscribers: HashMap<Uuid, SubscriberInfo>,
}

#[derive(Debug, Default)]
pub struct AppState {
    inner: Mutex<AppStateInner>,
}

impl AppState {
    pub fn new() -> Self {
        return Self::default();
    }

    fn _lock_inner(&self) -> Result<MutexGuard<'_, AppStateInner>, AppStateError> {
        self.inner.lock().map_err(|_| AppStateError::LockError)
    }

    pub fn add_publisher(&self, id: Uuid, addr: SocketAddr) -> Result<(), AppStateError> {
        let mut inner = self._lock_inner()?;

        // check if the publisher already exists in the hashmap
        if inner.publishers.contains_key(&id) {
            return Err(AppStateError::PublisherAlreadyExists { id });
        }

        let (sender, _receiver) = broadcast::channel(100);

        let info = PublisherInfo {
            id,
            addr,
            created_at: std::time::SystemTime::now(),
            subscriber_count: 0,
        };

        let data = PublisherData {
            info,
            sender,
            subscriber_ids: Vec::new(),
        };

        inner.publishers.insert(id, data);

        return Ok(());
    }

    pub fn remove_publisher(&self, id: Uuid) -> Result<(), AppStateError> {
        let mut inner = self._lock_inner()?;

        let publisher = inner
            .publishers
            .remove(&id)
            .ok_or(AppStateError::PublisherNotFound { id })?;

        // send a notif that im closing
        let notification = serde_json::json!({
            "kind": "response",
            "success": false,
            "message": format!("Publisher {} has been deregistered", id),
            "data": null
        });
        let _ = publisher.sender.send(notification);

        // remove all subscribers of this publisher
        for subscriber_id in publisher.subscriber_ids {
            inner.subscribers.remove(&subscriber_id);
        }

        return Ok(());
    }

    pub fn get_publisher_sender(
        &self,
        id: Uuid,
    ) -> Result<broadcast::Sender<Value>, AppStateError> {
        let inner = self._lock_inner()?;

        inner
            .publishers
            .get(&id)
            .map(|p| p.sender.clone())
            .ok_or(AppStateError::PublisherNotFound { id })
    }

    pub fn publisher_exists(&self, id: Uuid) -> Result<bool, AppStateError> {
        let inner = self._lock_inner()?;
        return Ok(inner.publishers.contains_key(&id));
    }

    pub fn get_publisher_info(&self, id: Uuid) -> Result<PublisherInfo, AppStateError> {
        let inner = self._lock_inner()?;
        inner
            .publishers
            .get(&id)
            .map(|p| {
                let mut info = p.info.clone();
                info.subscriber_count = p.subscriber_ids.len();
                info
            })
            .ok_or(AppStateError::PublisherNotFound { id })
    }

    pub fn list_publishers(&self) -> Result<Vec<PublisherInfo>, AppStateError> {
        let inner = self._lock_inner()?;

        let mut publishers_arr = Vec::new();
        for data in inner.publishers.values() {
            let mut info = data.info.clone();
            info.subscriber_count = data.subscriber_ids.len();
            publishers_arr.push(info);
        }

        return Ok(publishers_arr);
    }

    pub fn subscriber_exists(&self, id: Uuid) -> Result<bool, AppStateError> {
        let inner = self._lock_inner()?;
        return Ok(inner.subscribers.contains_key(&id));
    }

    pub fn add_subscriber(
        &self,
        id: Uuid,
        publisher_id: Uuid,
        addr: SocketAddr,
        max_subscribers_per_publisher: Option<usize>,
    ) -> Result<(), AppStateError> {
        let mut inner = self._lock_inner()?;

        if inner.subscribers.contains_key(&id) {
            return Err(AppStateError::SubscriberAlreadyExists { id });
        }

        if !inner.publishers.contains_key(&publisher_id) {
            return Err(AppStateError::PublisherNotFound { id: publisher_id });
        }

        if let Some(max) = max_subscribers_per_publisher {
            let current_count = inner
                .publishers
                .get(&publisher_id)
                .map(|p| p.subscriber_ids.len())
                .unwrap_or(0);

            if current_count >= max {
                return Err(AppStateError::MaxSubscribersReached { publisher_id, max });
            }
        }

        let info = SubscriberInfo {
            id,
            publisher_id,
            addr,
            connected_at: std::time::SystemTime::now(),
        };

        inner.subscribers.insert(id, info);
        let publisher = inner
            .publishers
            .get_mut(&publisher_id)
            .ok_or(AppStateError::PublisherNotFound { id: publisher_id })?;
        publisher.subscriber_ids.push(id);

        return Ok(());
    }

    pub fn remove_subscriber(&self, id: Uuid) -> Result<SubscriberInfo, AppStateError> {
        let mut inner = self._lock_inner()?;

        let subscriber = inner
            .subscribers
            .remove(&id)
            .ok_or(AppStateError::SubscriberNotFound { id })?;

        if let Some(publisher) = inner.publishers.get_mut(&subscriber.publisher_id) {
            publisher.subscriber_ids.retain(|sub_id| *sub_id != id);
        }

        return Ok(subscriber);
    }

    pub fn get_subscriber(&self, id: Uuid) -> Result<SubscriberInfo, AppStateError> {
        let inner = self._lock_inner()?;

        inner
            .subscribers
            .get(&id)
            .cloned()
            .ok_or(AppStateError::SubscriberNotFound { id })
    }

    pub fn list_subscribers_by_publisher_id(
        &self,
        publisher_id: Uuid,
    ) -> Result<Vec<SubscriberInfo>, AppStateError> {
        let inner = self._lock_inner()?;

        let publisher = inner
            .publishers
            .get(&publisher_id)
            .ok_or(AppStateError::PublisherNotFound { id: publisher_id })?;

        let mut result = Vec::new();
        for subscriber_id in &publisher.subscriber_ids {
            if let Some(sub_info) = inner.subscribers.get(subscriber_id) {
                result.push(sub_info.clone());
            }
        }

        return Ok(result);
    }

    pub fn list_subscribers(&self) -> Result<Vec<SubscriberInfo>, AppStateError> {
        let inner = self._lock_inner()?;
        return Ok(inner.subscribers.values().cloned().collect());
    }

    pub fn get_subscriber_count_by_publisher_id(
        &self,
        publisher_id: Uuid,
    ) -> Result<usize, AppStateError> {
        let inner = self._lock_inner()?;

        let publisher = inner
            .publishers
            .get(&publisher_id)
            .ok_or(AppStateError::PublisherNotFound { id: publisher_id })?;

        return Ok(publisher.subscriber_ids.len());
    }

    pub fn is_subscribed(
        &self,
        subscriber_id: Uuid,
        publisher_id: Uuid,
    ) -> Result<bool, AppStateError> {
        let inner = self._lock_inner()?;

        let publisher = inner
            .publishers
            .get(&publisher_id)
            .ok_or(AppStateError::PublisherNotFound { id: publisher_id })?;

        return Ok(publisher.subscriber_ids.contains(&subscriber_id));
    }

    pub fn unsubscribe(
        &self,
        subscriber_id: Uuid,
        publisher_id: Uuid,
    ) -> Result<(), AppStateError> {
        let mut inner = self._lock_inner()?;

        // ensure subscriber exists
        let subscriber = inner
            .subscribers
            .get(&subscriber_id)
            .cloned()
            .ok_or(AppStateError::SubscriberNotFound { id: subscriber_id })?;

        if subscriber.publisher_id != publisher_id {
            return Err(AppStateError::NotSubscribed {
                id: subscriber_id,
                publisher_id,
            });
        }

        inner.subscribers.remove(&subscriber_id);
        if let Some(publisher) = inner.publishers.get_mut(&publisher_id) {
            publisher.subscriber_ids.retain(|id| *id != subscriber_id);
        }

        return Ok(());
    }

    pub fn publish_message(&self, publisher_id: Uuid, data: Value) -> Result<usize, AppStateError> {
        let inner = self._lock_inner()?;

        let publisher = inner
            .publishers
            .get(&publisher_id)
            .ok_or(AppStateError::PublisherNotFound { id: publisher_id })?;

        publisher
            .sender
            .send(data)
            .map_err(|e| AppStateError::MessageSendFailed {
                id: publisher_id,
                reason: e.to_string(),
            })
    }

    pub fn cleanup_subscriber_by_addr(
        &self,
        addr: SocketAddr,
    ) -> Result<Option<SubscriberInfo>, AppStateError> {
        let mut inner = self._lock_inner()?;

        let subscriber_id = inner
            .subscribers
            .iter()
            .find(|(_, info)| info.addr == addr)
            .map(|(id, _)| *id);

        if let Some(id) = subscriber_id {
            let sub = inner.subscribers.remove(&id).unwrap();

            if let Some(pub_data) = inner.publishers.get_mut(&sub.publisher_id) {
                pub_data.subscriber_ids.retain(|sid| *sid != id);
            }

            return Ok(Some(sub));
        } else {
            return Ok(None);
        }
    }

    pub fn cleanup_publisher_by_addr(
        &self,
        addr: SocketAddr,
    ) -> Result<Option<PublisherInfo>, AppStateError> {
        let mut inner = self._lock_inner()?;

        let publisher_id = inner
            .publishers
            .iter()
            .find(|(_, data)| data.info.addr == addr)
            .map(|(id, _)| *id);

        if let Some(id) = publisher_id {
            let publisher = inner.publishers.remove(&id).unwrap();

            // notify subscribers
            let notification = serde_json::json!({
                "kind": "response",
                "success": false,
                "message": format!("Publisher {} has been deregistered", id),
                "data": null
            });
            let _ = publisher.sender.send(notification);

            // remove all subscribers
            for sub_id in &publisher.subscriber_ids {
                inner.subscribers.remove(sub_id);
            }

            let mut info = publisher.info.clone();
            info.subscriber_count = publisher.subscriber_ids.len();

            return Ok(Some(info));
        } else {
            return Ok(None);
        }
    }
}

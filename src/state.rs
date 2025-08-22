use serde_json::Value;
use std::{collections::HashMap, net::SocketAddr, sync::Mutex};
use thiserror::Error;
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum AppStateError {
    #[error("Publisher '{id}' already exists")]
    PublisherAlreadyExists { id: Uuid },

    #[error("Publisher '{id}' not found")]
    PublisherNotFound { id: Uuid },

    #[error("Subscriber '{id}' already exists")]
    SubscriberAlreadyExists { id: Uuid },

    #[error("Subscriber '{id}' not found")]
    SubscriberNotFound { id: Uuid },

    #[error("Subscriber '{id}' is not subscribed to publisher '{publisher_id}'")]
    NotSubscribed { id: Uuid, publisher_id: Uuid },

    #[error("Failed to send message to publisher '{id}': {reason}")]
    MessageSendFailed { id: Uuid, reason: String },

    #[error("Failed to acquire lock")]
    LockError,

    #[error("Maximum subscribers ({max}) reached for publisher '{publisher_id}'")]
    MaxSubscribersReached { publisher_id: Uuid, max: usize },
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

#[derive(Debug, Default)]
pub struct AppState {
    publishers: Mutex<HashMap<Uuid, broadcast::Sender<Value>>>,
    subscribers: Mutex<HashMap<Uuid, SubscriberInfo>>,
    publisher_subscribers: Mutex<HashMap<Uuid, Vec<Uuid>>>, // publisher_id -> [subscriber_id_1, subscriber_id_2, ...]
    publisher_info: Mutex<HashMap<Uuid, PublisherInfo>>,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_publisher(&self, id: Uuid, addr: SocketAddr) -> Result<(), AppStateError> {
        let mut publishers = self
            .publishers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        let mut publisher_info = self
            .publisher_info
            .lock()
            .map_err(|_| return AppStateError::LockError)?;

        let mut publisher_subscriber = self
            .publisher_subscribers
            .lock()
            .map_err(|_| return AppStateError::LockError)?;

        if publishers.contains_key(&id) {
            return Err(AppStateError::PublisherAlreadyExists { id: id });
        }

        let (sender, _receiver) = broadcast::channel(100);
        let new_publisher_info = PublisherInfo {
            id: id,
            addr: addr,
            created_at: std::time::SystemTime::now(),
            subscriber_count: 0,
        };

        publishers.insert(id, sender);
        publisher_info.insert(id, new_publisher_info);
        publisher_subscriber.insert(id, Vec::new());

        return Ok(());
    }

    pub fn remove_publisher(&self, id: Uuid) -> Result<(), AppStateError> {
        let mut publishers = self
            .publishers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        let mut publisher_info = self
            .publisher_info
            .lock()
            .map_err(|_| return AppStateError::LockError)?;

        let mut publisher_subscribers = self
            .publisher_subscribers
            .lock()
            .map_err(|_| return AppStateError::LockError)?;

        let mut subscribers = self
            .subscribers
            .lock()
            .map_err(|_| return AppStateError::LockError)?;

        if !publishers.contains_key(&id) {
            return Err(AppStateError::SubscriberNotFound { id: id });
        }

        if let Some(subscriber_ids) = publisher_subscribers.remove(&id) {
            for subscriber_id in subscriber_ids {
                subscribers.remove(&subscriber_id);
            }
        }

        publishers.remove(&id);
        publisher_info.remove(&id);

        return Ok(());
    }

    pub fn get_publisher(&self, id: Uuid) -> Result<broadcast::Sender<Value>, AppStateError> {
        let publishers = self
            .publishers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        match publishers.get(&id) {
            Some(sender) => Ok(sender.clone()),
            None => Err(AppStateError::PublisherNotFound { id }),
        }
    }

    pub fn publisher_exists(&self, id: Uuid) -> Result<bool, AppStateError> {
        let publishers = self
            .publishers
            .lock()
            .map_err(|_| AppStateError::LockError)?;
        Ok(publishers.contains_key(&id))
    }

    pub fn add_subscriber(
        &self,
        id: Uuid,
        publisher_id: Uuid,
        addr: SocketAddr,
        max_subscribers_per_publisher: Option<usize>,
    ) -> Result<(), AppStateError> {
        let mut subscribers = self
            .subscribers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        let mut publisher_subscribers = self
            .publisher_subscribers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        let publishers = self
            .publishers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        if !publishers.contains_key(&publisher_id) {
            return Err(AppStateError::PublisherNotFound { id: publisher_id });
        }

        // drop(publishers);

        if subscribers.contains_key(&id) {
            return Err(AppStateError::SubscriberAlreadyExists { id });
        }

        if let Some(max) = max_subscribers_per_publisher {
            let current_count = publisher_subscribers
                .get(&publisher_id)
                .map(|subs| subs.len())
                .unwrap_or(0);

            if current_count >= max {
                return Err(AppStateError::MaxSubscribersReached { publisher_id, max });
            }
        }

        let new_subscriber_info = SubscriberInfo {
            id,
            publisher_id,
            addr,
            connected_at: std::time::SystemTime::now(),
        };

        subscribers.insert(id, new_subscriber_info);
        publisher_subscribers
            .entry(publisher_id)
            .or_insert_with(Vec::new)
            .push(id);

        return Ok(());
    }

    pub fn remove_subscriber(&self, id: Uuid) -> Result<SubscriberInfo, AppStateError> {
        let mut subscribers = self
            .subscribers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        let mut publisher_subscribers = self
            .publisher_subscribers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        let subscriber_info = subscribers
            .remove(&id)
            .ok_or(AppStateError::SubscriberNotFound { id })?;

        if let Some(subscriber_list) = publisher_subscribers.get_mut(&subscriber_info.publisher_id)
        {
            subscriber_list.retain(|&subscriber_id| subscriber_id != id);
        }

        return Ok(subscriber_info);
    }

    pub fn get_subscriber(&self, id: Uuid) -> Result<SubscriberInfo, AppStateError> {
        let subscribers = self
            .subscribers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        subscribers
            .get(&id)
            .cloned()
            .ok_or(AppStateError::SubscriberNotFound { id })
    }

    pub fn list_subscribers_by_publisher_id(
        &self,
        publisher_id: Uuid,
    ) -> Result<Vec<SubscriberInfo>, AppStateError> {
        let subscribers = self
            .subscribers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        let publisher_subscribers = self
            .publisher_subscribers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        let subscriber_ids = publisher_subscribers
            .get(&publisher_id)
            .ok_or(AppStateError::PublisherNotFound { id: publisher_id })?;

        let mut result = Vec::new();
        for &subscriber_id in subscriber_ids {
            if let Some(subscriber_info) = subscribers.get(&subscriber_id) {
                result.push(subscriber_info.clone());
            }
        }

        return Ok(result);
    }

    pub fn list_all_subscribers(&self) -> Result<Vec<SubscriberInfo>, AppStateError> {
        let subscribers = self
            .subscribers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        return Ok(subscribers.values().cloned().collect());
    }

    pub fn get_subscriber_count_by_publisher_id(
        &self,
        publisher_id: Uuid,
    ) -> Result<usize, AppStateError> {
        let publisher_subscribers = self
            .publisher_subscribers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        Ok(publisher_subscribers
            .get(&publisher_id)
            .map(|subs| subs.len())
            .unwrap_or(0))
    }

    pub fn publish_message(&self, publisher_id: Uuid, data: Value) -> Result<usize, AppStateError> {
        let publishers = self
            .publishers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        match publishers.get(&publisher_id) {
            Some(sender) => sender
                .send(data)
                .map_err(|e| AppStateError::MessageSendFailed {
                    id: publisher_id,
                    reason: e.to_string(),
                }),
            None => Err(AppStateError::PublisherNotFound { id: publisher_id }),
        }
    }

    pub fn cleanup_subscriber_by_addr(
        &self,
        addr: SocketAddr,
    ) -> Result<Option<SubscriberInfo>, AppStateError> {
        let mut subscribers = self
            .subscribers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        let mut publisher_subscribers = self
            .publisher_subscribers
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        // Find subscriber by address
        let subscriber_id = subscribers
            .iter()
            .find(|(_, info)| info.addr == addr)
            .map(|(id, _)| *id);

        if let Some(id) = subscriber_id {
            let subscriber_info = subscribers.remove(&id).unwrap();

            if let Some(subscriber_list) =
                publisher_subscribers.get_mut(&subscriber_info.publisher_id)
            {
                subscriber_list.retain(|&sub_id| sub_id != id);
            }

            return Ok(Some(subscriber_info));
        } else {
            return Ok(None);
        }
    }

    pub fn cleanup_publisher_by_addr(
        &self,
        addr: SocketAddr,
    ) -> Result<Option<PublisherInfo>, AppStateError> {
        let publisher_info = self
            .publisher_info
            .lock()
            .map_err(|_| AppStateError::LockError)?;

        // Find publisher by address
        let publisher_id = publisher_info
            .iter()
            .find(|(_, info)| info.addr == addr)
            .map(|(id, _)| *id);

        drop(publisher_info);

        if let Some(id) = publisher_id {
            self.remove_publisher(id)?;
            return Ok(Some(PublisherInfo {
                id,
                addr,
                created_at: std::time::SystemTime::now(),
                subscriber_count: 0,
            }));
        } else {
            return Ok(None);
        }
    }
}

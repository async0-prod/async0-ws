use std::{collections::HashMap, sync::Mutex};
use tokio::sync::broadcast;

#[derive(Debug, Default)]
pub struct AppState {
    publishers: Mutex<HashMap<String, broadcast::Sender<String>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_publisher(&self, id: String) -> Result<(), String> {
        let mut publishers = self.publishers.lock().unwrap();

        if publishers.contains_key(&id) {
            return Err(format!("Publisher '{}' already exists", id));
        }

        let (sender, _receiver) = broadcast::channel(100); // Increased capacity
        publishers.insert(id, sender);

        Ok(())
    }

    pub fn remove_publisher(&self, id: &str) -> Result<(), String> {
        let mut publishers = self.publishers.lock().unwrap();
        if !publishers.contains_key(id) {
            return Err(format!("Publisher '{}' does not exist", id));
        }

        publishers.remove(id);
        Ok(())
    }

    pub fn get_publisher(&self, id: &str) -> Result<broadcast::Sender<String>, String> {
        let publishers = self.publishers.lock().unwrap();
        match publishers.get(id) {
            Some(sender) => Ok(sender.clone()),
            None => Err(format!("Publisher '{}' does not exist", id)),
        }
    }

    // Add method to publish messages
    pub fn publish_message(&self, publisher_id: &str, message: String) -> Result<usize, String> {
        let publishers = self.publishers.lock().unwrap();
        match publishers.get(publisher_id) {
            Some(sender) => sender
                .send(message)
                .map_err(|e| format!("Failed to send message: {}", e)),
            None => Err(format!("Publisher '{}' does not exist", publisher_id)),
        }
    }

    // Get list of all publishers
    pub fn list_publishers(&self) -> Vec<String> {
        let publishers = self.publishers.lock().unwrap();
        publishers.keys().cloned().collect()
    }
}

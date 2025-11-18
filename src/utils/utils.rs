// pub async fn send_response(
//     socket: &mut WebSocket,
//     response: ServerResponse,
// ) -> Result<(), AppStateError> {
//     let json = serde_json::to_string(&response).map_err(|_| AppStateError::GenericError)?;

//     socket
//         .send(Message::Text(json.into()))
//         .await
//         .map_err(|_| AppStateError::GenericError)?;

//     Ok(())
// }

pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use crate::app;
    use axum_test::TestServer;

    #[tokio::test]
    async fn test() {
        let server = TestServer::new(app()).unwrap();
        server.get("/health").await.assert_text("Ok");
    }
}

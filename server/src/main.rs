use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    dlsrv::run().await
}

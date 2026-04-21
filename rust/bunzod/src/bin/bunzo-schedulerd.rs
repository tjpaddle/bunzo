use anyhow::Result;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    bunzod::scheduler::run_scheduler().await
}

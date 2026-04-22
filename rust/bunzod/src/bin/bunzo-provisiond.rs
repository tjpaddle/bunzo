use anyhow::Result;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--reconcile-once") => bunzod::provisioning::reconcile_runtime_state(),
        Some(other) => anyhow::bail!("unsupported bunzo-provisiond argument '{other}'"),
        None => bunzod::provisioning::run_server().await,
    }
}

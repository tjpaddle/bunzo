use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::ledger::Ledger;
use crate::runtime;
use crate::skills::{self, Registry};
use crate::store::RuntimeStore;

const CLAIM_LEASE: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_secs(1);

pub async fn run_scheduler() -> Result<()> {
    let ledger = Arc::new(Ledger::new(Ledger::default_path()));
    let store = Arc::new(RuntimeStore::new(RuntimeStore::default_path()));
    let registry = Registry::load_from(&skills::default_dir());
    let worker_id = format!("sched-{}", std::process::id());

    eprintln!(
        "bunzo-schedulerd: loaded {} skills",
        registry.tool_descriptors().len()
    );
    eprintln!("bunzo-schedulerd: watching for due jobs");

    loop {
        let claim = match store.claim_due_scheduled_job(&worker_id, CLAIM_LEASE) {
            Ok(claim) => claim,
            Err(e) => {
                eprintln!("bunzo-schedulerd: claim failed: {e:#}");
                tokio::time::sleep(POLL_INTERVAL).await;
                continue;
            }
        };

        let Some(claim) = claim else {
            tokio::time::sleep(POLL_INTERVAL).await;
            continue;
        };

        eprintln!(
            "bunzo-schedulerd: claiming job {} ({}) due at {}",
            claim.job_id, claim.name, claim.scheduled_for_ms
        );

        let request_id = format!("job-{}", claim.job_run_id);
        let request = match store.prepare_claimed_scheduled_job_request(&request_id, &claim) {
            Ok(request) => request,
            Err(e) => {
                let text = format!("{e:#}");
                eprintln!(
                    "bunzo-schedulerd: failed to prepare job {}: {}",
                    claim.job_id, text
                );
                if let Err(mark_err) =
                    store.fail_claimed_scheduled_job_run(&claim.job_run_id, &text)
                {
                    eprintln!(
                        "bunzo-schedulerd: failed to mark claimed run {} failed: {mark_err:#}",
                        claim.job_run_id
                    );
                }
                continue;
            }
        };

        let mut sink = tokio::io::sink();
        if let Err(e) = runtime::execute_prepared_request(
            &mut sink,
            &request_id,
            request,
            &ledger,
            &store,
            registry.clone(),
        )
        .await
        {
            let text = format!("{e:#}");
            eprintln!(
                "bunzo-schedulerd: job {} execution failed: {}",
                claim.job_id, text
            );
            if let Err(mark_err) = store.fail_claimed_scheduled_job_run(&claim.job_run_id, &text) {
                eprintln!(
                    "bunzo-schedulerd: failed to mark claimed run {} failed: {mark_err:#}",
                    claim.job_run_id
                );
            }
        }
    }
}

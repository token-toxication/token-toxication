use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use tokio::task::JoinHandle;

use crate::{codex_subscription::codex_account_quota, db::Db, server::ShutdownSignal};

const SCAN_INTERVAL: Duration = Duration::from_secs(30);
const FAILED_CHECK_RETRY: ChronoDuration = ChronoDuration::minutes(15);
const CHECK_BATCH_SIZE: usize = 100;

pub fn spawn(db: Db, http: aioduct::TokioClient, shutdown: ShutdownSignal) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut shutdown_rx = shutdown.subscribe();
        let mut interval = tokio::time::interval(SCAN_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow_and_update() {
                        break;
                    }
                }
                _ = interval.tick() => {
                    check_due_accounts(&db, &http).await;
                }
            }
        }
    })
}

async fn check_due_accounts(db: &Db, http: &aioduct::TokioClient) {
    let account_ids = match db
        .due_codex_limit_account_ids(Utc::now(), CHECK_BATCH_SIZE)
        .await
    {
        Ok(account_ids) => account_ids,
        Err(error) => {
            tracing::error!(error = %error, "failed to list due provider quota checks");
            return;
        }
    };

    for account_id in account_ids {
        if let Err(error) = codex_account_quota(db, http, &account_id).await {
            tracing::warn!(
                account_id = %account_id,
                error = %error,
                "provider quota recheck failed"
            );
            if let Err(schedule_error) = db
                .reschedule_provider_limits(&account_id, Utc::now() + FAILED_CHECK_RETRY)
                .await
            {
                tracing::error!(
                    account_id = %account_id,
                    error = %schedule_error,
                    "failed to reschedule provider quota check"
                );
            }
        }
    }
}

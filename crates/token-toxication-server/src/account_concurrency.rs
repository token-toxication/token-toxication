//! Per-provider-account running request limits with a FIFO wait queue.
//!
//! A permit is held from the moment a request is routed to an account until
//! the relay finishes with it (success, error, or cancellation). Waiters are
//! served strictly in arrival order; dropping a waiting future removes it from
//! the queue, and a slot granted to a waiter that was cancelled concurrently is
//! handed to the next waiter.
//!
//! The limit is captured when an account's slot state is created and is only
//! changed afterwards through [`AccountConcurrency::set_limit`], so requests
//! holding a stale account snapshot cannot overwrite a newer limit. Slot
//! state is kept for the account's lifetime (one entry per account) and is
//! dropped by [`AccountConcurrency::forget`] when the account is deleted.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

use tokio::sync::oneshot;

#[derive(Clone, Default)]
pub struct AccountConcurrency {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    accounts: HashMap<String, AccountSlot>,
    next_waiter_id: u64,
}

#[derive(Default)]
struct AccountSlot {
    /// Zero means unlimited.
    limit: u32,
    running: u32,
    waiters: VecDeque<Waiter>,
}

struct Waiter {
    id: u64,
    grant: oneshot::Sender<()>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AccountLoad {
    pub running: u32,
    pub queued: u32,
}

impl AccountSlot {
    fn new(limit: u32) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }

    fn has_capacity(&self) -> bool {
        self.limit == 0 || self.running < self.limit
    }

    /// Hands free slots to queued waiters in FIFO order.
    fn dispatch(&mut self) {
        while self.has_capacity() {
            let Some(waiter) = self.waiters.pop_front() else {
                break;
            };
            self.running += 1;
            if waiter.grant.send(()).is_err() {
                // The waiter was cancelled after leaving the queue check.
                self.running -= 1;
            }
        }
    }

    fn is_idle(&self) -> bool {
        self.running == 0 && self.waiters.is_empty()
    }
}

impl AccountConcurrency {
    /// Acquires a slot only when the account has capacity and nobody is queued.
    /// `limit` is used only if the account has no slot state yet.
    pub fn try_acquire(&self, account_id: &str, limit: u32) -> Option<AccountPermit> {
        let mut inner = self.lock();
        let slot = inner
            .accounts
            .entry(account_id.to_owned())
            .or_insert_with(|| AccountSlot::new(limit));
        if slot.waiters.is_empty() && slot.has_capacity() {
            slot.running += 1;
            Some(self.permit(account_id))
        } else {
            None
        }
    }

    /// Waits without a timeout until a slot is available. Cancel-safe.
    /// `limit` is used only if the account has no slot state yet.
    pub async fn acquire(&self, account_id: &str, limit: u32) -> AccountPermit {
        let (grant, receiver) = oneshot::channel();
        let id = {
            let mut inner = self.lock();
            let id = inner.next_waiter_id;
            inner.next_waiter_id += 1;
            let slot = inner
                .accounts
                .entry(account_id.to_owned())
                .or_insert_with(|| AccountSlot::new(limit));
            if slot.waiters.is_empty() && slot.has_capacity() {
                slot.running += 1;
                drop(inner);
                return self.permit(account_id);
            }
            slot.waiters.push_back(Waiter { id, grant });
            id
        };
        let mut queued = QueuedWaiter {
            concurrency: self,
            account_id,
            id,
            receiver: Some(receiver),
        };
        let receiver = queued.receiver.as_mut().expect("receiver present");
        // The sender lives in the queue until a grant, so it is never dropped
        // while this future is alive.
        let _ = receiver.await;
        queued.receiver = None;
        self.permit(account_id)
    }

    /// Applies a changed limit immediately so raised limits wake waiters.
    pub fn set_limit(&self, account_id: &str, limit: u32) {
        let mut inner = self.lock();
        let slot = inner
            .accounts
            .entry(account_id.to_owned())
            .or_insert_with(|| AccountSlot::new(limit));
        slot.limit = limit;
        slot.dispatch();
    }

    /// Drops idle state for a deleted account.
    pub fn forget(&self, account_id: &str) {
        let mut inner = self.lock();
        if inner
            .accounts
            .get(account_id)
            .is_some_and(AccountSlot::is_idle)
        {
            inner.accounts.remove(account_id);
        }
    }

    pub fn load(&self, account_id: &str) -> AccountLoad {
        self.lock()
            .accounts
            .get(account_id)
            .map(|slot| AccountLoad {
                running: slot.running,
                queued: slot.waiters.len() as u32,
            })
            .unwrap_or_default()
    }

    fn permit(&self, account_id: &str) -> AccountPermit {
        AccountPermit {
            concurrency: self.clone(),
            account_id: account_id.to_owned(),
        }
    }

    fn release(&self, account_id: &str) {
        let mut inner = self.lock();
        if let Some(slot) = inner.accounts.get_mut(account_id) {
            slot.running = slot.running.saturating_sub(1);
            slot.dispatch();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .expect("account concurrency lock poisoned")
    }
}

struct QueuedWaiter<'a> {
    concurrency: &'a AccountConcurrency,
    account_id: &'a str,
    id: u64,
    receiver: Option<oneshot::Receiver<()>>,
}

impl Drop for QueuedWaiter<'_> {
    fn drop(&mut self) {
        let Some(mut receiver) = self.receiver.take() else {
            return;
        };
        let granted = {
            let mut inner = self.concurrency.lock();
            let Some(slot) = inner.accounts.get_mut(self.account_id) else {
                return;
            };
            if let Some(position) = slot.waiters.iter().position(|waiter| waiter.id == self.id) {
                slot.waiters.remove(position);
                false
            } else {
                // Grants happen under the lock, so a missing waiter was granted.
                receiver.try_recv().is_ok()
            }
        };
        if granted {
            self.concurrency.release(self.account_id);
        }
    }
}

/// Releases the account slot when dropped.
pub struct AccountPermit {
    concurrency: AccountConcurrency,
    account_id: String,
}

impl std::fmt::Debug for AccountPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AccountPermit")
            .field("account_id", &self.account_id)
            .finish()
    }
}

impl Drop for AccountPermit {
    fn drop(&mut self) {
        self.concurrency.release(&self.account_id);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn load(concurrency: &AccountConcurrency) -> (u32, u32) {
        let load = concurrency.load("a");
        (load.running, load.queued)
    }

    #[tokio::test]
    async fn unlimited_accounts_never_queue() {
        let concurrency = AccountConcurrency::default();
        let permits: Vec<_> = (0..10)
            .map(|_| concurrency.try_acquire("a", 0).expect("unlimited"))
            .collect();
        assert_eq!(load(&concurrency), (10, 0));
        drop(permits);
        assert_eq!(load(&concurrency), (0, 0));
    }

    #[tokio::test]
    async fn try_acquire_respects_limit() {
        let concurrency = AccountConcurrency::default();
        let first = concurrency.try_acquire("a", 1).expect("first");
        assert!(concurrency.try_acquire("a", 1).is_none());
        assert!(concurrency.try_acquire("b", 1).is_some());
        drop(first);
        assert!(concurrency.try_acquire("a", 1).is_some());
    }

    #[tokio::test]
    async fn waiters_are_served_in_fifo_order() {
        let concurrency = AccountConcurrency::default();
        let held = concurrency.try_acquire("a", 1).expect("held");
        let (order_tx, mut order_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tasks = Vec::new();
        for index in 0..3 {
            let task_concurrency = concurrency.clone();
            let order_tx = order_tx.clone();
            tasks.push(tokio::spawn(async move {
                let permit = task_concurrency.acquire("a", 1).await;
                order_tx.send(index).unwrap();
                tokio::time::sleep(Duration::from_millis(5)).await;
                drop(permit);
            }));
            // Make enqueue order deterministic.
            while concurrency.load("a").queued as usize != index + 1 {
                tokio::task::yield_now().await;
            }
        }
        assert!(
            concurrency.try_acquire("a", 1).is_none(),
            "new arrivals must not jump the queue"
        );
        drop(held);
        for task in tasks {
            task.await.unwrap();
        }
        drop(order_tx);
        let mut order = Vec::new();
        while let Some(index) = order_rx.recv().await {
            order.push(index);
        }
        assert_eq!(order, vec![0, 1, 2]);
        assert_eq!(load(&concurrency), (0, 0));
    }

    #[tokio::test]
    async fn cancelled_waiter_leaves_queue() {
        let concurrency = AccountConcurrency::default();
        let held = concurrency.try_acquire("a", 1).expect("held");
        let waiting = {
            let concurrency = concurrency.clone();
            tokio::spawn(async move { concurrency.acquire("a", 1).await })
        };
        while concurrency.load("a").queued == 0 {
            tokio::task::yield_now().await;
        }
        waiting.abort();
        let _ = waiting.await;
        assert_eq!(load(&concurrency), (1, 0));
        drop(held);
        assert_eq!(load(&concurrency), (0, 0));
        assert!(concurrency.try_acquire("a", 1).is_some());
    }

    #[tokio::test]
    async fn granted_but_cancelled_waiter_returns_slot() {
        let concurrency = AccountConcurrency::default();
        let held = concurrency.try_acquire("a", 1).expect("held");
        let mut waiting = Box::pin(concurrency.acquire("a", 1));
        assert!(futures_util::poll!(waiting.as_mut()).is_pending());
        // Grant the slot to the waiter, then cancel before it observes the grant.
        drop(held);
        assert_eq!(load(&concurrency), (1, 0));
        drop(waiting);
        assert_eq!(load(&concurrency), (0, 0));
    }

    #[tokio::test]
    async fn stale_limits_do_not_overwrite_the_current_limit() {
        let concurrency = AccountConcurrency::default();
        let _first = concurrency.try_acquire("a", 2).expect("first");
        // A request with an older snapshot (limit 1) must still see limit 2.
        let _second = concurrency.try_acquire("a", 1).expect("second");
        // A snapshot with a higher limit must not lift it either.
        assert!(concurrency.try_acquire("a", 5).is_none());
    }

    #[tokio::test]
    async fn set_limit_applies_before_any_request_arrives() {
        let concurrency = AccountConcurrency::default();
        concurrency.set_limit("a", 1);
        let _first = concurrency.try_acquire("a", 0).expect("first");
        assert!(
            concurrency.try_acquire("a", 0).is_none(),
            "a stale unlimited snapshot must not lift the configured limit"
        );
    }

    #[tokio::test]
    async fn raising_limit_wakes_waiters() {
        let concurrency = AccountConcurrency::default();
        let _held = concurrency.try_acquire("a", 1).expect("held");
        let waiting = {
            let concurrency = concurrency.clone();
            tokio::spawn(async move { concurrency.acquire("a", 1).await })
        };
        while concurrency.load("a").queued == 0 {
            tokio::task::yield_now().await;
        }
        concurrency.set_limit("a", 2);
        let permit = tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("woken")
            .unwrap();
        assert_eq!(load(&concurrency), (2, 0));
        drop(permit);
    }
}

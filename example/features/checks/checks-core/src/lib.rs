//! checks business logic: watch URLs, record what they answered.
//!
//! This crate is runtime-free and wasm-safe. Every effect — the probe, the clock
//! the interval is measured against, the wait between retries — arrives through
//! `Caps`, so the retry ladder runs at full speed under a virtual clock and
//! against a real socket in a host, from one implementation.

use checks_api::{
    check_err, Check, CheckErr, CheckError, CheckId, CheckReport, CheckService, NewCheck, Probe,
};
use gabion::prelude::*;

/// How long a watch must sit before it is due again.
const INTERVAL_SECS: u64 = 60;
/// How long one attempt may take before it counts as no answer.
const TIMEOUT_MS: u64 = 2_000;
/// Tries before a probe is recorded as a miss.
const ATTEMPTS: u32 = 3;
/// First wait between tries; each one after it doubles.
const BACKOFF_MS: u64 = 250;
/// Observations kept per watch before the oldest are dropped.
const HISTORY: usize = 50;

pub struct CheckServiceImpl<C: Caps> {
    caps: C,
}

impl<C: Caps> CheckServiceImpl<C> {
    pub fn new(caps: C) -> Self {
        Self { caps }
    }

    fn check_key(id: &str) -> String {
        format!("check/{id}")
    }

    /// Zero-padded so the store's lexicographic order is chronological order.
    fn probe_key(id: &str, at: u64, nonce: &str) -> String {
        format!("probe/{id}/{at:019}-{nonce}")
    }

    fn probe_prefix(id: &str) -> String {
        format!("probe/{id}/")
    }

    /// Short unpredictable suffix for ids. The clock alone is not an id: two
    /// probes in the same second would overwrite each other in silence.
    async fn nonce(&self) -> String {
        format!("{:08x}", self.caps.random().next_u64().await as u32)
    }

    async fn load(&self, id: &str) -> Result<Check, CheckErr> {
        let raw = self
            .caps
            .store()
            .get(&Self::check_key(id))
            .await
            .map_err(|_| check_err(CheckError::Internal))?;
        raw.and_then(|s| gabion::serde_json::from_str::<Check>(&s).ok())
            .ok_or_else(|| check_err(CheckError::NotFound))
    }

    async fn watches(&self) -> Result<Vec<Check>, CheckErr> {
        let items = self
            .caps
            .store()
            .list("check/")
            .await
            .map_err(|_| check_err(CheckError::Internal))?;
        items
            .iter()
            .map(|(_, v)| {
                gabion::serde_json::from_str::<Check>(v)
                    .map_err(|_| check_err(CheckError::Internal))
            })
            .collect()
    }

    async fn observations(&self, id: &str) -> Result<Vec<Probe>, CheckErr> {
        let items = self
            .caps
            .store()
            .list(&Self::probe_prefix(id))
            .await
            .map_err(|_| check_err(CheckError::Internal))?;
        items
            .iter()
            .map(|(_, v)| {
                gabion::serde_json::from_str::<Probe>(v)
                    .map_err(|_| check_err(CheckError::Internal))
            })
            .collect()
    }

    async fn latest(&self, id: &str) -> Result<Option<Probe>, CheckErr> {
        Ok(self.observations(id).await?.pop())
    }

    async fn reports(&self) -> Result<Vec<CheckReport>, CheckErr> {
        let mut out = Vec::new();
        for check in self.watches().await? {
            let last = self.latest(&check.id).await?;
            out.push(CheckReport { check, last });
        }
        Ok(out)
    }

    async fn record(&self, probe: &Probe) -> Result<(), CheckErr> {
        let key = Self::probe_key(&probe.check_id, probe.at, &self.nonce().await);
        let json =
            gabion::serde_json::to_string(probe).map_err(|_| check_err(CheckError::Internal))?;
        self.caps
            .store()
            .set(&key, &json)
            .await
            .map_err(|_| check_err(CheckError::Internal))?;
        self.trim(&probe.check_id).await
    }

    async fn trim(&self, id: &str) -> Result<(), CheckErr> {
        let keys = self
            .caps
            .store()
            .list(&Self::probe_prefix(id))
            .await
            .map_err(|_| check_err(CheckError::Internal))?;
        if keys.len() <= HISTORY {
            return Ok(());
        }
        for (key, _) in keys.iter().take(keys.len() - HISTORY) {
            self.caps
                .store()
                .delete(key)
                .await
                .map_err(|_| check_err(CheckError::Internal))?;
        }
        Ok(())
    }

    /// One try. `Err` means no response arrived at all; a `500` is an answer.
    async fn attempt(&self, check: &Check) -> Result<(u16, u64), String> {
        let http = self.caps.http().clone();
        let request = HttpRequest::get(check.url.clone());
        let started = self.caps.clock().now().await.0;
        let sent = Box::pin(async move { http.send(request).await });
        let outcome = timeout(self.caps.sleep(), check.timeout_ms, sent).await;
        let latency_ms = self.caps.clock().now().await.0.saturating_sub(started) * 1000;
        match outcome {
            Ok(Ok(response)) => Ok((response.status, latency_ms)),
            Ok(Err(_)) => Err("no connection".to_string()),
            Err(_) => Err("no answer before the deadline".to_string()),
        }
    }

    fn observed(check: &Check, at: u64, attempts: u32, latency_ms: u64, status: u16) -> Probe {
        let ok = (200..300).contains(&status);
        Probe {
            check_id: check.id.clone(),
            at,
            ok,
            status,
            latency_ms,
            attempts,
            note: if ok {
                String::new()
            } else {
                format!("status {status}")
            },
        }
    }

    fn miss(check: &Check, at: u64, attempts: u32, note: String) -> Probe {
        Probe {
            check_id: check.id.clone(),
            at,
            ok: false,
            status: 0,
            latency_ms: 0,
            attempts,
            note,
        }
    }

    /// Up to `ATTEMPTS` tries, with a doubling wait between them.
    async fn run_probe(&self, check: &Check) -> Probe {
        let started = self.caps.clock().now().await.0;
        let mut delay = BACKOFF_MS;
        let mut attempt = 1;
        let (status, latency_ms, attempts) = loop {
            match self.attempt(check).await {
                Ok((status, latency_ms)) => break (status, latency_ms, attempt),
                Err(note) if attempt == ATTEMPTS => {
                    return Self::miss(check, started, attempt, note)
                }
                Err(_) => {}
            }
            self.caps.sleep().sleep(delay).await;
            delay = delay.saturating_mul(2);
            attempt += 1;
        };
        Self::observed(check, started, attempts, latency_ms, status)
    }
}

#[async_trait]
impl<C: Caps> CheckService for CheckServiceImpl<C> {
    async fn add(&self, input: NewCheck) -> Result<Check, CheckErr> {
        let scheme_ok = input.url.starts_with("http://") || input.url.starts_with("https://");
        if !scheme_ok {
            return Err(check_err(CheckError::InvalidUrl));
        }
        if input.name.trim().is_empty() {
            return Err(check_err(CheckError::InvalidUrl));
        }
        let now = self.caps.clock().now().await;
        let check = Check {
            id: format!("c{}-{}", now.0, self.nonce().await),
            name: input.name,
            url: input.url,
            interval_secs: INTERVAL_SECS,
            timeout_ms: TIMEOUT_MS,
            created_at: now.0,
        };
        let json =
            gabion::serde_json::to_string(&check).map_err(|_| check_err(CheckError::Internal))?;
        self.caps
            .store()
            .set(&Self::check_key(&check.id), &json)
            .await
            .map_err(|_| check_err(CheckError::Internal))?;
        Ok(check)
    }

    async fn list(&self, _input: ()) -> Result<Vec<CheckReport>, CheckErr> {
        self.reports().await
    }

    async fn probe(&self, input: CheckId) -> Result<Probe, CheckErr> {
        let check = self.load(&input.0).await?;
        let probe = self.run_probe(&check).await;
        self.record(&probe).await?;
        Ok(probe)
    }

    async fn sweep(&self, _input: ()) -> Result<Vec<Probe>, CheckErr> {
        let now = self.caps.clock().now().await.0;
        let mut out = Vec::new();
        for report in self.reports().await? {
            let due = match &report.last {
                None => true,
                Some(last) => now.saturating_sub(last.at) >= report.check.interval_secs,
            };
            if !due {
                continue;
            }
            let probe = self.run_probe(&report.check).await;
            self.record(&probe).await?;
            out.push(probe);
        }
        Ok(out)
    }

    async fn history(&self, input: CheckId) -> Result<Vec<Probe>, CheckErr> {
        self.load(&input.0).await?;
        self.observations(&input.0).await
    }
}

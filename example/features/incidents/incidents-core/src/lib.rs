//! incidents business logic: turn what the watches saw into outages.
//!
//! Everything here reaches `checks` through the `checks-api` contract. There is
//! no `checks-core` dependency, so this crate cannot see a probe's storage keys,
//! its retry ladder, or anything else an implementation might change. The same
//! call an HTTP client makes is the call this code makes.

use checks_api::{CheckErr, CheckId, CheckReport, CheckService, Probe};
use gabion::prelude::*;
use incidents_api::{incident_err, Incident, IncidentErr, IncidentError, IncidentService, Uptime};

/// Consecutive misses before an outage counts as real.
const FAILS_TO_OPEN: usize = 2;
/// Probes uptime looks at, newest first.
const WINDOW: usize = 20;

pub struct IncidentServiceImpl<C: Caps, S: CheckService> {
    caps: C,
    checks: S,
}

impl<C: Caps, S: CheckService> IncidentServiceImpl<C, S> {
    pub fn new(caps: C, checks: S) -> Self {
        Self { caps, checks }
    }

    fn incident_key(id: &str) -> String {
        format!("incident/{id}")
    }

    async fn nonce(&self) -> String {
        format!("{:08x}", self.caps.random().next_u64().await as u32)
    }

    /// A neighbour's `NotFound` is this contract's `NotFound`; anything else a
    /// caller has no business hearing about is `Internal`.
    fn forward(error: CheckErr) -> IncidentErr {
        match error.kind {
            ErrorKind::NotFound => incident_err(IncidentError::NotFound),
            _ => incident_err(IncidentError::Internal),
        }
    }

    async fn all(&self) -> Result<Vec<Incident>, IncidentErr> {
        let items = self
            .caps
            .store()
            .list("incident/")
            .await
            .map_err(|_| incident_err(IncidentError::Internal))?;
        items
            .iter()
            .map(|(_, v)| {
                gabion::serde_json::from_str::<Incident>(v)
                    .map_err(|_| incident_err(IncidentError::Internal))
            })
            .collect()
    }

    async fn save(&self, incident: &Incident) -> Result<(), IncidentErr> {
        let json = gabion::serde_json::to_string(incident)
            .map_err(|_| incident_err(IncidentError::Internal))?;
        self.caps
            .store()
            .set(&Self::incident_key(&incident.id), &json)
            .await
            .map_err(|_| incident_err(IncidentError::Internal))?;
        Ok(())
    }

    async fn open_one(&self, check_id: &str) -> Result<Option<Incident>, IncidentErr> {
        Ok(self
            .all()
            .await?
            .into_iter()
            .find(|i| i.check_id == check_id && i.closed_at.is_none()))
    }

    /// Closes an outage once the newest observation came back.
    async fn recovered(
        &self,
        open: &Incident,
        history: &[Probe],
    ) -> Result<Option<Incident>, IncidentErr> {
        let Some(last) = history.last() else {
            return Ok(None);
        };
        if !last.ok {
            return Ok(None);
        }
        let mut done = open.clone();
        done.closed_at = Some(self.caps.clock().now().await.0);
        self.save(&done).await?;
        Ok(Some(done))
    }

    /// Opens an outage once the newest observations are all misses. The incident
    /// is dated at the first miss of the run, not at the moment it was noticed.
    async fn broken(
        &self,
        report: &CheckReport,
        history: &[Probe],
    ) -> Result<Option<Incident>, IncidentErr> {
        let run: Vec<&Probe> = history.iter().rev().take_while(|p| !p.ok).collect();
        let Some(start) = run.last().copied() else {
            return Ok(None);
        };
        if run.len() < FAILS_TO_OPEN {
            return Ok(None);
        }
        let fresh = Incident {
            id: format!(
                "i{}-{}",
                self.caps.clock().now().await.0,
                self.nonce().await
            ),
            check_id: report.check.id.clone(),
            check_name: report.check.name.clone(),
            opened_at: start.at,
            closed_at: None,
            cause: if start.note.is_empty() {
                format!("status {}", start.status)
            } else {
                start.note.clone()
            },
        };
        self.save(&fresh).await?;
        Ok(Some(fresh))
    }
}

#[async_trait]
impl<C: Caps, S: CheckService + Sync> IncidentService for IncidentServiceImpl<C, S> {
    async fn evaluate(&self, _input: ()) -> Result<Vec<Incident>, IncidentErr> {
        let mut changed = Vec::new();
        for report in self.checks.list(()).await.map_err(Self::forward)? {
            let id = report.check.id.clone();
            let history = self
                .checks
                .history(CheckId(id))
                .await
                .map_err(Self::forward)?;
            match self.open_one(&report.check.id).await? {
                Some(open) => {
                    if let Some(done) = self.recovered(&open, &history).await? {
                        changed.push(done);
                    }
                }
                None => {
                    if let Some(fresh) = self.broken(&report, &history).await? {
                        changed.push(fresh);
                    }
                }
            }
        }
        Ok(changed)
    }

    async fn list(&self, _input: ()) -> Result<Vec<Incident>, IncidentErr> {
        let mut all = self.all().await?;
        all.sort_by(|a, b| {
            a.closed_at
                .is_some()
                .cmp(&b.closed_at.is_some())
                .then(b.opened_at.cmp(&a.opened_at))
        });
        Ok(all)
    }

    async fn uptime(&self, input: CheckId) -> Result<Uptime, IncidentErr> {
        let check_id = input.0.clone();
        let history = self.checks.history(input).await.map_err(Self::forward)?;
        let window: Vec<&Probe> = history.iter().rev().take(WINDOW).collect();
        let probes = window.len();
        let ok = window.iter().filter(|p| p.ok).count();
        let pct = ok
            .checked_mul(100)
            .and_then(|hits| hits.checked_div(probes))
            .map_or(0, |pct| pct as u8);
        Ok(Uptime {
            check_id,
            probes: probes as u32,
            ok: ok as u32,
            pct,
        })
    }
}

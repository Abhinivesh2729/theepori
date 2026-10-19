//! In-memory key holder (the port of Panuval's key agent, minus the socket).
//!
//! tpd keeps one [`KeyHolder`]. It owns an [`Unlocked`] vault and drops it
//! (wiping the DEK) on [`KeyHolder::lock`], when its absolute TTL or the
//! lease expires, or after an idle timeout. Access goes through a [`Lease`]:
//! a remote (phone) unlock grants only the scopes the founder approved.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use zeroize::Zeroizing;

use crate::error::Result;
use crate::scope::Scope;
use crate::vault::Unlocked;

/// Source of the current time (injectable for tests).
pub type Clock = Arc<dyn Fn() -> OffsetDateTime + Send + Sync>;

/// Which scopes a lease grants.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseScopes {
    /// Every scope, plus administrative access ([`KeyHolder::with_unlocked`]).
    All,
    /// Only these exact scopes. A granted scope also sees the `global`
    /// layers it inherits (see [`Scope::layers`]).
    Only(Vec<Scope>),
}

/// A time-limited grant of access to some scopes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub scopes: LeaseScopes,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

impl Lease {
    pub fn all(expires_at: OffsetDateTime) -> Self {
        Self {
            scopes: LeaseScopes::All,
            expires_at,
        }
    }

    pub fn only(scopes: Vec<Scope>, expires_at: OffsetDateTime) -> Self {
        Self {
            scopes: LeaseScopes::Only(scopes),
            expires_at,
        }
    }

    /// Whether the lease grants `scope` (ignoring expiry).
    pub fn covers(&self, scope: &Scope) -> bool {
        match &self.scopes {
            LeaseScopes::All => true,
            LeaseScopes::Only(v) => v.contains(scope),
        }
    }
}

/// Lifetime limits for a [`KeyHolder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HolderConfig {
    /// Absolute lifetime from unlock.
    pub ttl: Duration,
    /// Lock after this long without a successful access (`None` = never).
    pub idle: Option<Duration>,
}

impl Default for HolderConfig {
    /// Panuval's defaults: 8 h TTL, 30 min idle.
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(8 * 3600),
            idle: Some(Duration::from_secs(30 * 60)),
        }
    }
}

/// A snapshot of the holder's state (no secrets).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HolderStatus {
    pub unlocked: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub idle_until: Option<OffsetDateTime>,
    pub lease: Option<Lease>,
}

struct Inner {
    unlocked: Option<Unlocked>,
    lease: Lease,
    deadline: OffsetDateTime,
    idle: Option<Duration>,
    last_use: OffsetDateTime,
}

impl Inner {
    fn idle_until(&self) -> Option<OffsetDateTime> {
        self.idle.map(|i| add(self.last_use, i))
    }

    /// Wipes the key if any limit has passed; returns whether still unlocked.
    fn alive(&mut self, now: OffsetDateTime) -> bool {
        if self.unlocked.is_none() {
            return false;
        }
        if now >= self.deadline || self.idle_until().is_some_and(|t| now >= t) {
            self.unlocked = None;
            return false;
        }
        true
    }
}

fn add(t: OffsetDateTime, d: Duration) -> OffsetDateTime {
    time::Duration::try_from(d)
        .ok()
        .and_then(|d| t.checked_add(d))
        .unwrap_or(OffsetDateTime::new_utc(
            time::Date::MAX,
            time::Time::MIDNIGHT,
        ))
}

/// Holds an unlocked vault in memory under a TTL, an idle timeout and a lease.
pub struct KeyHolder {
    inner: Mutex<Inner>,
    clock: Clock,
}

impl fmt::Debug for KeyHolder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyHolder")
            .field("status", &self.status())
            .finish()
    }
}

impl KeyHolder {
    pub fn new(unlocked: Unlocked, cfg: HolderConfig, lease: Lease) -> Self {
        Self::with_clock(unlocked, cfg, lease, Arc::new(OffsetDateTime::now_utc))
    }

    pub fn with_clock(unlocked: Unlocked, cfg: HolderConfig, lease: Lease, clock: Clock) -> Self {
        let now = clock();
        let deadline = add(now, cfg.ttl).min(lease.expires_at);
        Self {
            inner: Mutex::new(Inner {
                unlocked: Some(unlocked),
                lease,
                deadline,
                idle: cfg.idle,
                last_use: now,
            }),
            clock,
        }
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the key is still held (wipes it first if a limit has passed).
    /// Call periodically from a reaper so expiry happens without an access.
    pub fn is_unlocked(&self) -> bool {
        let now = (self.clock)();
        self.inner().alive(now)
    }

    pub fn status(&self) -> HolderStatus {
        let now = (self.clock)();
        let mut g = self.inner();
        if g.alive(now) {
            HolderStatus {
                unlocked: true,
                expires_at: Some(g.deadline),
                idle_until: g.idle_until(),
                lease: Some(g.lease.clone()),
            }
        } else {
            HolderStatus {
                unlocked: false,
                expires_at: None,
                idle_until: None,
                lease: None,
            }
        }
    }

    /// Access to `scope` if the vault is unlocked and the lease grants it.
    /// Counts as activity for the idle timeout. The returned guard holds the
    /// holder's mutex: keep it short and do not call other holder methods
    /// while holding it.
    pub fn get_for(&self, scope: &Scope) -> Option<ScopedAccess<'_>> {
        let now = (self.clock)();
        let mut g = self.inner();
        if !g.alive(now) || !g.lease.covers(scope) {
            return None;
        }
        g.last_use = now;
        Some(ScopedAccess {
            guard: g,
            scope: scope.clone(),
        })
    }

    /// Full read/write access; only under an [`LeaseScopes::All`] lease.
    pub fn with_unlocked<R>(&self, f: impl FnOnce(&mut Unlocked) -> R) -> Option<R> {
        let now = (self.clock)();
        let mut g = self.inner();
        if !g.alive(now) || g.lease.scopes != LeaseScopes::All {
            return None;
        }
        g.last_use = now;
        g.unlocked.as_mut().map(f)
    }

    /// Drops the unlocked vault, wiping the DEK.
    pub fn lock(&self) {
        self.inner().unlocked = None;
    }
}

/// Read access to one leased scope (with its inherited `global` layers).
pub struct ScopedAccess<'a> {
    guard: MutexGuard<'a, Inner>,
    scope: Scope,
}

impl fmt::Debug for ScopedAccess<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScopedAccess")
            .field("scope", &self.scope)
            .finish()
    }
}

impl ScopedAccess<'_> {
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    fn unlocked(&self) -> &Unlocked {
        // `get_for` only hands out a guard while the vault is held, and the
        // guard keeps the mutex, so nothing can lock it in between.
        match self.guard.unlocked.as_ref() {
            Some(u) => u,
            None => unreachable!("ScopedAccess exists only while unlocked"),
        }
    }

    /// The process environment for the scope ([`Unlocked::env_for`]).
    pub fn env(&self) -> Result<Vec<(String, Zeroizing<String>)>> {
        self.unlocked().env_for(&self.scope)
    }

    /// The effective value of `name` in the scope ([`Unlocked::resolve`]).
    pub fn get(&self, name: &str) -> Result<Zeroizing<String>> {
        self.unlocked().resolve(&self.scope, name)
    }
}

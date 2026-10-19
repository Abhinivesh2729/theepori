use std::sync::{Arc, Mutex};
use std::time::Duration;

use time::OffsetDateTime;
use tp_vault::crypto::KdfParams;
use tp_vault::{Clock, HolderConfig, KeyHolder, Lease, Scope, Unlocked, Vault};

const PW: &str = "correct horse battery staple";

struct FakeClock(Arc<Mutex<OffsetDateTime>>);

impl FakeClock {
    fn new() -> (Self, Clock) {
        let t = Arc::new(Mutex::new(OffsetDateTime::now_utc()));
        let c = t.clone();
        let clock: Clock = Arc::new(move || *c.lock().expect("clock"));
        (Self(t), clock)
    }

    fn now(&self) -> OffsetDateTime {
        *self.0.lock().expect("clock")
    }

    fn advance(&self, secs: i64) {
        *self.0.lock().expect("clock") += time::Duration::seconds(secs);
    }
}

fn unlocked() -> (tempfile::TempDir, Unlocked) {
    let tmp = tempfile::tempdir().expect("tempdir");
    Vault::init_with_kdf(
        tmp.path(),
        PW,
        KdfParams::with_cost(1024, 1, 1).expect("kdf"),
    )
    .expect("init");
    let mut u = Vault::open(tmp.path())
        .expect("open")
        .unlock(PW)
        .expect("unlock");
    u.set(&Scope::global(), "SHARED", "g").expect("set");
    u.set(&Scope::new("thinai", "prod"), "OPENAI_API_KEY", "sk-thinai")
        .expect("set");
    u.set(&Scope::new("kavi", "prod"), "OPENAI_API_KEY", "sk-kavi")
        .expect("set");
    (tmp, u)
}

const HOUR: Duration = Duration::from_secs(3600);

#[test]
fn scoped_lease_only_grants_its_scopes() {
    let (_t, u) = unlocked();
    let (clock, c) = FakeClock::new();
    let thinai = Scope::new("thinai", "prod");
    let lease = Lease::only(vec![thinai.clone()], clock.now() + time::Duration::hours(1));
    let h = KeyHolder::with_clock(
        u,
        HolderConfig {
            ttl: HOUR * 8,
            idle: None,
        },
        lease,
        c,
    );

    let access = h.get_for(&thinai).expect("leased scope");
    assert_eq!(*access.get("OPENAI_API_KEY").expect("get"), "sk-thinai");
    assert_eq!(*access.get("SHARED").expect("inherits global"), "g");
    let env = access.env().expect("env");
    let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, vec!["OPENAI_API_KEY", "SHARED"]);
    drop(access);

    assert!(h.get_for(&Scope::new("kavi", "prod")).is_none());
    assert!(h.get_for(&Scope::new("thinai", "dev")).is_none());
    assert!(h.get_for(&Scope::global()).is_none());
    assert!(
        h.with_unlocked(|_| ()).is_none(),
        "no admin under a scoped lease"
    );
    assert!(h.is_unlocked());
}

#[test]
fn all_lease_allows_admin_and_every_scope() {
    let (_t, u) = unlocked();
    let h = KeyHolder::new(
        u,
        HolderConfig::default(),
        Lease::all(OffsetDateTime::now_utc() + time::Duration::hours(1)),
    );
    assert!(h.get_for(&Scope::new("kavi", "prod")).is_some());
    h.with_unlocked(|u| u.set(&Scope::global(), "NEW", "n"))
        .expect("admin")
        .expect("set");
    let a = h.get_for(&Scope::global()).expect("global");
    assert_eq!(*a.get("NEW").expect("get"), "n");
}

#[test]
fn lease_expiry_wipes_the_key() {
    let (_t, u) = unlocked();
    let (clock, c) = FakeClock::new();
    let lease = Lease::all(clock.now() + time::Duration::minutes(10));
    let h = KeyHolder::with_clock(
        u,
        HolderConfig {
            ttl: HOUR * 8,
            idle: None,
        },
        lease,
        c,
    );
    assert_eq!(
        h.status().expires_at,
        Some(clock.now() + time::Duration::minutes(10)),
        "deadline is the earlier of TTL and lease"
    );
    clock.advance(9 * 60);
    assert!(h.get_for(&Scope::global()).is_some());
    clock.advance(60);
    assert!(h.get_for(&Scope::global()).is_none());
    assert!(!h.is_unlocked());
    // Stays locked even if the clock goes back.
    clock.advance(-3600);
    assert!(h.get_for(&Scope::global()).is_none());
    assert!(!h.status().unlocked);
}

#[test]
fn ttl_expiry() {
    let (_t, u) = unlocked();
    let (clock, c) = FakeClock::new();
    let lease = Lease::all(clock.now() + time::Duration::days(30));
    let h = KeyHolder::with_clock(
        u,
        HolderConfig {
            ttl: HOUR,
            idle: None,
        },
        lease,
        c,
    );
    clock.advance(3599);
    assert!(h.is_unlocked());
    clock.advance(1);
    assert!(!h.is_unlocked());
    assert!(h.with_unlocked(|_| ()).is_none());
}

#[test]
fn idle_timeout_and_activity() {
    let (_t, u) = unlocked();
    let (clock, c) = FakeClock::new();
    let lease = Lease::all(clock.now() + time::Duration::days(1));
    let cfg = HolderConfig {
        ttl: HOUR * 8,
        idle: Some(Duration::from_secs(300)),
    };
    let h = KeyHolder::with_clock(u, cfg, lease, c);
    for _ in 0..5 {
        clock.advance(299);
        assert!(
            h.get_for(&Scope::global()).is_some(),
            "activity resets idle"
        );
    }
    assert_eq!(
        h.status().idle_until,
        Some(clock.now() + time::Duration::seconds(300))
    );
    clock.advance(299);
    assert!(h.is_unlocked(), "status checks are not activity");
    clock.advance(1);
    assert!(h.get_for(&Scope::global()).is_none());
    assert!(!h.is_unlocked());
}

#[test]
fn denied_scope_is_not_activity() {
    let (_t, u) = unlocked();
    let (clock, c) = FakeClock::new();
    let thinai = Scope::new("thinai", "prod");
    let lease = Lease::only(vec![thinai.clone()], clock.now() + time::Duration::days(1));
    let cfg = HolderConfig {
        ttl: HOUR * 8,
        idle: Some(Duration::from_secs(60)),
    };
    let h = KeyHolder::with_clock(u, cfg, lease, c);
    clock.advance(59);
    assert!(h.get_for(&Scope::new("kavi", "prod")).is_none());
    clock.advance(1);
    assert!(
        h.get_for(&thinai).is_none(),
        "denied request did not extend idle"
    );
}

#[test]
fn lock_wipes() {
    let (_t, u) = unlocked();
    let h = KeyHolder::new(
        u,
        HolderConfig::default(),
        Lease::all(OffsetDateTime::now_utc() + time::Duration::hours(1)),
    );
    assert!(h.status().unlocked);
    h.lock();
    assert!(!h.is_unlocked());
    assert!(h.get_for(&Scope::global()).is_none());
    assert_eq!(h.status().lease, None);
    let dbg = format!("{h:?}");
    assert!(!dbg.contains("sk-"), "{dbg}");
}

#[test]
fn holder_is_shareable_across_threads() {
    let (_t, u) = unlocked();
    let h = Arc::new(KeyHolder::new(
        u,
        HolderConfig::default(),
        Lease::all(OffsetDateTime::now_utc() + time::Duration::hours(1)),
    ));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let h = h.clone();
            std::thread::spawn(move || {
                let a = h.get_for(&Scope::new("kavi", "prod")).expect("access");
                assert_eq!(*a.get("OPENAI_API_KEY").expect("get"), "sk-kavi");
            })
        })
        .collect();
    for t in threads {
        t.join().expect("thread");
    }
}

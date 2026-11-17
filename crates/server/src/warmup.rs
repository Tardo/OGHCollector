// Copyright 2026 Alexandre D. Díaz
//! Cache warm-up: a thread recomputes the whole-page caches (Overview, modules, migration's
//! newest pair, committers, Doodba fleet) of every workspace at start and then a bit before
//! `cache_ttl` runs out, so a visitor never pays for a cold one. They take no request
//! parameters, or the common ones; the rest (other version pairs, the atlas per version)
//! stay lazy. Entries are replaced through the `*_prime_cache` functions `#[cached]`
//! generates: a plain call would return the stale entry. Off with `cache_warmup: false`.
use crate::config::SERVER_CONFIG;
use crate::routes::{committers, dashboard, projects};
use crate::workspace::{AreaPool, Workspaces};
use sqlitedb::Pool;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

/// Entries live `cache_ttl`: refresh at 90% of it so none expires first.
fn interval() -> Duration {
    Duration::from_secs((*SERVER_CONFIG.get_cache_ttl() * 9 / 10).max(60))
}

fn warm_scope(shared: &AreaPool, own: &AreaPool, all: &Pool) {
    // A query failing panics (`expect`): one scope must not stop the others or the thread.
    let step = |what: &str, f: &dyn Fn()| {
        if catch_unwind(AssertUnwindSafe(f)).is_err() {
            log::warn!("cache warm-up: {what} failed");
        }
    };
    step("overview", &|| {
        dashboard::warm(&mut own.get().unwrap(), &mut all.get().unwrap(), own.key())
    });
    step("committers", &|| {
        committers::warm(&mut shared.get().unwrap(), shared.key())
    });
    if SERVER_CONFIG.get_projects_enabled() {
        step("projects", &|| projects::warm(shared, all));
    }
}

pub fn spawn(workspaces: actix_web::web::Data<Workspaces>, all: Pool) {
    if !SERVER_CONFIG.get_cache_warmup() {
        log::info!("cache warm-up disabled");
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("cache-warmup".into())
        .spawn(move || loop {
            let started = Instant::now();
            let areas = workspaces.all_areas();
            for (shared, own) in &areas {
                warm_scope(shared, own, &all);
            }
            log::info!(
                "cache warm-up: {} workspace(s) in {:.1}s",
                areas.len(),
                started.elapsed().as_secs_f32()
            );
            std::thread::sleep(interval());
        });
    if let Err(e) = spawned {
        log::error!("cache warm-up thread: {e}");
    }
}

//! The process's one logger. The `log` crate takes a single logger per process, for good,
//! and on Android two things in one process want it: the app's setup (lib.rs) and a
//! background check that Android started the process for (background.rs). The check often
//! runs first, so the app found the slot taken, its setup failed, and Tauri aborted: the
//! "crashes two or three times, then opens" seen on phones. Both now go through here: the
//! slot holds a [`Switch`], and what it forwards to can be replaced.

use std::sync::{Arc, RwLock};

use log::{LevelFilter, Log, Metadata, Record};

static CURRENT: RwLock<Option<Arc<dyn Log>>> = RwLock::new(None);
static SWITCH: Switch = Switch;

struct Switch;

impl Switch {
    /// A clone, so no lock is held while a logger runs (one that logs can't deadlock).
    fn current(&self) -> Option<Arc<dyn Log>> {
        CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Log for Switch {
    fn enabled(&self, md: &Metadata<'_>) -> bool {
        self.current().is_some_and(|l| l.enabled(md))
    }

    fn log(&self, record: &Record<'_>) {
        if let Some(l) = self.current() {
            l.log(record);
        }
    }

    fn flush(&self) {
        if let Some(l) = self.current() {
            l.flush();
        }
    }
}

/// Claims the process's logger slot for [`Switch`]. Ok if it already has it; an error only
/// if something else took the slot first.
fn claim() -> Result<(), log::SetLoggerError> {
    match log::set_logger(&SWITCH) {
        Ok(()) => Ok(()),
        Err(_) if CURRENT.read().unwrap_or_else(|e| e.into_inner()).is_some() => Ok(()),
        Err(e) => Err(e),
    }
}

/// The app's logger: replaces whatever logged before (a background check's, on Android).
pub fn install(logger: Box<dyn Log>, max_level: LevelFilter) -> Result<(), log::SetLoggerError> {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::from(logger));
    claim()?;
    log::set_max_level(max_level);
    Ok(())
}

/// A fallback logger, used only if nothing logs yet: a background check in a process the
/// app may start in too, which then takes over with [`install`].
#[cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]
pub fn install_if_unset(logger: Box<dyn Log>, max_level: LevelFilter) {
    {
        let mut current = CURRENT.write().unwrap_or_else(|e| e.into_inner());
        if current.is_some() {
            return;
        }
        *current = Some(Arc::from(logger));
    }
    if claim().is_ok() {
        log::set_max_level(max_level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Keep(Arc<Mutex<Vec<String>>>);

    impl Log for Keep {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn log(&self, r: &Record<'_>) {
            self.0.lock().unwrap().push(r.args().to_string());
        }
        fn flush(&self) {}
    }

    fn keep() -> (Box<dyn Log>, Arc<Mutex<Vec<String>>>) {
        let lines = Arc::new(Mutex::new(Vec::new()));
        (Box::new(Keep(lines.clone())), lines)
    }

    /// One test: the logger slot is process-wide, so the steps share it in order.
    #[test]
    fn the_app_starts_after_a_background_check_took_the_logger_and_takes_it_over() {
        // A background check starts the process and logs first (background.rs).
        let (check, check_lines) = keep();
        install_if_unset(check, LevelFilter::Info);
        log::info!("background check: starting");

        // Then the app starts in the same process: this used to fail and abort it.
        let (app, app_lines) = keep();
        install(app, LevelFilter::Info).expect("the app gets the logger");
        log::info!("from the app");

        // A later check in the running app leaves the app's logger alone.
        let (late, late_lines) = keep();
        install_if_unset(late, LevelFilter::Info);
        log::info!("still the app's");

        // Other tests log in parallel, so look for these lines only.
        let ours = |lines: &Arc<Mutex<Vec<String>>>| -> Vec<String> {
            let mine = ["background check: starting", "from the app", "still the app's"];
            lines.lock().unwrap().iter().filter(|l| mine.contains(&l.as_str())).cloned().collect()
        };
        assert_eq!(ours(&check_lines), vec!["background check: starting"]);
        assert_eq!(ours(&app_lines), vec!["from the app", "still the app's"]);
        assert!(ours(&late_lines).is_empty());
    }
}

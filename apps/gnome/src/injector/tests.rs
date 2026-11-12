use super::*;
use std::sync::{Arc, Mutex};

type Log = Arc<Mutex<Vec<String>>>;

struct FakeShell {
    log: Log,
    down: bool,
    save_fails: bool,
}

#[async_trait]
impl Shell for FakeShell {
    async fn save_clipboard(&self) -> Result<u32, String> {
        if self.down || self.save_fails {
            return Err("no such service".into());
        }
        self.log.lock().unwrap().push("save".into());
        Ok(7)
    }
    async fn restore_clipboard(&self, token: u32) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("restore {token}"));
        Ok(())
    }
    async fn set_text(&self, text: &str) -> Result<(), String> {
        if self.down {
            return Err("no such service".into());
        }
        self.log.lock().unwrap().push(format!("set {text}"));
        Ok(())
    }
}

struct FakeKeys {
    log: Log,
    denied: bool,
    paste_fails: bool,
}

#[async_trait]
impl VirtualKeys for FakeKeys {
    async fn prepare(&self) -> Result<(), String> {
        if self.denied {
            Err("cannot open /dev/uinput".into())
        } else {
            Ok(())
        }
    }
    async fn ctrl_v(&self) -> Result<(), String> {
        self.log.lock().unwrap().push("ctrl-v".into());
        if self.paste_fails {
            Err("virtual keyboard write failed".into())
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Default)]
struct FakeReporter {
    statuses: Arc<Mutex<Vec<Option<Problem>>>>,
    notified: Arc<Mutex<Vec<Problem>>>,
}

impl ProblemReporter for FakeReporter {
    fn set_status(&self, problem: Option<Problem>) {
        self.statuses.lock().unwrap().push(problem);
    }
    fn notify(&self, problem: Problem) {
        self.notified.lock().unwrap().push(problem);
    }
}

type Fixture = (GnomeInjector<FakeShell, FakeKeys, FakeReporter>, Log, FakeReporter);

fn injector(down: bool, save_fails: bool, denied: bool, paste_fails: bool) -> Fixture {
    let log: Log = Default::default();
    let reporter = FakeReporter::default();
    let injector = GnomeInjector::new(
        FakeShell { log: log.clone(), down, save_fails },
        FakeKeys { log: log.clone(), denied, paste_fails },
        reporter.clone(),
    );
    (injector, log, reporter)
}

#[tokio::test]
async fn paste_sequence_matches_windows_order() {
    let (injector, log, reporter) = injector(false, false, false, false);
    injector.inject("hello", 0).await.unwrap();
    assert_eq!(*log.lock().unwrap(), ["save", "set hello", "ctrl-v", "restore 7"]);
    assert!(reporter.statuses.lock().unwrap().is_empty());
    assert!(reporter.notified.lock().unwrap().is_empty());
}

#[tokio::test]
async fn paste_without_shell_reports_missing_extension() {
    let (injector, _log, reporter) = injector(true, false, false, false);
    for _ in 0..2 {
        let err = injector.inject("hello", 0).await.unwrap_err();
        assert!(matches!(&err, InjectionError::ClipboardError(m) if m == MISSING_EXTENSION));
    }
    assert_eq!(*reporter.statuses.lock().unwrap(), [Some(Problem::MissingExtension); 2]);
    assert_eq!(*reporter.notified.lock().unwrap(), [Problem::MissingExtension]);
}

#[tokio::test]
async fn uinput_denied_is_reported() {
    let (injector, log, reporter) = injector(false, false, true, false);
    for _ in 0..2 {
        let err = injector.inject("hello", 0).await.unwrap_err();
        assert!(matches!(&err, InjectionError::KeyboardError(m) if m.contains("/dev/uinput")));
    }
    assert!(log.lock().unwrap().is_empty(), "text must not reach the clipboard");
    assert_eq!(*reporter.statuses.lock().unwrap(), [Some(Problem::UinputDenied); 2]);
    assert_eq!(*reporter.notified.lock().unwrap(), [Problem::UinputDenied]);
}

#[tokio::test]
async fn restore_runs_even_if_paste_fails() {
    let (injector, log, _reporter) = injector(false, false, false, true);
    assert!(injector.inject("hello", 0).await.is_err());
    assert_eq!(log.lock().unwrap().last().map(String::as_str), Some("restore 7"));
}

#[tokio::test]
async fn failed_save_still_pastes_and_skips_restore() {
    let (injector, log, reporter) = injector(false, true, false, false);
    injector.inject("hello", 0).await.unwrap();
    assert_eq!(*log.lock().unwrap(), ["set hello", "ctrl-v"]);
    assert!(reporter.statuses.lock().unwrap().is_empty());
}

#[tokio::test]
async fn success_after_failure_clears_status() {
    let log: Log = Default::default();
    let reporter = FakeReporter::default();
    let injector = GnomeInjector::new(
        FakeShell { log: log.clone(), down: false, save_fails: false },
        FakeKeys { log: log.clone(), denied: false, paste_fails: true },
        reporter.clone(),
    );
    assert!(injector.inject("a", 0).await.is_err());
    let injector = GnomeInjector {
        keys: FakeKeys { log: log.clone(), denied: false, paste_fails: false },
        ..injector
    };
    injector.inject("b", 0).await.unwrap();
    assert_eq!(*reporter.statuses.lock().unwrap(), [Some(Problem::UinputDenied), None]);
    injector.inject("c", 0).await.unwrap();
    assert_eq!(reporter.statuses.lock().unwrap().len(), 2);
}

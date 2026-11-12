use super::*;
use std::sync::{Arc, Mutex};

type Log = Arc<Mutex<Vec<String>>>;

struct FakeShell {
    log: Log,
    down: bool,
}

#[async_trait]
impl Shell for FakeShell {
    async fn save_clipboard(&self) -> Result<u32, String> {
        if self.down {
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
        self.log.lock().unwrap().push(format!("set {text}"));
        Ok(())
    }
}

struct FakeKeys {
    log: Log,
    fail: bool,
}

#[async_trait]
impl VirtualKeys for FakeKeys {
    async fn ctrl_v(&self) -> Result<(), String> {
        self.log.lock().unwrap().push("ctrl-v".into());
        if self.fail {
            Err("cannot open /dev/uinput".into())
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

fn injector(down: bool, fail: bool) -> (GnomeInjector<FakeShell, FakeKeys, FakeReporter>, Log, FakeReporter) {
    let log: Log = Default::default();
    let reporter = FakeReporter::default();
    let injector = GnomeInjector::new(
        FakeShell { log: log.clone(), down },
        FakeKeys { log: log.clone(), fail },
        reporter.clone(),
    );
    (injector, log, reporter)
}

#[tokio::test]
async fn paste_sequence_matches_windows_order() {
    let (injector, log, reporter) = injector(false, false);
    injector.inject("hello", 0).await.unwrap();
    assert_eq!(*log.lock().unwrap(), ["save", "set hello", "ctrl-v", "restore 7"]);
    assert_eq!(*reporter.statuses.lock().unwrap(), [None]);
    assert!(reporter.notified.lock().unwrap().is_empty());
}

#[tokio::test]
async fn paste_without_shell_reports_missing_extension() {
    let (injector, log, reporter) = injector(true, false);
    for _ in 0..2 {
        let err = injector.inject("hello", 0).await.unwrap_err();
        assert!(matches!(&err, InjectionError::ClipboardError(m) if m == MISSING_EXTENSION));
    }
    assert!(log.lock().unwrap().is_empty());
    assert_eq!(*reporter.statuses.lock().unwrap(), [Some(Problem::MissingExtension); 2]);
    assert_eq!(*reporter.notified.lock().unwrap(), [Problem::MissingExtension]);
}

#[tokio::test]
async fn uinput_denied_is_reported() {
    let (injector, _log, reporter) = injector(false, true);
    for _ in 0..2 {
        let err = injector.inject("hello", 0).await.unwrap_err();
        assert!(matches!(&err, InjectionError::KeyboardError(m) if m.contains("/dev/uinput")));
    }
    assert_eq!(*reporter.statuses.lock().unwrap(), [Some(Problem::UinputDenied); 2]);
    assert_eq!(*reporter.notified.lock().unwrap(), [Problem::UinputDenied]);
}

#[tokio::test]
async fn restore_runs_even_if_paste_fails() {
    let (injector, log, _reporter) = injector(false, true);
    assert!(injector.inject("hello", 0).await.is_err());
    assert_eq!(log.lock().unwrap().last().map(String::as_str), Some("restore 7"));
}

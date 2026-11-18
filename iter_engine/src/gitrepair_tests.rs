use super::*;
use std::sync::Mutex as StdMutex;

/// Records what the engine was asked to do.
#[derive(Default)]
struct Fake {
    repair_result: StdMutex<Option<Result<String, String>>>,
    prompts: StdMutex<Vec<String>>,
    requeued: StdMutex<Vec<(Vec<String>, String)>>,
    p0s: StdMutex<Vec<(String, String)>>,
    not_ready: bool,
}

impl Hooks for Fake {
    fn ready(&self) -> bool {
        !self.not_ready
    }
    fn repair(&self, prompt: String) -> JoinHandle<Result<String, String>> {
        self.prompts.lock().unwrap().push(prompt);
        let r = self.repair_result.lock().unwrap().clone().unwrap_or(Ok("REPAIRED: removed a stale lock".into()));
        std::thread::spawn(move || r)
    }
    fn requeue(&self, ids: &[String], note: &str) -> usize {
        self.requeued.lock().unwrap().push((ids.to_vec(), note.to_string()));
        ids.len()
    }
    fn file_p0(&self, name: &str, request: &str) -> Result<String, String> {
        let mut p = self.p0s.lock().unwrap();
        p.push((name.to_string(), request.to_string()));
        Ok(format!("p0-0000000000{}", p.len()))
    }
}

fn fail(class: Class) -> Outcome {
    Outcome::Git(Failure { class, command: "git pull -q --no-rebase --no-edit 'origin' 'main'".into(), output: "error: cannot lock ref 'refs/remotes/origin/main': Unable to create '/r/.git/refs/remotes/origin/main.lock': File exists.".into() })
}

/// Drive until the repair thread has finished and its outcome was handled.
fn drive_to_end(top: &str, h: &Fake, verify: &dyn Fn(&str) -> Result<(), (String, String)>) {
    for _ in 0..200 {
        drive_with(top, h, verify, DEFAULT_PROMPT);
        let all = repos();
        let phase_done = match all.get(&gitsafe::key(Path::new(top))).and_then(|r| r.incident.as_ref()) {
            None => true,
            Some(i) => matches!(i.phase, Phase::Stuck { .. }),
        };
        drop(all);
        if phase_done {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the incident never finished");
}

fn backdate_check(top: &str) {
    let mut all = repos();
    if let Some(Incident { phase: Phase::Stuck { checked, .. }, .. }) = all.get_mut(&gitsafe::key(Path::new(top))).and_then(|r| r.incident.as_mut()) {
        *checked = Instant::now() - RECHECK - Duration::from_secs(1);
    }
}

/// Three items in a row failing on one class pause the checkout; one
/// repair session runs (its prompt names the class, the command and its
/// output); the engine's own check passes; work is released and every item
/// that failed during the pause — the three, and one that was already
/// running and failed after — is queued again.
#[test]
fn three_same_class_failures_pause_then_repair_verify_release_and_requeue() {
    let top = "/nonexistent/gitrepair-test-release";
    let h = Fake::default();
    record(top, "w1", fail(Class::RefLock));
    record(top, "w2", fail(Class::RefLock));
    assert!(!paused(top), "two are not a streak");
    record(top, "w3", fail(Class::RefLock));
    assert!(paused(top), "three in a row pause it");
    let reason = hold_reason(top).unwrap();
    assert!(reason.contains("a git ref cannot be locked") && reason.contains("3 items"), "{reason}");
    record(top, "w4", fail(Class::RefLock)); // was running when the pause began

    let verified = StdMutex::new(0);
    let ok = |_: &str| -> Result<(), (String, String)> {
        *verified.lock().unwrap() += 1;
        Ok(())
    };
    drive_with(top, &h, &ok, DEFAULT_PROMPT);
    assert_eq!(repairs_started(top), 1);
    assert!(paused(top), "still paused while the repair runs");
    let prompt = h.prompts.lock().unwrap()[0].clone();
    assert!(prompt.contains("a git ref cannot be locked") && prompt.contains("git pull -q --no-rebase") && prompt.contains("main.lock': File exists"), "{prompt}");
    assert!(prompt.contains("without discarding anyone's uncommitted work"));
    drive_to_end(top, &h, &ok);

    assert_eq!(*verified.lock().unwrap(), 1, "the engine verified once itself");
    assert!(!paused(top), "released");
    let rq = h.requeued.lock().unwrap().clone();
    assert_eq!(rq.len(), 1);
    assert_eq!(rq[0].0, vec!["w1", "w2", "w3", "w4"]);
    assert!(h.p0s.lock().unwrap().is_empty());
    assert_eq!(repairs_started(top), 1, "one repair session in all");
}

/// A failure of another class, or any other outcome, breaks the streak.
#[test]
fn mixed_classes_or_a_success_between_do_not_pause() {
    let top = "/nonexistent/gitrepair-test-mixed";
    record(top, "a", fail(Class::Lock));
    record(top, "b", fail(Class::RefLock));
    record(top, "c", fail(Class::Lock));
    assert!(!paused(top));
    record(top, "d", fail(Class::Lock));
    record(top, "e", Outcome::Other);
    record(top, "f", fail(Class::Lock));
    record(top, "g", fail(Class::Lock));
    assert!(!paused(top), "a success reset the streak");
    record(top, "h", fail(Class::Lock));
    assert!(paused(top));
}

/// The repair does not fix it: the checkout stays paused, exactly one P0 is
/// filed with the failing command and output, and however many ticks follow
/// no second repair session starts.  The periodic re-check picks up a fix
/// made from outside and releases; an incident that comes back within the
/// hour gets no repair session at all.
#[test]
fn a_failed_repair_stays_paused_with_one_p0_and_no_second_repair() {
    let top = "/nonexistent/gitrepair-test-stuck";
    let h = Fake::default();
    *h.repair_result.lock().unwrap() = Some(Ok("NOT REPAIRED: the index is corrupt and a merge is half done".into()));
    for w in ["s1", "s2", "s3"] {
        record(top, w, fail(Class::Index));
    }
    let broken = |_: &str| -> Result<(), (String, String)> { Err(("git status --porcelain".into(), "fatal: index file corrupt".into())) };
    drive_to_end(top, &h, &broken);
    assert!(paused(top), "still paused");
    for _ in 0..50 {
        drive_with(top, &h, &broken, DEFAULT_PROMPT);
    }
    backdate_check(top);
    drive_with(top, &h, &broken, DEFAULT_PROMPT); // the re-check fails too
    assert_eq!(repairs_started(top), 1, "never a second repair session");
    let p0s = h.p0s.lock().unwrap().clone();
    assert_eq!(p0s.len(), 1, "exactly one P0");
    assert!(p0s[0].1.contains("git status --porcelain") && p0s[0].1.contains("fatal: index file corrupt"), "{}", p0s[0].1);
    assert!(p0s[0].1.contains("NOT REPAIRED"));
    assert!(hold_reason(top).unwrap().contains("P0"), "{:?}", hold_reason(top));
    assert!(h.requeued.lock().unwrap().is_empty());

    // a person fixed it: the next re-check releases and requeues
    backdate_check(top);
    let ok = |_: &str| -> Result<(), (String, String)> { Ok(()) };
    drive_with(top, &h, &ok, DEFAULT_PROMPT);
    assert!(!paused(top));
    assert_eq!(h.requeued.lock().unwrap()[0].0, vec!["s1", "s2", "s3"]);

    // it comes back at once: no repair session, straight to the check and a P0
    for w in ["t1", "t2", "t3"] {
        record(top, w, fail(Class::Index));
    }
    assert!(paused(top));
    drive_to_end(top, &h, &broken);
    assert_eq!(repairs_started(top), 1, "no repair session within the hour of the last release");
    assert_eq!(h.p0s.lock().unwrap().len(), 2, "one P0 for the new incident");
    assert!(h.p0s.lock().unwrap()[1].1.contains("No repair session ran"));
}

/// No usable account: the repair waits (paused) instead of failing.
#[test]
fn the_repair_waits_for_a_usable_account() {
    let top = "/nonexistent/gitrepair-test-account";
    let h = Fake { not_ready: true, ..Default::default() };
    for w in ["u1", "u2", "u3"] {
        record(top, w, fail(Class::Lock));
    }
    let ok = |_: &str| -> Result<(), (String, String)> { Ok(()) };
    drive_with(top, &h, &ok, DEFAULT_PROMPT);
    assert_eq!(repairs_started(top), 0);
    assert!(paused(top));
}

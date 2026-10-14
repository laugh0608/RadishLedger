use serde_json::{Value, json};
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn spawn(request: &Value) -> Worker {
    let mut child = Command::new(env!("CARGO_BIN_EXE_m0-recovery-worker"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    Worker(child)
}
fn response(request: &Value) -> Value {
    use std::io::Read;
    let mut child = spawn(request);
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "worker timeout");
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut data = String::new();
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut data)
        .unwrap();
    let result: Value = serde_json::from_str(&data).unwrap();
    assert_eq!(result["ok"], true, "{result}");
    result
}
#[test]
fn killed_process_recovers_atomic_facts_and_replays_exactly_once() {
    let temp = tempfile::tempdir().unwrap();
    for point in ["beforeOutbox", "beforeCommit", "afterCommit"] {
        let run = radishledger_core::random_id().unwrap();
        let request = |kind: &str| json!({"abi":1,"directory":temp.path(),"action":{"kind":kind,"run":run,"point":point}});
        let mut child = spawn(&request("recoveryStart"));
        let ready = temp
            .path()
            .join(format!("radishledger-recovery-{run}/ready.json"));
        let deadline = Instant::now() + Duration::from_secs(15);
        let marker: Value = loop {
            if let Ok(bytes) = std::fs::read(&ready)
                && let Ok(value) = serde_json::from_slice(&bytes)
            {
                break value;
            }
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "worker exited before checkpoint"
            );
            assert!(Instant::now() < deadline, "checkpoint timeout");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(marker["pid"], child.0.id());
        assert_eq!(marker["point"], point);
        child.0.kill().unwrap();
        let status = child.0.wait().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(9));
        }
        let result = response(&request("recoveryInspect"));
        assert_ne!(result["report"]["pid"], marker["pid"]);
        assert_eq!(result["report"]["alreadyCommitted"], point == "afterCommit");
        assert_eq!(
            result["report"]["before"]["sequence"],
            if point == "afterCommit" { 3 } else { 2 }
        );
        assert_eq!(result["report"]["after"]["sequence"], 3);
        response(
            &json!({"abi":1,"directory":temp.path(),"action":{"kind":"recoveryCleanup","run":run}}),
        );
    }
    assert_eq!(temp.path().read_dir().unwrap().count(), 0);
}

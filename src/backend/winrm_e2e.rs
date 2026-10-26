//! End-to-end test of the WinRM client against `tools/winrm_mock.py` (a mock server, not a
//! real Windows host). Ignored by default:
//!
//! ```text
//! python3 tools/winrm_mock.py --port 5985 &
//! AURORA_WINRM_MOCK=127.0.0.1:5985:admin:S3cret!pw:http cargo test winrm_mock -- --ignored
//! ```
//! For HTTPS start the mock with `--tls cert.pem key.pem` and use `:https`; for Basic
//! add `--basic` and `:basic` after the transport.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::winrm_client::{RunError, Session, open_trusted};
use super::winrm_collect;
use super::winrm_http::{ConnectError, Target};
use super::winrm_types::WinrmAuth;

#[test]
#[ignore = "needs the mock server (tools/winrm_mock.py)"]
fn winrm_mock_end_to_end() {
    let spec = std::env::var("AURORA_WINRM_MOCK").expect("AURORA_WINRM_MOCK");
    let p: Vec<&str> = spec.split(':').collect();
    let mut target = Target {
        host: p[0].into(),
        port: p[1].parse().unwrap(),
        https: p[4] == "https",
        auth: if p.get(5) == Some(&"basic") {
            WinrmAuth::Basic
        } else {
            WinrmAuth::Ntlm
        },
        user: p[2].into(),
        domain: String::new(),
        password: p[3].into(),
        pin: None,
    };
    let cancel = AtomicBool::new(false);
    if target.https {
        // Without a pin the connection must be refused with the fingerprint.
        let err = open_trusted(&target, None)
            .err()
            .expect("untrusted")
            .to_string();
        assert!(err.contains(':'), "{err}");
        let fp = Session::open(&target).err().map(|e| match e {
            ConnectError::Untrusted(fp) => fp,
            other => panic!("{other}"),
        });
        target.pin = fp;
        // A wrong pin is a different error.
        let mut bad = target.clone();
        bad.pin = Some("00:11".into());
        assert!(matches!(
            Session::open(&bad).err(),
            Some(ConnectError::PinChanged { .. })
        ));
    }
    // A wrong password is refused.
    let mut wrong = target.clone();
    wrong.password = "nope".into();
    let e = Session::open(&wrong).err().expect("must fail").to_string();
    assert!(e.contains("401"), "{e}");

    let (mut s, _) = open_trusted(&target, None).expect("open");
    let out = s
        .run_script(
            "Write-Output 'hi there'",
            Duration::from_secs(20),
            1 << 20,
            &cancel,
        )
        .unwrap();
    assert_eq!(out.exit, 0);
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(text.contains("hi there") && text.contains("ünï"), "{text}");
    let out = s
        .run_script("exit 3", Duration::from_secs(20), 1 << 20, &cancel)
        .unwrap();
    assert_eq!(out.exit, 3);
    let out = s
        .run_script("throw 'x'", Duration::from_secs(20), 1 << 20, &cancel)
        .unwrap();
    assert_eq!(out.exit, 1);
    assert_eq!(out.stderr.trim(), "mock failure");
    // Output cap.
    let out = s
        .run_script("Write-Output 'abc'", Duration::from_secs(20), 5, &cancel)
        .unwrap();
    assert!(out.truncated && out.stdout.len() == 5);
    // Timeout of a slow command (the mock answers "no output yet" until it is ready).
    let r = s.run_script("Start-Sleep 30", Duration::from_secs(3), 1 << 20, &cancel);
    assert!(matches!(r, Err(RunError::Timeout)), "{r:?}");
    // Collector.
    let got = winrm_collect::collect(&mut s, &[], true, &cancel).expect("collect");
    assert_eq!(got.facts.os_id, "windows");
    assert_eq!(got.facts.hostname, "MOCK-WIN01");
    assert_eq!(got.facts.updates, Some(4));
    assert_eq!(got.detail.disks.len(), 2);
    assert!(
        got.detail
            .services
            .iter()
            .any(|x| x.name == "MyApp" && x.failed)
    );
    assert_eq!(got.detail.ports.len(), 5);
    assert_eq!(got.detail.firewall.rules[0].name, "Block telnet");
    s.close();
}

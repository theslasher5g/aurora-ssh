//! Container overview: runs the command lines of `containers_parse` on this PC or over an
//! SSH exec and delivers parsed results as events.

use std::time::Duration;

use russh::ChannelMsg;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

use super::container::{Target, explain};
use super::containers_parse as p;
use super::ssh::{SshHandle, connect};
use super::{HostKeyPrompt, Sink};

/// `None` = this PC, otherwise the SSH host.
pub type HostKey = Option<Uuid>;

/// Output cap of one remote command (a big cluster prints megabytes of JSON).
const CAP: usize = 6 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(40);
const RC_MARK: &str = "@@RC:";

/// Output and exit code of one command.
#[derive(Clone, Debug, Default)]
pub struct Seg {
    pub rc: i32,
    pub out: String,
}

/// What a refresh round asks for.
#[derive(Clone, Debug, Default)]
pub struct RoundSpec {
    /// `docker` and/or `podman`.
    pub runtimes: Vec<String>,
    pub kube: Option<KubeSpec>,
}

#[derive(Clone, Debug, Default)]
pub struct KubeSpec {
    /// Empty = the current context.
    pub context: String,
    /// Empty = all namespaces.
    pub namespace: String,
}

#[derive(Clone, Debug, Default)]
pub struct DockerRound {
    pub runtime: String,
    pub containers: Vec<p::DockerContainer>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct KubeRound {
    pub current: String,
    pub contexts: Vec<String>,
    pub pods: Vec<p::Pod>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct RoundData {
    pub docker: Vec<DockerRound>,
    pub kube: Option<KubeRound>,
}

pub enum CtrEvent {
    /// Which of docker/podman/kubectl exist.
    Probe(HostKey, Result<Vec<String>, String>),
    Round(HostKey, Result<RoundData, String>),
    /// An action finished: (host, label for the toast, container/pod it was about, result).
    Op(HostKey, String, String, Result<String, String>),
    /// Output for a sheet: `true` = inspect (JSON), `false` = describe (text).
    Text(bool, String, Result<String, String>),
}

pub type CtrRx = UnboundedReceiver<CtrEvent>;

fn join_line(argvs: &[Vec<String>]) -> String {
    let parts: Vec<String> = argvs
        .iter()
        .map(|a| {
            format!(
                "{} 2>&1; echo \"{RC_MARK}$?\"",
                super::container::shell_line(a)
            )
        })
        .collect();
    let inner = parts.join("; ");
    // sh -c '…' works whatever the login shell is.
    format!("sh -c {}", super::container::sh_quote(&inner))
}

/// Splits the combined output of [`join_line`] back into one [`Seg`] per command.
pub fn split_segments(out: &str, n: usize) -> Vec<Seg> {
    let mut segs = Vec::new();
    let mut cur = String::new();
    for line in out.lines() {
        if let Some(rc) = line.trim().strip_prefix(RC_MARK) {
            segs.push(Seg {
                rc: rc.trim().parse().unwrap_or(1),
                out: std::mem::take(&mut cur),
            });
        } else {
            cur.push_str(line);
            cur.push('\n');
        }
    }
    // Output cut off (cap or timeout): the unfinished command counts as failed.
    if segs.len() < n {
        segs.push(Seg { rc: 1, out: cur });
    }
    while segs.len() < n {
        segs.push(Seg {
            rc: 1,
            out: String::new(),
        });
    }
    segs
}

async fn exec_capped(handle: &SshHandle, cmd: &str) -> anyhow::Result<String> {
    let run = async {
        let mut ch = handle.channel_open_session().await?;
        ch.exec(true, cmd).await?;
        let mut out = Vec::new();
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    if out.len() < CAP {
                        out.extend_from_slice(&data);
                    }
                }
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        anyhow::Ok(String::from_utf8_lossy(&out).into_owned())
    };
    tokio::time::timeout(TIMEOUT, run)
        .await
        .map_err(|_| anyhow::anyhow!(t!("Zeitüberschreitung")))?
}

/// On Windows a tool that is not installed natively may live in WSL (Docker started inside
/// the default distribution): `wsl.exe -e <tool> …` runs it there.
fn wsl_exe() -> Option<std::path::PathBuf> {
    if cfg!(windows) {
        super::local::find_program("wsl.exe")
    } else {
        None
    }
}

/// Tools missing on this PC but found in WSL.
async fn wsl_has(tool: &str) -> bool {
    let Some(wsl) = wsl_exe() else { return false };
    let mut cmd = tokio::process::Command::new(wsl);
    cmd.args(["-e", "sh", "-c", "command -v \"$1\"", "sh", tool])
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    matches!(
        tokio::time::timeout(Duration::from_secs(15), cmd.status()).await,
        Ok(Ok(s)) if s.success()
    )
}

async fn run_local(argv: &[String]) -> Seg {
    let first = run_local_once(argv).await;
    // Added to the "docker" group a moment ago: the running Aurora (and its desktop session)
    // does not have the group yet. `sg docker` starts the command with it, no new login needed.
    if cfg!(unix)
        && argv[0] == "docker"
        && first.rc != 0
        && first.out.to_lowercase().contains("permission denied")
        && let Some(sg) = super::local::find_program("sg")
    {
        let line = argv
            .iter()
            .map(|a| super::container::sh_quote(a))
            .collect::<Vec<_>>()
            .join(" ");
        let mut cmd = tokio::process::Command::new(sg);
        cmd.args(["docker", "-c", &line])
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null());
        if let Ok(Ok(o)) = tokio::time::timeout(TIMEOUT, cmd.output()).await
            && o.status.success()
        {
            let mut out = String::from_utf8_lossy(&o.stdout).into_owned();
            out.push_str(&String::from_utf8_lossy(&o.stderr));
            out.truncate(floor_boundary(&out, CAP));
            return Seg { rc: 0, out };
        }
    }
    first
}

async fn run_local_once(argv: &[String]) -> Seg {
    let mut args: Vec<String> = argv[1..].to_vec();
    let exe = match super::local::find_program(&argv[0]) {
        Some(e) => e,
        None => match wsl_exe() {
            Some(wsl) if wsl_has(&argv[0]).await => {
                args.splice(0..0, ["-e".to_string(), argv[0].clone()]);
                wsl
            }
            _ => {
                return Seg {
                    rc: 127,
                    out: format!("{}: not found", argv[0]),
                };
            }
        },
    };
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(&args).kill_on_drop(true);
    cmd.stdin(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    match tokio::time::timeout(TIMEOUT, cmd.output()).await {
        Ok(Ok(o)) => {
            let mut out = String::from_utf8_lossy(&o.stdout).into_owned();
            out.push_str(&String::from_utf8_lossy(&o.stderr));
            out.truncate(floor_boundary(&out, CAP));
            Seg {
                rc: o.status.code().unwrap_or(1),
                out,
            }
        }
        Ok(Err(e)) => Seg {
            rc: 126,
            out: e.to_string(),
        },
        Err(_) => Seg {
            rc: 124,
            out: t!("Zeitüberschreitung").into(),
        },
    }
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Runs the commands (one SSH connection, one exec) and returns one [`Seg`] each.
pub async fn run(
    target: &Target,
    argvs: &[Vec<String>],
    prompts: Sink<HostKeyPrompt>,
) -> Result<Vec<Seg>, String> {
    if argvs.is_empty() || argvs.iter().any(|a| a.is_empty()) {
        return Err("empty command".into());
    }
    match target {
        Target::Local => {
            let mut out = Vec::new();
            for a in argvs {
                out.push(run_local(a).await);
            }
            Ok(out)
        }
        Target::Ssh(params) => {
            let handle = connect(params, prompts)
                .await
                .map_err(|e| format!("{e:#}"))?;
            let r = exec_capped(&handle, &join_line(argvs)).await;
            let _ = handle
                .disconnect(russh::Disconnect::ByApplication, "bye", "en")
                .await;
            let out = r.map_err(|e| format!("{e:#}"))?;
            Ok(split_segments(&out, argvs.len()))
        }
    }
}

fn first_error(cli: &str, seg: &Seg) -> String {
    explain(cli, seg.out.trim())
}

/// Builds the commands of a round and parses the answer.
pub async fn round(
    target: &Target,
    spec: &RoundSpec,
    prompts: Sink<HostKeyPrompt>,
) -> Result<RoundData, String> {
    // (runtime, index of ps, index of stats) / kube segment indices
    let mut argvs: Vec<Vec<String>> = Vec::new();
    let mut rts = Vec::new();
    for rt in spec.runtimes.iter().filter(|r| p::valid_runtime(r)) {
        let base = argvs.len();
        argvs.push(vec![
            rt.clone(),
            "ps".into(),
            "-a".into(),
            "--format".into(),
            "{{json .}}".into(),
        ]);
        argvs.push(vec![
            rt.clone(),
            "stats".into(),
            "--no-stream".into(),
            "--format".into(),
            "{{json .}}".into(),
        ]);
        rts.push((rt.clone(), base));
    }
    let mut kube_at = None;
    if let Some(k) = &spec.kube {
        let pods =
            p::pods_argv(&k.context, &k.namespace).ok_or_else(|| "invalid context".to_string())?;
        let mut cur = p::kube_base("").unwrap_or_default();
        cur.extend(["config".into(), "current-context".into()]);
        let mut ctxs = p::kube_base("").unwrap_or_default();
        ctxs.extend([
            "config".into(),
            "get-contexts".into(),
            "-o".into(),
            "name".into(),
        ]);
        kube_at = Some(argvs.len());
        argvs.extend([cur, ctxs, pods]);
    }
    let segs = run(target, &argvs, prompts).await?;
    let mut data = RoundData::default();
    for (rt, base) in rts {
        let ps = &segs[base];
        let mut r = DockerRound {
            runtime: rt.clone(),
            ..Default::default()
        };
        if ps.rc != 0 {
            r.error = Some(first_error(&rt, ps));
        } else {
            let mut parsed = p::parse_ps(&ps.out);
            let st = &segs[base + 1];
            if st.rc == 0 {
                p::apply_stats(&mut parsed.items, &p::parse_stats(&st.out));
            }
            if parsed.items.is_empty() && !parsed.notes.is_empty() {
                r.error = Some(explain(&rt, &parsed.notes.join("\n")));
            }
            r.containers = parsed.items;
        }
        data.docker.push(r);
    }
    if let Some(at) = kube_at {
        let mut k = KubeRound {
            current: segs[at]
                .out
                .trim()
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string(),
            contexts: p::parse_names(&segs[at + 1].out),
            ..Default::default()
        };
        if !p::valid_ref(&k.current) {
            k.current.clear();
        }
        let pods = &segs[at + 2];
        if pods.rc != 0 {
            k.error = Some(first_error("kubectl", pods));
        } else {
            match p::parse_pods_json(&pods.out) {
                Ok(v) => k.pods = v,
                Err(e) => k.error = Some(format!("kubectl: {e}")),
            }
        }
        data.kube = Some(k);
    }
    Ok(data)
}

pub fn spawn_probe(
    rt: &tokio::runtime::Runtime,
    key: HostKey,
    target: Target,
    sink: Sink<CtrEvent>,
    prompts: Sink<HostKeyPrompt>,
) {
    rt.spawn(async move {
        let res = match &target {
            Target::Local => {
                let mut found = Vec::new();
                for c in ["docker", "podman", "kubectl"] {
                    if super::local::find_program(c).is_some() || wsl_has(c).await {
                        found.push(c.to_string());
                    }
                }
                Ok(found)
            }
            Target::Ssh(_) => run(&target, &[p::probe_argv()], prompts)
                .await
                .map(|s| p::parse_names(&s[0].out))
                .map(|v| {
                    v.into_iter()
                        .filter(|c| matches!(c.as_str(), "docker" | "podman" | "kubectl"))
                        .collect()
                }),
        };
        sink.send(CtrEvent::Probe(key, res));
    });
}

pub fn spawn_round(
    rt: &tokio::runtime::Runtime,
    key: HostKey,
    target: Target,
    spec: RoundSpec,
    sink: Sink<CtrEvent>,
    prompts: Sink<HostKeyPrompt>,
) {
    rt.spawn(async move {
        let res = round(&target, &spec, prompts).await;
        sink.send(CtrEvent::Round(key, res));
    });
}

/// One action (start, stop, delete pod …). `cli` names the program for error hints.
#[allow(clippy::too_many_arguments)]
pub fn spawn_op(
    rt: &tokio::runtime::Runtime,
    key: HostKey,
    target: Target,
    label: String,
    subject: String,
    argv: Vec<String>,
    sink: Sink<CtrEvent>,
    prompts: Sink<HostKeyPrompt>,
) {
    rt.spawn(async move {
        let cli = argv.first().cloned().unwrap_or_default();
        let res = match run(&target, &[argv], prompts).await {
            Ok(s) if s[0].rc == 0 => Ok(s[0].out.trim().chars().take(400).collect()),
            Ok(s) => Err(first_error(&cli, &s[0])),
            Err(e) => Err(e),
        };
        sink.send(CtrEvent::Op(key, label, subject, res));
    });
}

/// Output for a sheet (`docker inspect`, `kubectl describe`), capped at 256 KiB.
pub fn spawn_text(
    rt: &tokio::runtime::Runtime,
    target: Target,
    inspect: bool,
    title: String,
    argv: Vec<String>,
    sink: Sink<CtrEvent>,
    prompts: Sink<HostKeyPrompt>,
) {
    rt.spawn(async move {
        let cli = argv.first().cloned().unwrap_or_default();
        let res = match run(&target, &[argv], prompts).await {
            Ok(s) if s[0].rc == 0 => {
                let mut o = s[0].out.clone();
                o.truncate(floor_boundary(&o, 256 * 1024));
                Ok(o)
            }
            Ok(s) => Err(first_error(&cli, &s[0])),
            Err(e) => Err(e),
        };
        sink.send(CtrEvent::Text(inspect, title, res));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments() {
        let out = "line1\n@@RC:0\nerr text\n@@RC:1\n";
        let s = split_segments(out, 2);
        assert_eq!(s[0].rc, 0);
        assert_eq!(s[0].out, "line1\n");
        assert_eq!(s[1].rc, 1);
        // cut off
        let s = split_segments("a\n@@RC:0\npartial", 3);
        assert_eq!(s.len(), 3);
        assert_eq!(s[1].rc, 1);
        assert_eq!(s[1].out, "partial\n");
        assert_eq!(s[2].rc, 1);
    }

    #[test]
    fn line_is_one_quoted_word() {
        let l = join_line(&[vec![
            "docker".into(),
            "ps".into(),
            "--format".into(),
            "{{json .}}".into(),
        ]]);
        assert!(l.starts_with("sh -c '"));
        let words = super::super::container::split_args(&l);
        assert_eq!(words.len(), 3);
        assert!(words[2].contains("'{{json .}}'"));
        assert!(words[2].ends_with("echo \"@@RC:$?\""));
    }
}

//! Docker/Podman containers and Kubernetes pods: building the CLI commands for shells
//! and logs, and listing what runs on this PC or on an SSH host.

use std::time::Duration;

use anyhow::{Result, anyhow};

use super::Sink;
use super::ssh::{SshParams, connect, exec};
use crate::model::{ContainerSettings, Protocol};

/// Quotes one word for a POSIX shell.
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@,+%".contains(c))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r#"'\''"#))
}

pub fn shell_line(argv: &[String]) -> String {
    argv.iter()
        .map(|a| sh_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_argv(c: &ContainerSettings) -> Vec<String> {
    if c.shell.trim().is_empty() {
        // bash if the image has it, else sh.
        vec![
            "sh".into(),
            "-c".into(),
            "if command -v bash >/dev/null 2>&1; then exec bash; else exec sh; fi".into(),
        ]
    } else {
        vec![c.shell.trim().to_string()]
    }
}

fn kubectl(c: &ContainerSettings) -> Vec<String> {
    let mut v = vec!["kubectl".to_string()];
    if !c.context.trim().is_empty() {
        v.extend(["--context".into(), c.context.trim().into()]);
    }
    if !c.namespace.trim().is_empty() {
        v.extend(["-n".into(), c.namespace.trim().into()]);
    }
    v
}

fn runtime(c: &ContainerSettings) -> String {
    if c.podman { "podman" } else { "docker" }.into()
}

/// Interactive shell inside the container/pod.
pub fn exec_argv(proto: Protocol, c: &ContainerSettings) -> Vec<String> {
    let name = c.name.trim().to_string();
    let mut v;
    if proto == Protocol::Kubernetes {
        v = kubectl(c);
        v.extend(["exec".into(), "-it".into(), name]);
        if !c.container.trim().is_empty() {
            v.extend(["-c".into(), c.container.trim().into()]);
        }
        v.push("--".into());
    } else {
        v = vec![runtime(c), "exec".into(), "-it".into()];
        if !c.user.trim().is_empty() {
            v.extend(["-u".into(), c.user.trim().into()]);
        }
        v.push(name);
    }
    v.extend(shell_argv(c));
    v
}

/// Follow the container's/pod's log output.
pub fn logs_argv(proto: Protocol, c: &ContainerSettings) -> Vec<String> {
    let name = c.name.trim().to_string();
    if proto == Protocol::Kubernetes {
        let mut v = kubectl(c);
        v.extend(["logs".into(), "-f".into(), "--tail=500".into(), name]);
        if !c.container.trim().is_empty() {
            v.extend(["-c".into(), c.container.trim().into()]);
        }
        v
    } else {
        vec![
            runtime(c),
            "logs".into(),
            "-f".into(),
            "--tail".into(),
            "500".into(),
            name,
        ]
    }
}

/// One entry of a container/pod listing.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub name: String,
    /// Image (Docker) or the pod's containers (Kubernetes).
    pub detail: String,
    pub status: String,
    pub running: bool,
    pub namespace: String,
    pub containers: Vec<String>,
}

const DOCKER_LIST: &str = "ps -a --format '{{.Names}}\t{{.Image}}\t{{.Status}}\t{{.State}}'";
const K8S_LIST: &str = "get pods -A --no-headers -o 'custom-columns=NS:.metadata.namespace,NAME:.metadata.name,PHASE:.status.phase,CONTAINERS:.spec.containers[*].name'";

pub fn parse_docker(out: &str) -> Vec<Item> {
    out.lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let name = f.next()?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let detail = f.next().unwrap_or_default().trim().to_string();
            let status = f.next().unwrap_or_default().trim().to_string();
            let state = f.next().unwrap_or_default().trim().to_lowercase();
            let running = if state.is_empty() {
                status.starts_with("Up")
            } else {
                state == "running"
            };
            Some(Item {
                name,
                detail,
                status,
                running,
                namespace: String::new(),
                containers: Vec::new(),
            })
        })
        .collect()
}

pub fn parse_pods(out: &str) -> Vec<Item> {
    out.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let namespace = f.next()?.to_string();
            let name = f.next()?.to_string();
            let status = f.next().unwrap_or_default().to_string();
            let containers: Vec<String> = f
                .next()
                .unwrap_or_default()
                .split(',')
                .filter(|c| !c.is_empty() && *c != "<none>")
                .map(str::to_string)
                .collect();
            Some(Item {
                running: status == "Running",
                detail: containers.join(", "),
                name,
                status,
                namespace,
                containers,
            })
        })
        .collect()
}

/// Where the CLI runs.
#[derive(Clone)]
pub enum Target {
    Local,
    Ssh(SshParams),
}

pub enum ListEvent {
    Done(Result<Vec<Item>, String>),
}

/// Lists containers or pods; the answer arrives as one `ListEvent`.
pub fn spawn_list(
    rt: &tokio::runtime::Runtime,
    proto: Protocol,
    c: ContainerSettings,
    target: Target,
    sink: Sink<ListEvent>,
    prompts: Sink<super::HostKeyPrompt>,
) {
    rt.spawn(async move {
        let r = list(proto, &c, target, prompts)
            .await
            .map_err(|e| format!("{e:#}"));
        sink.send(ListEvent::Done(r));
    });
}

async fn list(
    proto: Protocol,
    c: &ContainerSettings,
    target: Target,
    prompts: Sink<super::HostKeyPrompt>,
) -> Result<Vec<Item>> {
    let (cli, args) = if proto == Protocol::Kubernetes {
        let mut k = kubectl(&ContainerSettings {
            namespace: String::new(),
            ..c.clone()
        });
        let cli = k.remove(0);
        (cli, format!("{} {K8S_LIST}", shell_line(&k)))
    } else {
        (runtime(c), DOCKER_LIST.to_string())
    };
    let (status, out, err) = match target {
        Target::Ssh(p) => {
            let handle = connect(&p, prompts).await?;
            // 2>&1 so the reason (permission denied, not installed) reaches the user.
            let (status, out) = exec(&handle, &format!("{cli} {args} 2>&1")).await?;
            let _ = handle
                .disconnect(russh::Disconnect::ByApplication, "bye", "en")
                .await;
            (status, out, String::new())
        }
        Target::Local => {
            let line = format!("{cli} {args}");
            let argv = split_args(&line);
            let exe = super::local::find_program(&argv[0]).ok_or_else(|| {
                anyhow!(
                    "{}",
                    t!(
                        "{} wurde nicht gefunden. Ist es installiert und im PATH?",
                        cli
                    )
                )
            })?;
            let mut cmd = tokio::process::Command::new(exe);
            cmd.args(&argv[1..]).kill_on_drop(true);
            #[cfg(windows)]
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            let o = tokio::time::timeout(Duration::from_secs(25), cmd.output())
                .await
                .map_err(|_| anyhow!(t!("Zeitüberschreitung")))?
                .map_err(|e| {
                    anyhow!(
                        "{}",
                        t!("{} konnte nicht gestartet werden ({}). Ist es installiert und im PATH?", cli, e)
                    )
                })?;
            (
                o.status.code().unwrap_or(1) as u32,
                String::from_utf8_lossy(&o.stdout).into_owned(),
                String::from_utf8_lossy(&o.stderr).into_owned(),
            )
        }
    };
    if status != 0 {
        let msg = format!("{out}{err}");
        return Err(anyhow!("{}", explain(&cli, msg.trim())));
    }
    Ok(if proto == Protocol::Kubernetes {
        parse_pods(&out)
    } else {
        parse_docker(&out)
    })
}

/// Turns typical CLI failures into a hint what to do.
pub(crate) fn explain(cli: &str, msg: &str) -> String {
    let low = msg.to_lowercase();
    let hint = if low.contains("not found") || low.contains("nicht gefunden") {
        t!("{} ist dort nicht installiert.", cli)
    } else if low.contains("permission denied") && low.contains("docker.sock") {
        t!("Keine Berechtigung für Docker – den Benutzer in die Gruppe „docker“ aufnehmen (sudo usermod -aG docker $USER) und danach Aurora neu starten (bzw. ab- und wieder anmelden).")
            .to_string()
    } else if low.contains("cannot connect to the docker daemon")
        || low.contains("failed to connect to the docker api")
    {
        t!("Der Docker-Dienst läuft nicht.").to_string()
    } else if low.contains("current-context") || low.contains("kubeconfig") {
        t!("kubectl hat keinen Cluster konfiguriert (kubeconfig).").to_string()
    } else {
        String::new()
    };
    let first: String = msg.lines().take(3).collect::<Vec<_>>().join(" · ");
    let first: String = first.chars().take(300).collect();
    if hint.is_empty() {
        first
    } else if first.is_empty() {
        hint
    } else {
        format!("{hint}\n{first}")
    }
}

/// Splits a command line built by `shell_line` back into words (single quotes only).
pub fn split_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut has = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                quoted = !quoted;
                has = true;
            }
            '\\' if !quoted => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !quoted => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        let c = ContainerSettings {
            name: "web".into(),
            user: "root".into(),
            ..Default::default()
        };
        let a = exec_argv(Protocol::Docker, &c);
        assert_eq!(a[..5], ["docker", "exec", "-it", "-u", "root"]);
        assert_eq!(a[5], "web");
        assert!(shell_line(&a).contains("'if command -v bash"));
        let k = ContainerSettings {
            name: "api-7f".into(),
            namespace: "prod".into(),
            container: "app".into(),
            shell: "/bin/sh".into(),
            ..Default::default()
        };
        assert_eq!(
            shell_line(&exec_argv(Protocol::Kubernetes, &k)),
            "kubectl -n prod exec -it api-7f -c app -- /bin/sh"
        );
        assert_eq!(
            shell_line(&logs_argv(Protocol::Kubernetes, &k)),
            "kubectl -n prod logs -f --tail=500 api-7f -c app"
        );
        assert_eq!(sh_quote("it's"), r#"'it'\''s'"#);
        assert_eq!(
            split_args(&format!("docker {DOCKER_LIST}")),
            vec![
                "docker",
                "ps",
                "-a",
                "--format",
                "{{.Names}}\t{{.Image}}\t{{.Status}}\t{{.State}}"
            ]
        );
    }

    #[test]
    fn parsing() {
        let d = parse_docker(
            "web\tnginx:1.27\tUp 3 hours\trunning\ndb\tpostgres\tExited (0) 2 days ago\texited\n",
        );
        assert_eq!(d.len(), 2);
        assert!(d[0].running && !d[1].running);
        let p = parse_pods(
            "kube-system   coredns-5d   Running   coredns\nprod  api-7f  Pending  app,sidecar\n",
        );
        assert_eq!(p[1].containers, vec!["app", "sidecar"]);
        assert_eq!(p[1].namespace, "prod");
        assert!(p[0].running);
    }
}

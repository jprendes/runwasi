use std::{collections::HashMap, io::ErrorKind, process::Stdio, time::Duration};

use nix::{
    errno::Errno, sys::signal::{kill, Signal}, unistd::Pid
};
use oci_spec::runtime::Hook;
use tokio::{io::AsyncWriteExt as _, process::Command};

use containerd_shim::monitor::{ExitEvent, Subject, Subscription, Topic, monitor_subscribe};

pub async fn run_hooks(hooks: impl IntoIterator<Item = &Hook>) -> anyhow::Result<()> {
    for hook in hooks {
        log::info!("run_hooks: {:?}", hook);
        let mut hook_command = Command::new(hook.path());
        // Based on OCI spec, the first argument of the args vector is the
        // arg0, which can be different from the path.  For example, path
        // may be "/usr/bin/true" and arg0 is set to "true". However, rust
        // command differenciates arg0 from args, where rust command arg
        // doesn't include arg0. So we have to make the split arg0 from the
        // rest of args.
        if let Some((arg0, args)) = hook.args().as_ref().and_then(|a| a.split_first()) {
            log::debug!("run_hooks arg0: {:?}, args: {:?}", arg0, args);
            hook_command.arg0(arg0).args(args)
        } else {
            hook_command.arg0(&hook.path().display().to_string())
        };

        let envs: HashMap<String, String> = if let Some(env) = hook.env() {
            env.iter()
                .filter_map(|e| {
                    let mut split = e.split('=');

                    split.next().map(|key| {
                        let value = split.collect::<Vec<&str>>().join("=");
                        (key.into(), value)
                    })
                })
                .collect()
            //parse_env(env)
        } else {
            HashMap::new()
        };
        log::debug!("run_hooks envs: {:?}", envs);

        let subs = monitor_subscribe(Topic::Pid)?;

        let mut hook_process = hook_command
            .env_clear()
            .envs(envs)
            .stdin(Stdio::piped())
            .spawn()?;

        let hook_process_pid = Pid::from_raw(hook_process.id().unwrap() as i32);

        if let Some(stdin) = &mut hook_process.stdin {
            // We want to ignore BrokenPipe here. A BrokenPipe indicates
            // either the hook is crashed/errored or it ran successfully.
            // Either way, this is an indication that the hook command
            // finished execution.  If the hook command was successful,
            // which we will check later in this function, we should not
            // fail this step here. We still want to check for all the other
            // error, in the case that the hook command is waiting for us to
            // write to stdin.
            let state = format!("{{ \"pid\": {} }}", std::process::id());
            if let Err(e) = stdin.write_all(state.as_bytes()).await {
                if e.kind() != ErrorKind::BrokenPipe {
                    // Not a broken pipe. The hook command may be waiting
                    // for us.
                    let _ = kill(hook_process_pid, Signal::SIGKILL);
                    return Err(e.into());
                }
            }
        }
        try_wait_pid(hook_process_pid.as_raw(), subs).await?;
    }
    Ok(())
}


pub async fn try_wait_pid(pid: i32, s: Subscription) -> Result<i32, Errno> {
    tokio::task::spawn_blocking(move || {
        while let Ok(ExitEvent { subject, exit_code }) = s.rx.recv_timeout(Duration::from_secs(2)) {
            let Subject::Pid(p) = subject else {
                continue;
            };
            if pid == p {
                return Ok(exit_code);
            }
        }
        Err(Errno::ECHILD)
    })
    .await
    .map_err(|_| Errno::ECHILD)?
}

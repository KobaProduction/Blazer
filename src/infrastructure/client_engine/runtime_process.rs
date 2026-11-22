use std::io::{self, Read};
use std::path::Path;

use vpn_domain::client::ClientError;

const RUNTIME_ARGS: [&str; 3] = ["--threads", "1", "--no-reload"];

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{fs::File, time::Duration};
    use win_custody::{Job, JobLimits, Launch, Process, WaitOutcome, anonymous_pipe, null_device};

    pub(in crate::client_engine) struct RuntimeProcess {
        job: Job,
        process: Process,
    }

    impl RuntimeProcess {
        pub(in crate::client_engine) fn spawn(
            directory: &Path,
            profile: &Path,
        ) -> io::Result<(Self, impl Read + Send + 'static)> {
            let job = Job::create(&JobLimits::new().kill_on_close(true))?;
            let (stdout, writer) = anonymous_pipe()?;
            let null = null_device()?;
            let mut command =
                Launch::new(directory.join(super::super::private::RUNTIME_EXECUTABLE));
            command
                .args(RUNTIME_ARGS)
                .arg(profile)
                .current_dir(directory)
                .create_no_window(true)
                .stdin(&null)
                .stdout(&writer)
                .stderr(&null);
            if let Some(system_root) = std::env::var_os("SystemRoot") {
                let path = Path::new(&system_root).join("System32");
                command
                    .env_clear()
                    .env("SystemRoot", system_root)
                    .env("PATH", path);
            }
            let process = command.spawn_in_job(&job)?;
            Ok((Self { job, process }, File::from(stdout)))
        }

        pub(in crate::client_engine) fn is_running(&mut self) -> io::Result<bool> {
            Ok(self.process.wait(Some(Duration::ZERO))? == WaitOutcome::TimedOut)
        }

        pub(in crate::client_engine) fn stop(&mut self) -> Result<(), ClientError> {
            self.job
                .terminate(1)
                .map_err(|_| ClientError::EngineStopped)?;
            match self.process.wait(Some(Duration::from_secs(2))) {
                Ok(WaitOutcome::Signaled) => Ok(()),
                _ => Err(ClientError::EngineStopped),
            }
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;
    use std::process::{Child, Stdio};

    pub(in crate::client_engine) struct RuntimeProcess(Child);

    impl RuntimeProcess {
        pub(in crate::client_engine) fn spawn(
            directory: &Path,
            profile: &Path,
        ) -> io::Result<(Self, impl Read + Send + 'static)> {
            let executable = directory.join(super::super::private::RUNTIME_EXECUTABLE);
            let mut child = super::super::engine_command(&executable, directory)
                .args(RUNTIME_ARGS)
                .arg(profile)
                .stdout(Stdio::piped())
                .spawn()?;
            let stdout = child.stdout.take().expect("piped BlazeCore stdout");
            Ok((Self(child), stdout))
        }

        pub(in crate::client_engine) fn is_running(&mut self) -> io::Result<bool> {
            Ok(self.0.try_wait()?.is_none())
        }

        pub(in crate::client_engine) fn stop(&mut self) -> Result<(), ClientError> {
            stop(&mut self.0)
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn stop(child: &mut Child) -> Result<(), ClientError> {
        super::super::stop_child(child)
    }

    #[cfg(target_os = "linux")]
    fn stop(child: &mut Child) -> Result<(), ClientError> {
        use rustix::process::{Pid, Signal, kill_process};
        use std::time::{Duration, Instant};

        let failure = ClientError::EngineStopped;
        if child.try_wait().map_err(|_| failure)?.is_some() {
            return Ok(());
        }
        // SIGTERM lets BlazeCore revert its owned nftables, routing and DNS state.
        kill_process(Pid::from_child(child), Signal::TERM).map_err(|_| failure)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if super::super::wait_for_exit(child, Duration::from_millis(20))
                .map_err(|_| failure)?
                .is_some()
            {
                return Ok(());
            }
        }
        Err(failure)
    }

    #[cfg(all(test, target_os = "linux"))]
    #[test]
    fn linux_shutdown_runs_the_child_cleanup_handler() {
        use std::io::{BufRead, BufReader};
        use std::process::Command;

        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                "trap 'echo cleaned; exit 0' TERM; echo ready; while :; do sleep 0.1; done",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut ready = String::new();
        output.read_line(&mut ready).unwrap();
        assert_eq!(ready.trim(), "ready");
        stop(&mut child).unwrap();
        assert!(child.try_wait().unwrap().unwrap().success());
        let mut cleanup = String::new();
        output.read_line(&mut cleanup).unwrap();
        assert_eq!(cleanup.trim(), "cleaned");
    }
}

pub(super) use platform::RuntimeProcess;

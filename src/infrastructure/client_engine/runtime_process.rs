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
            let mut command = Launch::new(directory.join("engine.exe"));
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
            let mut child = super::super::engine_command(&directory.join("engine.exe"), directory)
                .args(RUNTIME_ARGS)
                .arg(profile)
                .stdout(Stdio::piped())
                .spawn()?;
            let stdout = child.stdout.take().expect("piped Shoes stdout");
            Ok((Self(child), stdout))
        }

        pub(in crate::client_engine) fn is_running(&mut self) -> io::Result<bool> {
            Ok(self.0.try_wait()?.is_none())
        }

        pub(in crate::client_engine) fn stop(&mut self) -> Result<(), ClientError> {
            super::super::stop_child(&mut self.0)
        }
    }
}

pub(super) use platform::RuntimeProcess;

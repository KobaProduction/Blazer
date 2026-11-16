mod command;
mod render;

use std::io::{self, BufRead};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant};

use vpn_backend::{
    BackendOptions, ClientCommand, ClientError, ClientHandle, ClientSnapshot, ConnectionMode,
    ConnectionStatus, SubmitError,
};

use command::Action;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<_> = arguments.iter().map(String::as_str).collect();
    let action = match Action::parse(&words) {
        Ok(Some(action)) => action,
        Ok(None) => {
            render::help();
            return ExitCode::SUCCESS;
        }
        Err(()) => {
            eprintln!("Unsupported command. Use vpn-headless --help");
            return ExitCode::from(2);
        }
    };
    match run(action) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}", render::error_message(error));
            ExitCode::FAILURE
        }
    }
}

fn options() -> BackendOptions {
    let mut options = BackendOptions::default();
    if let Some(path) = std::env::var_os("VPN_CLIENT_DATA_DIR") {
        options.data_dir = path.into();
    }
    if let Some(path) = std::env::var_os("VPN_CLIENT_SHOES_PATH") {
        options.engine_path = path.into();
    }
    options
}

fn run(action: Action) -> Result<(), ClientError> {
    let options = options();
    let engine_present = options.engine_path.is_file();
    let mut client = ClientHandle::spawn(options)?;
    let (wakeup, updates) = mpsc::sync_channel(1);
    let backend_wakeup = wakeup.clone();
    client.set_notifier(Arc::new(move || {
        let _ = backend_wakeup.try_send(());
    }));

    let result = run_action(action, &client, &updates, wakeup, engine_present);
    let shutdown = client.shutdown();
    result.and(shutdown)
}

fn run_action(
    action: Action,
    client: &ClientHandle,
    updates: &Receiver<()>,
    wakeup: SyncSender<()>,
    engine_present: bool,
) -> Result<(), ClientError> {
    let initial = wait_for(client, updates, |snapshot| !snapshot.busy)?;
    match action {
        Action::Status => render::status(&initial, engine_present),
        Action::Profiles => render::profiles(&initial),
        Action::Subscriptions => render::subscriptions(&initial),
        Action::Settings => render::settings(&initial),
        Action::Diagnostics => render::diagnostics(&initial),
        Action::Routing(uuid) => render::routing(&initial, uuid.as_deref())?,
        Action::Serve(mode) => return serve_connection(client, updates, wakeup, mode),
        Action::Execute(operation) => {
            let (command, completion) =
                operation.request(io::stdin().lock(), &initial.routing.catalog)?;
            let result = complete(client, updates, command)?;
            render::completion(completion, &result)?;
        }
    }
    Ok(())
}

fn complete(
    client: &ClientHandle,
    updates: &Receiver<()>,
    command: ClientCommand,
) -> Result<ClientSnapshot, ClientError> {
    let request = client.send(command).map_err(|error| match error {
        SubmitError::Rejected(error) => error,
        SubmitError::Busy => ClientError::Timeout,
        SubmitError::Stopped => ClientError::EngineStopped,
    })?;
    wait_for(client, updates, |snapshot| {
        snapshot.completed_request == Some(request)
    })
}

fn wait_for(
    client: &ClientHandle,
    updates: &Receiver<()>,
    ready: impl Fn(&ClientSnapshot) -> bool,
) -> Result<ClientSnapshot, ClientError> {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let snapshot = client.snapshot();
        if ready(&snapshot) {
            return snapshot.last_error.map_or(Ok(snapshot), Err);
        }
        updates
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| ClientError::Timeout)?;
    }
}

fn serve_connection(
    client: &ClientHandle,
    updates: &Receiver<()>,
    wakeup: SyncSender<()>,
    mode: ConnectionMode,
) -> Result<(), ClientError> {
    let connected = complete(client, updates, ClientCommand::Connect { mode })?;
    render::connection(&connected, mode)?;
    let stop = Arc::new(AtomicBool::new(false));
    let stdin_stop = stop.clone();
    std::thread::Builder::new()
        .name("vpn-headless-stdin".to_owned())
        .spawn(move || {
            let _ = io::stdin().lock().skip_until(b'\n');
            stdin_stop.store(true, Ordering::Release);
            let _ = wakeup.try_send(());
        })
        .map_err(|_| ClientError::EngineStart)?;

    loop {
        let snapshot = client.snapshot();
        if snapshot.connection != ConnectionStatus::Connected {
            return Err(snapshot.last_error.unwrap_or(ClientError::EngineStopped));
        }
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        updates.recv().map_err(|_| ClientError::EngineStopped)?;
    }
}

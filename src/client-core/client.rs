//! Nonblocking GUI/headless facade. All I/O and mutable catalog state live on one worker.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use vpn_application::MAX_SUBSCRIPTION_BYTES;
use vpn_application::client::{
    Cancellation, CancellationSignal, ClientApplication, ClientEnginePort, ClientServices,
    EngineRuntime, Subscription, TelemetrySettings, UuidPort,
};
pub use vpn_application::session::*;
use vpn_application::subscription_refresh::AutomaticRefresh;
use vpn_infrastructure::client_engine::{
    APPROVED_ENGINE_SHA256, APPROVED_ENGINE_VERSION, BlazeCoreProcess, TrustedEngineSpec,
};
use vpn_infrastructure::client_http::HttpSubscriptionSource;
use vpn_infrastructure::client_normalize::SubscriptionNormalizer;
use vpn_infrastructure::client_probe::TcpProfileProbe;
use vpn_infrastructure::client_store::{CatalogProtection, SealedCatalogStore};

use crate::telemetry::Telemetry;

type Notifier = Arc<dyn Fn() + Send + Sync>;
const COMMAND_CAPACITY: usize = 32;
const ENGINE_CHECK_INTERVAL: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum SubscriptionRefreshMode {
    #[default]
    Manual,
    Automatic,
    Platform,
}

#[derive(Clone)]
pub struct ClientCoreOptions {
    pub data_dir: PathBuf,
    pub engine_path: PathBuf,
    pub local_proxy_port: u16,
    pub expected_engine_sha256: String,
    pub expected_engine_version: String,
    pub subscription_refresh: SubscriptionRefreshMode,
}

impl Default for ClientCoreOptions {
    fn default() -> Self {
        Self {
            data_dir: std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .map(|path| path.join("Blazer"))
                .unwrap_or_default(),
            engine_path: std::env::current_exe()
                .ok()
                .and_then(|path| path.parent().map(|path| path.join("blazecore.exe")))
                .unwrap_or_default(),
            local_proxy_port: 0,
            expected_engine_sha256: APPROVED_ENGINE_SHA256.to_owned(),
            expected_engine_version: APPROVED_ENGINE_VERSION.to_owned(),
            subscription_refresh: SubscriptionRefreshMode::Manual,
        }
    }
}

struct Envelope {
    request_id: RequestId,
    work: Work,
}

enum Work {
    Command(ClientCommand),
    Foreground(bool),
    PeriodicRefresh(Arc<CancellationSignal>),
    Shutdown,
}

struct Submission {
    next_request: u64,
    queued: usize,
    closed: bool,
    scheduled_request: Option<(u64, Arc<CancellationSignal>)>,
}

impl Submission {
    fn enqueue(
        &mut self,
        sender: &SyncSender<Envelope>,
        work: Work,
    ) -> Result<RequestId, SubmitError> {
        if self.closed {
            return Err(SubmitError::Stopped);
        }
        let disconnect = matches!(&work, Work::Command(ClientCommand::Disconnect));
        if !disconnect && self.queued >= COMMAND_CAPACITY {
            return Err(SubmitError::Busy);
        }
        let request_id = RequestId(self.next_request);
        let next_request = self
            .next_request
            .checked_add(1)
            .ok_or(SubmitError::Stopped)?;
        sender
            .try_send(Envelope { request_id, work })
            .map_err(|error| match error {
                TrySendError::Full(_) => SubmitError::Busy,
                TrySendError::Disconnected(_) => SubmitError::Stopped,
            })?;
        self.next_request = next_request;
        self.queued += 1;
        Ok(request_id)
    }

    fn interrupt_scheduled(&self) {
        if let Some((id, signal)) = &self.scheduled_request {
            signal.cancel(*id);
        }
    }
}

pub struct RefreshControl {
    sender: SyncSender<Envelope>,
    submission: Arc<Mutex<Submission>>,
    status: Arc<RwLock<RefreshStatus>>,
}

impl RefreshControl {
    pub fn set_foreground(&self, visible: bool) -> Result<(), SubmitError> {
        self.submit(Work::Foreground(visible)).map(|_| ())
    }

    pub fn status(&self) -> RefreshStatus {
        self.status
            .read()
            .map(|status| *status)
            .unwrap_or(RefreshStatus::Failed)
    }

    pub fn periodic(&self) -> Result<PeriodicRefresh, SubmitError> {
        let signal = Arc::new(CancellationSignal::default());
        let id = self.submit(Work::PeriodicRefresh(signal.clone()))?;
        Ok(PeriodicRefresh { id, signal })
    }

    fn submit(&self, work: Work) -> Result<RequestId, SubmitError> {
        self.submission
            .lock()
            .map_err(|_| SubmitError::Stopped)?
            .enqueue(&self.sender, work)
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum RefreshStatus {
    #[default]
    Starting,
    Scheduled {
        next_unix: Option<u64>,
        completed: Option<RequestId>,
    },
    Failed,
}

pub struct PeriodicRefresh {
    pub id: RequestId,
    signal: Arc<CancellationSignal>,
}

impl PeriodicRefresh {
    pub fn cancel(&self) {
        self.signal.cancel(self.id.0);
    }
}

pub struct ClientHandle {
    sender: SyncSender<Envelope>,
    snapshot: Arc<RwLock<ClientSnapshot>>,
    notifier: Arc<Mutex<Option<Notifier>>>,
    cancellation: Arc<CancellationSignal>,
    submission: Arc<Mutex<Submission>>,
    refresh_status: Arc<RwLock<RefreshStatus>>,
    done: Mutex<Receiver<Result<(), ClientError>>>,
    worker: Option<JoinHandle<()>>,
    shutdown_requested: bool,
    shutdown_result: Option<Result<(), ClientError>>,
}

impl ClientHandle {
    pub fn spawn(options: ClientCoreOptions) -> Result<Self, ClientError> {
        if options.data_dir.as_os_str().is_empty() {
            return Err(ClientError::InvalidInput);
        }
        let component_versions = ComponentVersions {
            expected_blazecore: (!options.expected_engine_version.is_empty())
                .then(|| options.expected_engine_version.clone()),
            #[cfg(windows)]
            tun2proxy: vpn_infrastructure::client_engine::tun2proxy_version(),
            #[cfg(not(windows))]
            tun2proxy: None,
            #[cfg(windows)]
            bundled_wintun: vpn_infrastructure::client_engine::bundled_wintun_version()
                .map(str::to_owned),
            #[cfg(not(windows))]
            bundled_wintun: None,
        };
        Self::spawn_worker(
            options.subscription_refresh,
            component_versions,
            move || application(&options),
        )
    }

    pub fn spawn_with_platform(
        data_dir: PathBuf,
        protection: Box<dyn CatalogProtection>,
        engine: Box<dyn ClientEnginePort>,
        subscription_refresh: SubscriptionRefreshMode,
    ) -> Result<Self, ClientError> {
        if !data_dir.is_absolute() {
            return Err(ClientError::InvalidInput);
        }
        Self::spawn_worker(
            subscription_refresh,
            ComponentVersions::default(),
            move || {
                let store = SealedCatalogStore::with_protection(data_dir, protection)?;
                application_with_adapters(store, engine)
            },
        )
    }

    fn spawn_worker(
        subscription_refresh: SubscriptionRefreshMode,
        component_versions: ComponentVersions,
        load: impl FnOnce() -> Result<LoadedApplication, ClientError> + Send + 'static,
    ) -> Result<Self, ClientError> {
        // Commands share a FIFO, with one additional slot reserved for Disconnect.
        let (sender, receiver) = mpsc::sync_channel(COMMAND_CAPACITY + 1);
        let (done_sender, done) = mpsc::sync_channel(1);
        let snapshot = Arc::new(RwLock::new(ClientSnapshot {
            component_versions,
            ..ClientSnapshot::default()
        }));
        let notifier = Arc::new(Mutex::new(None));
        let cancellation = Arc::new(CancellationSignal::default());
        let refresh_status = Arc::new(RwLock::new(RefreshStatus::default()));
        let submission = Arc::new(Mutex::new(Submission {
            next_request: 1,
            queued: 0,
            closed: false,
            scheduled_request: None,
        }));
        let context = WorkerContext {
            snapshot: snapshot.clone(),
            notifier: notifier.clone(),
            cancellation: cancellation.clone(),
            submission: submission.clone(),
            refresh_status: refresh_status.clone(),
        };
        let worker = std::thread::Builder::new()
            .name("blazer-backend".to_owned())
            .spawn(move || {
                let result = run_worker(load, receiver, context, subscription_refresh);
                let _ = done_sender.send(result);
            })
            .map_err(|_| ClientError::StorageUnavailable)?;
        Ok(Self {
            sender,
            snapshot,
            notifier,
            cancellation,
            submission,
            refresh_status,
            done: Mutex::new(done),
            worker: Some(worker),
            shutdown_requested: false,
            shutdown_result: None,
        })
    }

    pub fn set_notifier(&self, notifier: Notifier) {
        if let Ok(mut target) = self.notifier.lock() {
            *target = Some(notifier);
        }
    }

    pub fn snapshot(&self) -> ClientSnapshot {
        self.snapshot
            .read()
            .map(|value| value.clone())
            .unwrap_or_else(|_| ClientSnapshot {
                busy: false,
                last_error: Some(ClientError::EngineStopped),
                ..ClientSnapshot::default()
            })
    }

    pub fn send(&self, command: ClientCommand) -> Result<RequestId, SubmitError> {
        validate_command(&command).map_err(SubmitError::Rejected)?;
        let mut submission = self.submission.lock().map_err(|_| SubmitError::Stopped)?;
        let disconnect = matches!(command, ClientCommand::Disconnect);
        let request_id = submission.enqueue(&self.sender, Work::Command(command))?;
        if disconnect {
            self.cancellation.cancel(request_id.0);
        }
        submission.interrupt_scheduled();
        Ok(request_id)
    }

    pub fn refresh_control(&self) -> RefreshControl {
        RefreshControl {
            sender: self.sender.clone(),
            submission: self.submission.clone(),
            status: self.refresh_status.clone(),
        }
    }

    pub fn shutdown(&mut self) -> Result<(), ClientError> {
        if let Some(result) = self.shutdown_result {
            return result;
        }
        if !self.shutdown_requested {
            let mut submission = self
                .submission
                .lock()
                .map_err(|_| ClientError::EngineStopped)?;
            self.shutdown_requested = true;
            submission.closed = true;
            self.cancellation.cancel(u64::MAX);
            submission.interrupt_scheduled();
            let _ = self.sender.try_send(Envelope {
                request_id: RequestId(u64::MAX),
                work: Work::Shutdown,
            });
        }
        let completion = self
            .done
            .lock()
            .map_err(|_| ClientError::ShutdownTimeout)?
            .recv_timeout(Duration::from_secs(12));
        let result = match completion {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => return Err(ClientError::ShutdownTimeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(ClientError::EngineStopped),
        };
        let result = match self.worker.take().map(JoinHandle::join) {
            Some(Err(_)) => Err(ClientError::EngineStopped),
            _ => result,
        };
        self.shutdown_result = Some(result);
        result
    }
}

impl Drop for ClientHandle {
    fn drop(&mut self) {
        if !self.shutdown_requested {
            let _ = self.shutdown();
        }
    }
}

fn validate_command(command: &ClientCommand) -> Result<(), ClientError> {
    validate_routing_command(command)?;
    if command_subscription(command).is_some_and(|identifier| identifier.len() > 64) {
        return Err(ClientError::InvalidInput);
    }
    match command {
        ClientCommand::ImportText { title: name, text } => {
            if text.len() > MAX_SUBSCRIPTION_BYTES {
                return Err(ClientError::PayloadTooLarge);
            }
            Subscription::validate_title(name)?;
        }
        ClientCommand::AddSubscription { title: name, url }
        | ClientCommand::UpdateSubscription {
            title: name, url, ..
        } => {
            if url.len() > 8192 {
                return Err(ClientError::PayloadTooLarge);
            }
            Subscription::validate_title_override(name)?;
            if url.is_empty() {
                return Err(ClientError::InvalidInput);
            }
        }
        ClientCommand::RenameSubscription {
            subscription_uuid,
            title,
        } => {
            if subscription_uuid.len() > 64 {
                return Err(ClientError::InvalidInput);
            }
            Subscription::validate_title_override(title)?;
        }
        ClientCommand::SelectProfile { profile_uuid }
        | ClientCommand::ProbeProfile { profile_uuid }
        | ClientCommand::RemoveProfile { profile_uuid }
            if profile_uuid.len() > 64 =>
        {
            return Err(ClientError::InvalidInput);
        }
        ClientCommand::ProbeProfiles {
            subscription_uuid: Some(uuid),
        } if uuid.len() > 64 => {
            return Err(ClientError::InvalidInput);
        }
        ClientCommand::ApproveProviderChange { nonce, .. }
        | ClientCommand::RejectProviderChange { nonce, .. }
            if nonce.len() != 36
                || !nonce
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() || byte == b'-') =>
        {
            return Err(ClientError::InvalidInput);
        }
        ClientCommand::SetSubscriptionFallback { url: Some(url), .. } if url.len() > 8192 => {
            return Err(ClientError::PayloadTooLarge);
        }
        ClientCommand::SetSubscriptionUserAgent { value: Some(value) } => {
            validate_subscription_user_agent(value.expose())?;
        }
        ClientCommand::SetTelemetrySettings {
            endpoint, bearer, ..
        } if endpoint.len() > 8192
            || matches!(bearer, TelemetryBearerChange::Replace(value) if value.len() > 4096) =>
        {
            return Err(ClientError::PayloadTooLarge);
        }
        _ => {}
    }
    Ok(())
}

fn validate_routing_command(command: &ClientCommand) -> Result<(), ClientError> {
    let uuid = match command {
        ClientCommand::SaveRoutingProfile { uuid, profile } => {
            profile.validate()?;
            uuid.as_deref()
        }
        ClientCommand::RemoveRoutingProfile { profile_uuid } => Some(profile_uuid.as_str()),
        ClientCommand::SetDnsPolicy {
            profile_uuid,
            policy,
        } => {
            policy.validate()?;
            Some(profile_uuid.as_str())
        }
        ClientCommand::SetRoutingAssignment {
            subscription_uuid,
            assignment,
        } => {
            if subscription_uuid
                .as_ref()
                .is_some_and(|uuid| uuid.len() > 64)
            {
                return Err(ClientError::InvalidInput);
            }
            assignment.profile_uuid.as_deref()
        }
        ClientCommand::ReconnectRouting { pending } => {
            if pending.connection_id.is_empty() || pending.connection_id.len() > 64 {
                return Err(ClientError::InvalidInput);
            }
            if pending
                .target
                .as_ref()
                .is_some_and(|target| target.revision == 0)
            {
                return Err(ClientError::InvalidInput);
            }
            pending
                .target
                .as_ref()
                .map(|target| target.profile_uuid.as_str())
        }
        _ => None,
    };
    if uuid.is_some_and(|uuid| uuid.is_empty() || uuid.len() > 64) {
        return Err(ClientError::InvalidInput);
    }
    Ok(())
}

struct WorkerContext {
    snapshot: Arc<RwLock<ClientSnapshot>>,
    notifier: Arc<Mutex<Option<Notifier>>>,
    cancellation: Arc<CancellationSignal>,
    submission: Arc<Mutex<Submission>>,
    refresh_status: Arc<RwLock<RefreshStatus>>,
}

impl WorkerContext {
    fn begin_command(&self) -> bool {
        let Ok(mut submission) = self.submission.lock() else {
            return false;
        };
        submission.queued = submission.queued.saturating_sub(1);
        !submission.closed
    }

    fn publish(&self, update: impl FnOnce(&mut ClientSnapshot)) {
        if let Ok(mut snapshot) = self.snapshot.write() {
            update(&mut snapshot);
            snapshot.revision = snapshot.revision.saturating_add(1);
        }
        self.notify();
    }

    fn notify(&self) {
        let notifier = self.notifier.lock().ok().and_then(|value| value.clone());
        if let Some(notifier) = notifier {
            notifier();
        }
    }

    fn publish_refresh(&self, deadline: Option<u64>, completed: Option<RequestId>) {
        let Ok(mut status) = self.refresh_status.write() else {
            return;
        };
        let previous = match *status {
            RefreshStatus::Scheduled { completed, .. } => completed,
            _ => None,
        };
        let next = RefreshStatus::Scheduled {
            next_unix: deadline,
            completed: completed.or(previous),
        };
        if *status != next {
            *status = next;
            drop(status);
            self.notify();
        }
    }

    fn begin_scheduled(&self) -> Option<Cancellation> {
        let mut submission = self.submission.lock().ok()?;
        if submission.closed || submission.queued != 0 {
            return None;
        }
        let request = submission.next_request;
        submission.next_request = request.checked_add(1)?;
        submission.scheduled_request = Some((request, self.cancellation.clone()));
        Some(Cancellation::new(self.cancellation.clone(), request))
    }

    fn begin_periodic(
        &self,
        request: RequestId,
        signal: Arc<CancellationSignal>,
    ) -> Option<Cancellation> {
        let mut submission = self.submission.lock().ok()?;
        if submission.closed || submission.queued != 0 {
            return None;
        }
        submission.scheduled_request = Some((request.0, signal.clone()));
        Some(Cancellation::new(signal, request.0))
    }

    fn end_scheduled(&self) {
        if let Ok(mut submission) = self.submission.lock() {
            submission.scheduled_request = None;
        }
    }

    fn last_error(&self) -> Option<ClientError> {
        self.snapshot.read().ok()?.last_error
    }
}

struct SystemUuids;

impl UuidPort for SystemUuids {
    fn new_uuid(&self) -> String {
        vpn_infrastructure::new_profile_uuid()
    }
}

struct LoadedApplication {
    application: ClientApplication,
    subscription_hwid: Option<Secret>,
}

fn application(options: &ClientCoreOptions) -> Result<LoadedApplication, ClientError> {
    let store = SealedCatalogStore::new(options.data_dir.clone())?;
    let engine = BlazeCoreProcess::new(
        TrustedEngineSpec {
            source_path: options.engine_path.clone(),
            expected_sha256: options.expected_engine_sha256.clone(),
            expected_version: options.expected_engine_version.clone(),
        },
        options.data_dir.clone(),
        options.local_proxy_port,
    );
    #[cfg(windows)]
    let engine = vpn_infrastructure::client_engine::WindowsEngine::new(engine);
    application_with_adapters(store, Box::new(engine))
}

fn application_with_adapters(
    store: SealedCatalogStore,
    engine: Box<dyn ClientEnginePort>,
) -> Result<LoadedApplication, ClientError> {
    let subscription_hwid = store.subscription_hwid().ok();
    let source = HttpSubscriptionSource::new(store.directory().to_path_buf());
    let application = ClientApplication::load(ClientServices {
        store: Box::new(store),
        source: Box::new(source),
        normalizer: Box::new(SubscriptionNormalizer),
        uuids: Box::new(SystemUuids),
        profile_probe: Box::new(TcpProfileProbe),
        engine,
    })?;
    Ok(LoadedApplication {
        application,
        subscription_hwid,
    })
}

fn run_worker(
    load: impl FnOnce() -> Result<LoadedApplication, ClientError>,
    receiver: Receiver<Envelope>,
    context: WorkerContext,
    subscription_refresh: SubscriptionRefreshMode,
) -> Result<(), ClientError> {
    let application = load();
    let LoadedApplication {
        mut application,
        subscription_hwid,
    } = match application {
        Ok(application) => application,
        Err(error) => {
            if let Ok(mut submission) = context.submission.lock() {
                submission.closed = true;
            }
            if let Ok(mut status) = context.refresh_status.write() {
                *status = RefreshStatus::Failed;
            }
            context.publish(|snapshot| {
                snapshot.busy = false;
                snapshot.last_error = Some(error);
            });
            return Err(error);
        }
    };
    let mut state = WorkerState::new(&application.catalog().settings.telemetry, subscription_hwid);
    state.record(ClientEventKind::Started);
    state.publish(&application, &context, false, None, None);
    state.drive(&mut application, &context, &receiver, subscription_refresh);
    let result = application.disconnect();
    state.finish_disconnect(result);
    state.record(match result {
        Ok(()) => ClientEventKind::Disconnected,
        Err(error) => ClientEventKind::Failed(error),
    });
    state.publish(&application, &context, false, result.err(), None);
    result
}

fn receive_command(
    receiver: &Receiver<Envelope>,
    wait: Option<Duration>,
) -> Result<Envelope, mpsc::RecvTimeoutError> {
    match wait {
        Some(wait) => receiver.recv_timeout(wait),
        None => receiver
            .recv()
            .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
    }
}

impl WorkerState {
    fn drive(
        &mut self,
        application: &mut ClientApplication,
        context: &WorkerContext,
        receiver: &Receiver<Envelope>,
        mode: SubscriptionRefreshMode,
    ) {
        let mut refresh = RefreshDriver::new(mode, application);
        loop {
            refresh.publish_deadline(application, context);
            let engine_wait =
                (self.connection == ConnectionStatus::Connected).then_some(ENGINE_CHECK_INTERVAL);
            let wait = refresh
                .wait(application)
                .into_iter()
                .chain(engine_wait)
                .min();
            match receive_command(receiver, wait) {
                Ok(message) => {
                    if !self.handle_work(application, context, message, &mut refresh) {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.check_connected_engine(application, context);
                    if refresh.local {
                        self.refresh_when_due(application, context, &mut refresh.schedule);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    fn handle_work(
        &mut self,
        application: &mut ClientApplication,
        context: &WorkerContext,
        message: Envelope,
        refresh: &mut RefreshDriver,
    ) -> bool {
        if !context.begin_command() {
            return false;
        }
        self.check_connected_engine(application, context);
        match message.work {
            Work::Shutdown => return false,
            Work::Command(command) => self.handle_queued(
                application,
                context,
                command,
                message.request_id,
                &mut refresh.schedule,
            ),
            Work::Foreground(visible) if refresh.mode == SubscriptionRefreshMode::Platform => {
                refresh.local = visible;
                if visible {
                    refresh
                        .schedule
                        .start_launch(&application.catalog().subscriptions);
                }
            }
            Work::PeriodicRefresh(signal) if refresh.mode == SubscriptionRefreshMode::Platform => {
                self.run_periodic(
                    application,
                    context,
                    &mut refresh.schedule,
                    message.request_id,
                    signal,
                );
            }
            _ => {}
        }
        true
    }

    fn handle_queued(
        &mut self,
        application: &mut ClientApplication,
        context: &WorkerContext,
        command: ClientCommand,
        id: RequestId,
        schedule: &mut AutomaticRefresh,
    ) {
        let handled_subscription = refresh_or_removal(&command).map(str::to_owned);
        if self.handle(application, context, command, id).is_ok()
            && let Some(uuid) = handled_subscription
        {
            schedule.forget(&uuid);
        }
    }

    fn check_connected_engine(
        &mut self,
        application: &mut ClientApplication,
        context: &WorkerContext,
    ) {
        if self.connection == ConnectionStatus::Connected {
            self.check_engine(application, context);
        }
    }

    fn refresh_when_due(
        &mut self,
        application: &mut ClientApplication,
        context: &WorkerContext,
        schedule: &mut AutomaticRefresh,
    ) {
        let Some((uuid, Duration::ZERO)) = next_refresh(schedule, application, true) else {
            return;
        };
        let Some(cancel) = context.begin_scheduled() else {
            return;
        };
        let uuid = uuid.to_owned();
        self.refresh_scheduled(application, context, schedule, &uuid, &cancel);
        context.end_scheduled();
    }

    fn run_periodic(
        &mut self,
        application: &mut ClientApplication,
        context: &WorkerContext,
        schedule: &mut AutomaticRefresh,
        id: RequestId,
        signal: Arc<CancellationSignal>,
    ) {
        if let Some(cancel) = context.begin_periodic(id, signal) {
            while cancel.check().is_ok() {
                let Some((uuid, Duration::ZERO)) = next_refresh(schedule, application, false)
                else {
                    break;
                };
                let uuid = uuid.to_owned();
                if !self.refresh_scheduled(application, context, schedule, &uuid, &cancel) {
                    break;
                }
            }
            context.end_scheduled();
        }
        context.publish_refresh(
            schedule.periodic_deadline(&application.catalog().subscriptions),
            Some(id),
        );
    }

    fn refresh_scheduled(
        &mut self,
        application: &mut ClientApplication,
        context: &WorkerContext,
        schedule: &mut AutomaticRefresh,
        uuid: &str,
        cancel: &Cancellation,
    ) -> bool {
        let started = Instant::now();
        let result = application.refresh_subscription(uuid, cancel);
        if result == Err(ClientError::Cancelled) {
            return false;
        }
        let now = Instant::now();
        let unix = unix_seconds();
        if result.is_err()
            && let Some(unix) = unix
            && let Err(error) = application.record_automatic_refresh_failure(uuid, unix, cancel)
        {
            if error == ClientError::Cancelled {
                return false;
            }
            self.record(ClientEventKind::Failed(error));
        }
        schedule.attempted(uuid, now, unix.unwrap_or_default(), result.is_err());
        self.record_subscription_result(application, uuid.to_owned(), result);
        self.telemetry.record_with_duration(
            refresh_event(application, uuid, result),
            Some(started.elapsed()),
        );
        self.publish(application, context, false, context.last_error(), None);
        true
    }
}

struct RefreshDriver {
    mode: SubscriptionRefreshMode,
    local: bool,
    schedule: AutomaticRefresh,
}

impl RefreshDriver {
    fn new(mode: SubscriptionRefreshMode, application: &ClientApplication) -> Self {
        let mut schedule = AutomaticRefresh::default();
        let local = mode == SubscriptionRefreshMode::Automatic;
        if local {
            schedule.start_launch(&application.catalog().subscriptions);
        }
        Self {
            mode,
            local,
            schedule,
        }
    }

    fn wait(&self, application: &ClientApplication) -> Option<Duration> {
        self.local
            .then(|| next_refresh(&self.schedule, application, true))
            .flatten()
            .map(|(_, wait)| wait)
    }

    fn publish_deadline(&self, application: &ClientApplication, context: &WorkerContext) {
        if self.mode == SubscriptionRefreshMode::Platform {
            context.publish_refresh(
                self.schedule
                    .periodic_deadline(&application.catalog().subscriptions),
                None,
            );
        }
    }
}

fn unix_seconds() -> Option<u64> {
    Some(SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs())
}

fn next_refresh<'a>(
    schedule: &AutomaticRefresh,
    application: &'a ClientApplication,
    include_launch: bool,
) -> Option<(&'a str, Duration)> {
    let unix = unix_seconds()?;
    schedule.next(
        &application.catalog().subscriptions,
        application.active_subscription_uuid(),
        Instant::now(),
        unix,
        include_launch,
    )
}

struct WorkerState {
    connection: ConnectionStatus,
    local_proxy: Option<String>,
    cleanup_required: bool,
    telemetry: Telemetry,
    subscription_hwid: Option<Secret>,
    subscription_errors: HashMap<String, ClientError>,
}

impl WorkerState {
    fn new(telemetry_settings: &TelemetrySettings, subscription_hwid: Option<Secret>) -> Self {
        Self {
            connection: ConnectionStatus::Disconnected,
            local_proxy: None,
            cleanup_required: false,
            telemetry: Telemetry::new(telemetry_settings),
            subscription_hwid,
            subscription_errors: HashMap::new(),
        }
    }

    fn record(&mut self, kind: ClientEventKind) {
        self.telemetry.record(kind);
    }

    fn publish(
        &self,
        application: &ClientApplication,
        context: &WorkerContext,
        busy: bool,
        error: Option<ClientError>,
        request: Option<RequestId>,
    ) {
        let catalog = application.catalog();
        let provider_subscriptions: Vec<_> =
            catalog.subscriptions.iter().map(provider_summary).collect();
        let profiles = catalog
            .profiles
            .iter()
            .map(|item| ProfileSummary {
                profile_uuid: item.profile.profile_uuid.clone(),
                name: item.profile.name.clone(),
                protocol: item.profile.protocol.name().to_owned(),
                server: if provider_subscriptions.iter().any(|subscription| {
                    subscription.raw_config_hidden
                        && item.subscription_uuid.as_deref()
                            == Some(&subscription.subscription_uuid)
                }) {
                    String::new()
                } else {
                    format!("{}:{}", item.profile.host, item.profile.port)
                },
                subscription_uuid: item.subscription_uuid.clone(),
            })
            .collect();
        let subscriptions = catalog
            .subscriptions
            .iter()
            .map(|item| SubscriptionSummary {
                subscription_uuid: item.subscription_uuid.clone(),
                title: item.title.clone(),
                auto_title: item.auto_title,
                profile_count: catalog
                    .profiles
                    .iter()
                    .filter(|profile| {
                        profile.subscription_uuid.as_deref() == Some(&item.subscription_uuid)
                    })
                    .count()
                    + item.unavailable_profiles.len(),
                last_error: self
                    .subscription_errors
                    .get(&item.subscription_uuid)
                    .copied(),
            })
            .collect();
        context.publish(|snapshot| {
            snapshot.profiles = profiles;
            snapshot.profile_latencies = application.profile_latencies().clone();
            snapshot.tcp_probe_available =
                !self.cleanup_required && application.can_probe_profile();
            snapshot.subscriptions = subscriptions;
            snapshot.unavailable_profiles = catalog
                .subscriptions
                .iter()
                .filter(|item| !item.unavailable_profiles.is_empty())
                .map(|item| {
                    (
                        item.subscription_uuid.clone(),
                        item.unavailable_profiles
                            .iter()
                            .map(UnavailableProfile::for_display)
                            .collect(),
                    )
                })
                .collect();
            snapshot.subscription_usage = catalog
                .subscriptions
                .iter()
                .filter_map(|subscription| {
                    let usage =
                        vpn_infrastructure::subscription_usage(&subscription.provider.metadata)?;
                    Some((subscription.subscription_uuid.clone(), usage))
                })
                .collect();
            snapshot.provider_subscriptions = provider_subscriptions;
            snapshot.selected_profile_uuid = catalog.selected_profile_uuid.clone();
            snapshot.active_profile_uuid = application.active_profile_uuid().map(str::to_owned);
            snapshot.language = catalog.settings.language;
            snapshot.selected_mode = catalog.settings.connection_mode;
            snapshot.active_mode = application.active_mode();
            snapshot.mode_capabilities = application.capabilities();
            snapshot.connection = self.connection;
            snapshot.local_proxy = self.local_proxy.clone();
            snapshot.local_proxy_port = catalog.settings.local_proxy_port;
            snapshot.send_subscription_hwid = catalog.settings.send_subscription_hwid;
            snapshot
                .subscription_user_agent
                .clone_from(&catalog.settings.subscription_user_agent);
            snapshot
                .subscription_hwid
                .clone_from(&self.subscription_hwid);
            snapshot.http_proxy = self.local_proxy.clone();
            snapshot.cleanup_required = self.cleanup_required;
            snapshot.busy = busy;
            snapshot.last_error = error;
            if !busy && request.is_some() {
                snapshot.completed_request = request;
            }
            snapshot.diagnostics = self.telemetry.summary();
            snapshot.telemetry_export_configured = self.telemetry.export_configured();
            snapshot.telemetry_settings = (&catalog.settings.telemetry).into();
            snapshot.diagnostic_events = self.telemetry.events();
            snapshot.routing = application.routing_snapshot();
            if self.connection != ConnectionStatus::Connected || self.cleanup_required {
                snapshot.routing.pending_reconnect = None;
            }
        });
    }

    fn handle(
        &mut self,
        application: &mut ClientApplication,
        context: &WorkerContext,
        command: ClientCommand,
        request_id: RequestId,
    ) -> Result<(), ClientError> {
        if let ClientCommand::ReconnectRouting { pending } = command {
            return self.handle_routing_reconnect(application, context, &pending, request_id);
        }
        let connecting = matches!(&command, ClientCommand::Connect { mode }
            if application.capabilities().supports(*mode))
            && self.connection != ConnectionStatus::Connected
            && !self.cleanup_required;
        let is_disconnect = matches!(command, ClientCommand::Disconnect);
        if connecting {
            self.connection = ConnectionStatus::Connecting;
            self.record(ClientEventKind::ConnectionStarting);
        }
        if is_disconnect {
            self.connection = ConnectionStatus::Disconnecting;
        }
        self.publish(application, context, true, None, Some(request_id));
        let event = command_event(&command);
        let subscription = command_subscription(&command).map(str::to_owned);
        let refresh = matches!(&command, ClientCommand::RefreshSubscription { .. });
        let configure_telemetry = matches!(&command, ClientCommand::SetTelemetrySettings { .. });
        let cancel = Cancellation::new(context.cancellation.clone(), request_id.0);
        let started = Instant::now();
        let result = if self.cleanup_required && needs_clean_connection(&command, application) {
            Err(ClientError::DisconnectFirst)
        } else {
            execute(application, command, &cancel)
        };
        let result = if connecting {
            self.finish_connect(application, result)
        } else {
            if is_disconnect {
                self.finish_disconnect(result);
            }
            result
        };
        let event = final_command_event(
            application,
            event,
            subscription.as_deref().filter(|_| refresh),
            result,
        );
        if configure_telemetry && result.is_ok() {
            self.telemetry
                .reconfigure(&application.catalog().settings.telemetry);
        }
        if let Some(uuid) = subscription {
            self.record_subscription_result(application, uuid, result);
        }
        if let Some(event) = event {
            self.telemetry
                .record_with_duration(event, Some(started.elapsed()));
        }
        self.publish(application, context, false, result.err(), Some(request_id));
        result
    }

    fn handle_routing_reconnect(
        &mut self,
        application: &mut ClientApplication,
        context: &WorkerContext,
        pending: &RoutingReconnect,
        request_id: RequestId,
    ) -> Result<(), ClientError> {
        let started = Instant::now();
        let cancel = Cancellation::new(context.cancellation.clone(), request_id.0);
        let result = if self.cleanup_required {
            Err(ClientError::DisconnectFirst)
        } else if self.connection != ConnectionStatus::Connected {
            Err(ClientError::InvalidInput)
        } else {
            application.reconnect_routing(pending, &cancel, |application| {
                self.connection = ConnectionStatus::Connecting;
                self.local_proxy = None;
                self.record(ClientEventKind::ConnectionStarting);
                self.publish(application, context, true, None, Some(request_id));
            })
        };
        let result = if self.connection == ConnectionStatus::Connecting {
            self.finish_connect(application, result)
        } else {
            result
        };
        let event = match result {
            Ok(()) => application.active_mode().map(ClientEventKind::Connected),
            Err(error) => Some(ClientEventKind::Failed(error)),
        };
        if let Some(event) = event {
            self.telemetry
                .record_with_duration(event, Some(started.elapsed()));
        }
        self.publish(application, context, false, result.err(), Some(request_id));
        result
    }

    fn record_subscription_result(
        &mut self,
        application: &ClientApplication,
        uuid: String,
        result: Result<(), ClientError>,
    ) {
        match result {
            Ok(()) => {
                self.subscription_errors.remove(&uuid);
            }
            Err(error)
                if application
                    .catalog()
                    .subscriptions
                    .iter()
                    .any(|item| item.subscription_uuid == uuid) =>
            {
                self.subscription_errors.insert(uuid, error);
            }
            Err(_) => {}
        }
    }

    fn finish_connect(
        &mut self,
        application: &mut ClientApplication,
        result: Result<(), ClientError>,
    ) -> Result<(), ClientError> {
        match (result, application.poll_engine()) {
            (Ok(()), Ok(EngineRuntime::Running(connection))) => {
                self.connection = ConnectionStatus::Connected;
                self.local_proxy = connection
                    .proxy_address()
                    .map(|address| address.to_string());
                Ok(())
            }
            (Err(ClientError::Cancelled), Ok(EngineRuntime::Stopped)) => {
                self.finish_disconnect(Ok(()));
                result
            }
            (_, Ok(EngineRuntime::Stopped)) => {
                self.connection = ConnectionStatus::Failed;
                result.and(Err(ClientError::EngineStopped))
            }
            (_, Err(error)) => {
                self.finish_disconnect(Err(error));
                Err(error)
            }
            (Err(error), Ok(EngineRuntime::Running(_))) => {
                self.finish_disconnect(Err(error));
                Err(error)
            }
        }
    }

    fn finish_disconnect(&mut self, result: Result<(), ClientError>) {
        self.cleanup_required = result.is_err();
        self.connection = if result.is_ok() {
            ConnectionStatus::Disconnected
        } else {
            ConnectionStatus::Failed
        };
        self.local_proxy = None;
    }

    fn check_engine(&mut self, application: &mut ClientApplication, context: &WorkerContext) {
        let error = match application.poll_engine() {
            Ok(EngineRuntime::Running(_)) => return,
            Ok(EngineRuntime::Stopped) => ClientError::EngineStopped,
            Err(error) => {
                self.cleanup_required = true;
                error
            }
        };
        self.connection = ConnectionStatus::Failed;
        self.local_proxy = None;
        self.record(ClientEventKind::Failed(error));
        self.publish(application, context, false, Some(error), None);
    }
}

fn provider_summary(item: &Subscription) -> ProviderSubscriptionSummary {
    let policy = &item.provider;
    ProviderSubscriptionSummary {
        subscription_uuid: item.subscription_uuid.clone(),
        raw_config_hidden: policy.raw_config_hidden(),
        has_committed_fallback: policy.fallback.is_some(),
        last_success_unix: policy.last_success_unix,
        last_update_used_fallback: policy.last_update_used_fallback,
        pending: policy
            .pending
            .iter()
            .map(|pending| ProviderChangePreview {
                kind: pending.kind,
                nonce: pending.nonce.clone(),
                from_origin: pending.from_origin.clone(),
                target_origin: pending.endpoint.origin.clone(),
            })
            .collect(),
        recognized_keys: policy.metadata.values.keys().copied().collect(),
        display: vpn_infrastructure::subscription_display(
            &policy.metadata,
            unix_seconds().unwrap_or_default(),
        ),
    }
}

fn execute(
    application: &mut ClientApplication,
    command: ClientCommand,
    cancel: &Cancellation,
) -> Result<(), ClientError> {
    match command {
        ClientCommand::AddSubscription { title, url } => {
            application.add_subscription(&title, &url, cancel)
        }
        ClientCommand::UpdateSubscription {
            subscription_uuid,
            title,
            url,
        } => application.update_subscription(&subscription_uuid, &title, &url, cancel),
        ClientCommand::RenameSubscription {
            subscription_uuid,
            title,
        } => application.rename_subscription(&subscription_uuid, &title, cancel),
        ClientCommand::RefreshSubscription { subscription_uuid } => {
            application.refresh_subscription(&subscription_uuid, cancel)
        }
        ClientCommand::ApproveProviderChange {
            subscription_uuid,
            nonce,
        } => application.resolve_provider_change(&subscription_uuid, &nonce, true, cancel),
        ClientCommand::RejectProviderChange {
            subscription_uuid,
            nonce,
        } => application.resolve_provider_change(&subscription_uuid, &nonce, false, cancel),
        ClientCommand::SetSubscriptionFallback {
            subscription_uuid,
            url,
        } => application.set_subscription_fallback(&subscription_uuid, url.as_deref(), cancel),
        ClientCommand::RemoveSubscription { subscription_uuid } => {
            application.remove_subscription(&subscription_uuid)
        }
        ClientCommand::ImportText { title, text } => {
            application.import_text(&title, text.as_bytes())
        }
        ClientCommand::SelectProfile { profile_uuid } => application.select_profile(&profile_uuid),
        ClientCommand::ProbeProfile { profile_uuid } => {
            application.probe_profile(&profile_uuid, cancel)
        }
        ClientCommand::ProbeProfiles { subscription_uuid } => {
            application.probe_profiles(subscription_uuid.as_deref(), cancel)
        }
        ClientCommand::RemoveProfile { profile_uuid } => application.remove_profile(&profile_uuid),
        ClientCommand::Connect { mode } => application.connect(mode, cancel),
        ClientCommand::ReconnectRouting { pending } => {
            application.reconnect_routing(&pending, cancel, |_| {})
        }
        ClientCommand::Disconnect => application.disconnect(),
        ClientCommand::SetLanguage { language } => application.set_language(language),
        ClientCommand::SetConnectionMode { mode } => application.set_connection_mode(mode),
        ClientCommand::SetLocalProxyPort { port } => application.set_local_proxy_port(port),
        ClientCommand::SetSubscriptionHwid { enabled } => {
            application.set_subscription_hwid(enabled)
        }
        ClientCommand::SetSubscriptionUserAgent { value } => {
            application.set_subscription_user_agent(value)
        }
        ClientCommand::SetTelemetrySettings {
            mode,
            endpoint,
            bearer,
        } => application.set_telemetry_settings(
            mode,
            endpoint,
            bearer,
            cancel,
            Telemetry::verify_manual_settings,
        ),
        ClientCommand::SaveRoutingProfile { uuid, profile } => {
            application.save_routing_profile(uuid, profile, cancel)
        }
        ClientCommand::RemoveRoutingProfile { profile_uuid } => {
            application.remove_routing_profile(&profile_uuid)
        }
        ClientCommand::SetRoutingAssignment {
            subscription_uuid,
            assignment,
        } => application.set_routing_assignment(subscription_uuid, assignment),
        ClientCommand::SetDnsPolicy {
            profile_uuid,
            policy,
        } => application.set_dns_policy(profile_uuid, policy, cancel),
    }
}

fn command_subscription(command: &ClientCommand) -> Option<&str> {
    match command {
        ClientCommand::UpdateSubscription {
            subscription_uuid, ..
        }
        | ClientCommand::RefreshSubscription { subscription_uuid }
        | ClientCommand::ApproveProviderChange {
            subscription_uuid, ..
        }
        | ClientCommand::RejectProviderChange {
            subscription_uuid, ..
        }
        | ClientCommand::SetSubscriptionFallback {
            subscription_uuid, ..
        }
        | ClientCommand::RemoveSubscription { subscription_uuid } => Some(subscription_uuid),
        _ => None,
    }
}

fn refresh_or_removal(command: &ClientCommand) -> Option<&str> {
    match command {
        ClientCommand::UpdateSubscription {
            subscription_uuid, ..
        }
        | ClientCommand::RefreshSubscription { subscription_uuid }
        | ClientCommand::RemoveSubscription { subscription_uuid } => Some(subscription_uuid),
        _ => None,
    }
}

fn needs_clean_connection(command: &ClientCommand, application: &ClientApplication) -> bool {
    matches!(
        command,
        ClientCommand::ProbeProfile { .. } | ClientCommand::ProbeProfiles { .. }
    ) || matches!(command, ClientCommand::Connect { mode } if application.capabilities().supports(*mode))
}

fn refresh_event(
    application: &ClientApplication,
    subscription_uuid: &str,
    result: Result<(), ClientError>,
) -> ClientEventKind {
    if let Err(error) = result {
        return ClientEventKind::SubscriptionFailed(error);
    }
    let used_fallback = application
        .catalog()
        .subscriptions
        .iter()
        .any(|subscription| {
            subscription.subscription_uuid == subscription_uuid
                && subscription.provider.last_update_used_fallback
        });
    if used_fallback {
        ClientEventKind::SubscriptionFallbackUsed
    } else {
        ClientEventKind::SubscriptionUpdated
    }
}

fn final_command_event(
    application: &ClientApplication,
    event: Option<ClientEventKind>,
    refreshing_uuid: Option<&str>,
    result: Result<(), ClientError>,
) -> Option<ClientEventKind> {
    if let Some(uuid) = refreshing_uuid {
        return Some(refresh_event(application, uuid, result));
    }
    match (result, event) {
        (Ok(()), event) => event,
        (Err(error), Some(ClientEventKind::SubscriptionUpdated)) => {
            Some(ClientEventKind::SubscriptionFailed(error))
        }
        (
            Err(error),
            Some(ClientEventKind::SubscriptionSettingsSaved | ClientEventKind::SubscriptionRemoved),
        ) => Some(ClientEventKind::SubscriptionSettingsFailed(error)),
        (Err(error), _) => Some(ClientEventKind::Failed(error)),
    }
}

fn command_event(command: &ClientCommand) -> Option<ClientEventKind> {
    let event = match command {
        ClientCommand::AddSubscription { .. }
        | ClientCommand::UpdateSubscription { .. }
        | ClientCommand::RefreshSubscription { .. } => ClientEventKind::SubscriptionUpdated,
        ClientCommand::RenameSubscription { .. }
        | ClientCommand::ApproveProviderChange { .. }
        | ClientCommand::RejectProviderChange { .. }
        | ClientCommand::SetSubscriptionFallback { .. } => {
            ClientEventKind::SubscriptionSettingsSaved
        }
        ClientCommand::RemoveSubscription { .. } => ClientEventKind::SubscriptionRemoved,
        ClientCommand::ImportText { .. } => ClientEventKind::ProfilesImported,
        ClientCommand::SelectProfile { .. } => ClientEventKind::ProfileSelected,
        ClientCommand::ProbeProfile { .. }
        | ClientCommand::ProbeProfiles { .. }
        | ClientCommand::ReconnectRouting { .. } => return None,
        ClientCommand::RemoveProfile { .. } => ClientEventKind::ProfileRemoved,
        ClientCommand::Connect { mode } => ClientEventKind::Connected(*mode),
        ClientCommand::Disconnect => ClientEventKind::Disconnected,
        ClientCommand::SetLanguage { .. }
        | ClientCommand::SetConnectionMode { .. }
        | ClientCommand::SetLocalProxyPort { .. }
        | ClientCommand::SetSubscriptionHwid { .. }
        | ClientCommand::SetSubscriptionUserAgent { .. }
        | ClientCommand::SetTelemetrySettings { .. }
        | ClientCommand::SaveRoutingProfile { .. }
        | ClientCommand::RemoveRoutingProfile { .. }
        | ClientCommand::SetRoutingAssignment { .. }
        | ClientCommand::SetDnsPolicy { .. } => ClientEventKind::SettingsSaved,
    };
    Some(event)
}

impl ClientSession for ClientHandle {
    fn snapshot(&self) -> ClientSnapshot {
        ClientHandle::snapshot(self)
    }

    fn send(&self, command: ClientCommand) -> Result<RequestId, SubmitError> {
        ClientHandle::send(self, command)
    }

    fn shutdown(&mut self) -> Result<(), ClientError> {
        ClientHandle::shutdown(self)
    }
}

#[cfg(windows)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_modes_leave_the_selection_and_runtime_untouched() {
        use vpn_infrastructure::client_store::UserSecretProtection;

        let data_dir = std::env::temp_dir().join(format!(
            "blazer-mode-test-{}",
            vpn_infrastructure::new_profile_uuid()
        ));
        let proxy_engine = BlazeCoreProcess::new(
            TrustedEngineSpec {
                source_path: data_dir.join("unused-blazecore.exe"),
                expected_sha256: APPROVED_ENGINE_SHA256.into(),
                expected_version: APPROVED_ENGINE_VERSION.into(),
            },
            data_dir.clone(),
            0,
        );
        let mut client = ClientHandle::spawn_with_platform(
            data_dir.clone(),
            Box::new(UserSecretProtection),
            Box::new(proxy_engine),
            SubscriptionRefreshMode::Manual,
        )
        .expect("start isolated backend");
        let (update, receiver) = mpsc::sync_channel(1);
        client.set_notifier(Arc::new(move || {
            let _ = update.try_send(());
        }));
        for command in [
            ClientCommand::SetConnectionMode {
                mode: ConnectionMode::SystemProxy,
            },
            ClientCommand::Connect {
                mode: ConnectionMode::Tun,
            },
        ] {
            let request = client.send(command).expect("enqueue unsupported mode");
            let snapshot = loop {
                let snapshot = client.snapshot();
                if snapshot.completed_request == Some(request) {
                    break snapshot;
                }
                receiver
                    .recv_timeout(Duration::from_secs(10))
                    .expect("complete the request");
            };
            assert_eq!(snapshot.last_error, Some(ClientError::UnsupportedMode));
            assert_eq!(snapshot.selected_mode, ConnectionMode::LocalProxy);
            assert_eq!(snapshot.active_mode, None);
            assert_eq!(snapshot.connection, ConnectionStatus::Disconnected);
            assert!(snapshot.local_proxy.is_none() && snapshot.http_proxy.is_none());
            assert!(!snapshot.mode_capabilities.system_proxy);
            assert!(!snapshot.mode_capabilities.tun);
            assert!(!snapshot.cleanup_required);
        }
        client.shutdown().expect("shutdown isolated backend");
        assert!(!data_dir.join("catalog.sealed").exists());
        std::fs::remove_file(data_dir.join("catalog.lock")).expect("remove isolated lock");
        std::fs::remove_file(data_dir.join("subscription-device-id"))
            .expect("remove isolated subscription HWID");
        std::fs::remove_dir(&data_dir).expect("remove isolated catalog directory");
    }
}

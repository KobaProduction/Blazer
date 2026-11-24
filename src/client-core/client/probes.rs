use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{
    ClientError, ProbeBatchProgress, ProbeCompletion, ProfileProbeState, ProfileProbeStatus,
    RequestId, WorkerContext,
};
use vpn_application::client::{
    Cancellation, CancellationSignal, ClientApplication, PreparedProfileProbe, ProbeRevision,
};

const LIMIT: usize = vpn_application::client::MAX_CONCURRENT_PROFILE_PROBES;

struct Queued {
    token: u64,
    prepared: PreparedProfileProbe,
}
struct Running {
    token: u64,
    uuid: String,
    method: super::ProbeMethod,
    exclusive: bool,
    revision: ProbeRevision,
    signal: Arc<CancellationSignal>,
    cancelled: bool,
    thread: JoinHandle<Result<Duration, ClientError>>,
}
struct Batch {
    progress: ProbeBatchProgress,
    pending: HashSet<u64>,
    bulk: bool,
    cancelled: usize,
    single_error: Option<ClientError>,
}

impl Batch {
    fn complete(&mut self, token: u64, error: Option<ClientError>) {
        if !self.pending.remove(&token) {
            return;
        }
        self.progress.completed += 1;
        self.cancelled += usize::from(error == Some(ClientError::Cancelled));
        if !self.bulk {
            self.single_error =
                error.filter(|error| !matches!(error, ClientError::Network | ClientError::Timeout));
        }
    }
}

#[derive(Default)]
pub(super) struct ProbeQueue {
    next: u64,
    queued: VecDeque<Queued>,
    running: Vec<Running>,
    latest: HashMap<String, u64>,
    statuses: HashMap<String, ProfileProbeStatus>,
    batches: Vec<Batch>,
    completions: VecDeque<ProbeCompletion>,
}

impl ProbeQueue {
    pub(super) fn active(&self) -> bool {
        !self.queued.is_empty() || !self.running.is_empty()
    }

    pub(super) fn submit(
        &mut self,
        jobs: Vec<PreparedProfileProbe>,
        request: RequestId,
        subscription: Option<String>,
        bulk: bool,
    ) {
        if self.batches.len() == 32 {
            self.reject(request, ClientError::Timeout);
            return;
        }
        let pending = jobs
            .into_iter()
            .map(|job| self.enqueue(job, request))
            .collect::<HashSet<_>>();
        self.batches.push(Batch {
            progress: ProbeBatchProgress {
                request_id: request,
                subscription_uuid: subscription,
                total: pending.len(),
                completed: 0,
            },
            pending,
            bulk,
            cancelled: 0,
            single_error: None,
        });
    }

    pub(super) fn reject(&mut self, request: RequestId, error: ClientError) {
        self.completed(ProbeCompletion {
            request_id: request,
            error: Some(error),
        });
    }

    fn enqueue(&mut self, prepared: PreparedProfileProbe, request: RequestId) -> u64 {
        if let Some(token) = self.equivalent(&prepared) {
            return token;
        }
        for job in &mut self.running {
            if job.uuid == prepared.profile_uuid {
                job.signal.cancel(1);
                job.cancelled = true;
            }
        }
        self.next += 1;
        let token = self.next;
        self.latest.insert(prepared.profile_uuid.clone(), token);
        self.statuses.insert(
            prepared.profile_uuid.clone(),
            ProfileProbeStatus {
                request_id: request,
                method: prepared.method,
                state: ProfileProbeState::Queued,
                error: None,
            },
        );
        self.queued.push_back(Queued { token, prepared });
        token
    }

    fn equivalent(&self, prepared: &PreparedProfileProbe) -> Option<u64> {
        self.queued
            .iter()
            .find(|job| {
                job.prepared.profile_uuid == prepared.profile_uuid
                    && job.prepared.revision == prepared.revision
            })
            .map(|job| job.token)
            .or_else(|| {
                self.running
                    .iter()
                    .find(|job| {
                        !job.cancelled
                            && job.uuid == prepared.profile_uuid
                            && job.revision == prepared.revision
                    })
                    .map(|job| job.token)
            })
    }

    pub(super) fn poll(&mut self, application: &mut ClientApplication, context: &WorkerContext) {
        self.statuses.retain(|uuid, _| {
            self.latest.contains_key(uuid) || application.profile_latencies().contains_key(uuid)
        });
        self.cancel_stale(application);
        self.collect(application);
        self.start(application);
        self.finish_batches();
        self.publish(application, context);
    }

    fn cancel_stale(&mut self, application: &mut ClientApplication) {
        for job in &mut self.running {
            if !job.cancelled && !application.profile_probe_revision_is_current(&job.revision) {
                job.signal.cancel(1);
                job.cancelled = true;
            }
        }
        let mut index = 0;
        while index < self.queued.len() {
            let prepared = &self.queued[index].prepared;
            if application.profile_probe_revision_is_current(&prepared.revision) {
                index += 1;
            } else {
                let Queued { token, prepared } = self.queued.remove(index).expect("queued probe");
                self.finish(
                    application,
                    token,
                    &prepared.profile_uuid,
                    prepared.method,
                    &prepared.revision,
                    Err(ClientError::Cancelled),
                );
            }
        }
    }

    fn collect(&mut self, application: &mut ClientApplication) {
        let mut index = 0;
        while index < self.running.len() {
            if !self.running[index].thread.is_finished() {
                index += 1;
                continue;
            }
            let job = self.running.swap_remove(index);
            let result = job.thread.join().unwrap_or(Err(ClientError::EngineStopped));
            let result = if job.cancelled {
                Err(ClientError::Cancelled)
            } else {
                result
            };
            self.finish(
                application,
                job.token,
                &job.uuid,
                job.method,
                &job.revision,
                result,
            );
        }
    }

    fn start(&mut self, application: &mut ClientApplication) {
        while self.running.len() < LIMIT {
            let exclusive = self.running.iter().any(|job| job.exclusive);
            let Some(index) = self
                .queued
                .iter()
                .position(|job| !job.prepared.exclusive || !exclusive)
            else {
                break;
            };
            let Queued { token, prepared } =
                self.queued.remove(index).expect("selected queue item");
            self.start_one(application, token, prepared);
        }
    }

    fn start_one(
        &mut self,
        application: &mut ClientApplication,
        token: u64,
        prepared: PreparedProfileProbe,
    ) {
        let PreparedProfileProbe {
            profile_uuid,
            method,
            exclusive,
            revision,
            task,
        } = prepared;
        let task = match task {
            Ok(task) if application.profile_probe_revision_is_current(&revision) => task,
            other => {
                let error = other.err().unwrap_or(ClientError::Cancelled);
                self.finish(
                    application,
                    token,
                    &profile_uuid,
                    method,
                    &revision,
                    Err(error),
                );
                return;
            }
        };
        let signal = Arc::new(CancellationSignal::default());
        let cancel = Cancellation::new(signal.clone(), 1);
        let spawned = std::thread::Builder::new()
            .name("blazer-profile-probe".into())
            .spawn(move || task(&cancel));
        match spawned {
            Ok(thread) => {
                if self.latest.get(&profile_uuid) == Some(&token)
                    && let Some(status) = self.statuses.get_mut(&profile_uuid)
                {
                    status.state = ProfileProbeState::Running;
                }
                self.running.push(Running {
                    token,
                    uuid: profile_uuid,
                    method,
                    exclusive,
                    revision,
                    signal,
                    cancelled: false,
                    thread,
                });
            }
            Err(_) => self.finish(
                application,
                token,
                &profile_uuid,
                method,
                &revision,
                Err(ClientError::StorageUnavailable),
            ),
        }
    }

    fn finish(
        &mut self,
        application: &mut ClientApplication,
        token: u64,
        uuid: &str,
        method: super::ProbeMethod,
        revision: &ProbeRevision,
        result: Result<Duration, ClientError>,
    ) {
        let cancelled = result == Err(ClientError::Cancelled);
        if self.latest.get(uuid) == Some(&token) {
            if !cancelled
                && application
                    .accept_profile_probe(revision, result, method)
                    .is_some()
            {
                if let Some(status) = self.statuses.get_mut(uuid) {
                    status.state = ProfileProbeState::Finished;
                    status.error = result.err();
                }
            } else {
                self.statuses.remove(uuid);
            }
            self.latest.remove(uuid);
        }
        for batch in &mut self.batches {
            batch.complete(token, result.err());
        }
    }

    fn finish_batches(&mut self) {
        let mut index = 0;
        while index < self.batches.len() {
            if !self.batches[index].pending.is_empty() {
                index += 1;
                continue;
            }
            let batch = self.batches.remove(index);
            let error = batch.single_error.or_else(|| {
                (batch.cancelled == batch.progress.total).then_some(ClientError::Cancelled)
            });
            self.completed(ProbeCompletion {
                request_id: batch.progress.request_id,
                error,
            });
        }
    }

    fn completed(&mut self, completion: ProbeCompletion) {
        if self.completions.len() == 32 {
            self.completions.pop_front();
        }
        self.completions.push_back(completion);
    }

    pub(super) fn cancel_and_wait(
        &mut self,
        application: &mut ClientApplication,
        context: &WorkerContext,
    ) -> Result<(), ClientError> {
        for job in &mut self.running {
            job.signal.cancel(1);
            job.cancelled = true;
        }
        while let Some(Queued { token, prepared }) = self.queued.pop_front() {
            self.finish(
                application,
                token,
                &prepared.profile_uuid,
                prepared.method,
                &prepared.revision,
                Err(ClientError::Cancelled),
            );
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !self.running.is_empty() {
            self.collect(application);
            if Instant::now() >= deadline {
                self.finish_batches();
                self.publish(application, context);
                return Err(ClientError::ShutdownTimeout);
            }
            if !self.running.is_empty() {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        self.finish_batches();
        self.publish(application, context);
        Ok(())
    }

    fn publish(&self, application: &ClientApplication, context: &WorkerContext) {
        let bulk_progress = || {
            self.batches
                .iter()
                .filter(|batch| batch.bulk)
                .map(|batch| &batch.progress)
        };
        context.publish_if(|snapshot| {
            if snapshot.profile_probes == self.statuses
                && snapshot.profile_latencies == *application.profile_latencies()
                && snapshot.probe_batches.iter().eq(bulk_progress())
                && snapshot.probe_completions.iter().eq(&self.completions)
            {
                return false;
            }
            snapshot.profile_probes.clone_from(&self.statuses);
            snapshot.probe_batches = bulk_progress().cloned().collect();
            snapshot.probe_completions = self.completions.iter().copied().collect();
            snapshot
                .profile_latencies
                .clone_from(application.profile_latencies());
            true
        });
    }
}

//! Translates actions from widgets to the backend and retains the latest snapshot.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::localization::Key;
use crate::model::{ErrorNotice, Intent, Removal};
use crate::session::{
    ClientCommand, ClientError, ClientSession, ClientSnapshot, RequestId, SubmitError,
};

pub struct Controller {
    client: Box<dyn ClientSession>,
    snapshot: ClientSnapshot,
    submission_error: Option<ErrorNotice>,
    pending_mode: Option<RequestId>,
    dirty: Arc<AtomicBool>,
    #[cfg(debug_assertions)]
    preview: bool,
}

impl Controller {
    pub fn new(client: impl ClientSession + 'static, dirty: Arc<AtomicBool>) -> Self {
        Self {
            snapshot: client.snapshot(),
            client: Box::new(client),
            submission_error: None,
            pending_mode: None,
            dirty,
            #[cfg(debug_assertions)]
            preview: false,
        }
    }

    pub fn snapshot(&self) -> &ClientSnapshot {
        &self.snapshot
    }

    pub fn is_saving_mode(&self) -> bool {
        self.pending_mode.is_some()
    }

    pub fn error(&self) -> Option<ErrorNotice> {
        self.submission_error
            .or(self.snapshot.last_error.map(ErrorNotice::Client))
    }

    pub fn submission_error(&self) -> Option<ErrorNotice> {
        self.submission_error
    }

    #[cfg(debug_assertions)]
    pub fn preview(&mut self, snapshot: ClientSnapshot) {
        self.preview = true;
        self.snapshot = snapshot;
    }

    pub fn refresh(&mut self) {
        #[cfg(debug_assertions)]
        if self.preview {
            return;
        }
        if !self.dirty.swap(false, Ordering::AcqRel) {
            return;
        }
        let next = self.client.snapshot();
        if self
            .pending_mode
            .zip(next.completed_request)
            .is_some_and(|(pending, done)| done.0 >= pending.0)
        {
            self.pending_mode = None;
        }
        self.snapshot = next;
    }

    pub fn dispatch(&mut self, intent: Intent) -> bool {
        self.submit(intent).is_some()
    }

    pub fn submit(&mut self, intent: Intent) -> Option<RequestId> {
        let save_mode = matches!(&intent, Intent::ChangeMode(_));
        let command = backend_command(intent);
        self.submission_error = match self.client.send(command) {
            Ok(request) => {
                if save_mode {
                    self.pending_mode = Some(request);
                }
                self.submission_error = None;
                return Some(request);
            }
            Err(SubmitError::Busy) => Some(ErrorNotice::Local(Key::Busy)),
            Err(SubmitError::Stopped) => Some(ErrorNotice::Local(Key::BackendStopped)),
            Err(SubmitError::Rejected(error)) => Some(ErrorNotice::Client(error)),
        };
        None
    }

    pub fn shutdown(&mut self) -> Result<(), ClientError> {
        self.client.shutdown()
    }
}

fn backend_command(intent: Intent) -> ClientCommand {
    match intent {
        Intent::Connect(mode) => ClientCommand::Connect { mode },
        Intent::Disconnect => ClientCommand::Disconnect,
        Intent::ChangeMode(mode) => ClientCommand::SetConnectionMode { mode },
        Intent::SetLocalProxyPort(port) => ClientCommand::SetLocalProxyPort { port },
        Intent::SetProbeSettings(settings) => ClientCommand::SetProbeSettings { settings },
        Intent::SetSubscriptionHwid(enabled) => ClientCommand::SetSubscriptionHwid { enabled },
        Intent::SetSubscriptionUserAgent(value) => {
            ClientCommand::SetSubscriptionUserAgent { value }
        }
        Intent::SetTelemetrySettings {
            mode,
            endpoint,
            bearer,
        } => ClientCommand::SetTelemetrySettings {
            mode,
            endpoint,
            bearer,
        },
        Intent::ChangeLanguage(language) => ClientCommand::SetLanguage { language },
        Intent::SelectProfile(profile_uuid) => ClientCommand::SelectProfile { profile_uuid },
        Intent::ProbeProfile(profile_uuid) => ClientCommand::ProbeProfile { profile_uuid },
        Intent::ProbeProfiles(subscription_uuid) => {
            ClientCommand::ProbeProfiles { subscription_uuid }
        }
        Intent::SaveRoutingProfile { uuid, profile } => {
            ClientCommand::SaveRoutingProfile { uuid, profile }
        }
        Intent::RemoveRoutingProfile { profile_uuid } => {
            ClientCommand::RemoveRoutingProfile { profile_uuid }
        }
        Intent::SetRoutingAssignment {
            subscription_uuid,
            assignment,
        } => ClientCommand::SetRoutingAssignment {
            subscription_uuid,
            assignment,
        },
        Intent::SetDnsPolicy {
            profile_uuid,
            policy,
        } => ClientCommand::SetDnsPolicy {
            profile_uuid,
            policy,
        },
        Intent::ReconnectRouting(pending) => ClientCommand::ReconnectRouting { pending },
        Intent::ImportProfiles { title, text } => ClientCommand::ImportText { title, text },
        Intent::UpdateProfile { profile_uuid, text } => {
            ClientCommand::UpdateProfile { profile_uuid, text }
        }
        Intent::SaveSubscription {
            subscription_uuid: Some(subscription_uuid),
            title,
            url,
        } if url.trim().is_empty() => ClientCommand::RenameSubscription {
            subscription_uuid,
            title,
        },
        Intent::SaveSubscription {
            subscription_uuid: Some(subscription_uuid),
            title,
            url,
        } => ClientCommand::UpdateSubscription {
            subscription_uuid,
            title,
            url,
        },
        Intent::SaveSubscription {
            subscription_uuid: None,
            title,
            url,
        } => ClientCommand::AddSubscription { title, url },
        Intent::RefreshSubscription(subscription_uuid) => {
            ClientCommand::RefreshSubscription { subscription_uuid }
        }
        Intent::SetFallback {
            subscription_uuid,
            url,
        } => ClientCommand::SetSubscriptionFallback {
            subscription_uuid,
            url,
        },
        Intent::ApproveProviderChange {
            subscription_uuid,
            nonce,
        } => ClientCommand::ApproveProviderChange {
            subscription_uuid,
            nonce,
        },
        Intent::RejectProviderChange {
            subscription_uuid,
            nonce,
        } => ClientCommand::RejectProviderChange {
            subscription_uuid,
            nonce,
        },
        Intent::ConfirmRemoval(Removal::Subscription(subscription_uuid)) => {
            ClientCommand::RemoveSubscription { subscription_uuid }
        }
        Intent::ConfirmRemoval(Removal::Profile(profile_uuid)) => {
            ClientCommand::RemoveProfile { profile_uuid }
        }
    }
}

#[cfg(test)]
mod probe_settings_tests {
    use super::*;
    use crate::session::{ProbeHttpMethod, ProbeMethod, ProbeSettings, ProbeStrategy};

    #[test]
    fn saving_probe_settings_sends_all_fields_to_the_shared_client_core() {
        let settings = ProbeSettings {
            method: ProbeMethod::Proxy,
            http_method: ProbeHttpMethod::Head,
            url: "https://probe.example/check".into(),
            timeout_ms: 700,
            attempts: 4,
            warmup: false,
            strategy: ProbeStrategy::Median,
        };
        let command = backend_command(Intent::SetProbeSettings(settings.clone()));
        assert!(
            matches!(command, ClientCommand::SetProbeSettings { settings: saved } if saved == settings)
        );
    }
}

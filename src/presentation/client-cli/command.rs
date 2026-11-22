use std::io::Read;

use client_core::{
    ClientCommand, ClientError, ConnectionMode, DnsPolicy, Language, MAX_SUBSCRIPTION_BYTES,
    ProbeHttpMethod, ProbeMethod, ProbeSettings, ProbeStrategy, RouteAction, RouteRule,
    RouteTarget, RoutingAssignment, RoutingCatalog, RoutingInput,
};

const MAX_SUBSCRIPTION_URL_BYTES: usize = 8192;

pub enum Action {
    Status,
    Profiles,
    Subscriptions,
    Settings,
    PingSettings,
    Diagnostics,
    Routing(Option<String>),
    Serve(ConnectionMode),
    Execute(Operation),
}

pub enum Operation {
    Import,
    AddSubscription(String),
    EditSubscription { uuid: String, title: String },
    SetFallback(String),
    Probe(String),
    ProbeProfiles(Option<String>),
    SetProbeSettings(ProbeSettings),
    SaveRoutingProfile { uuid: Option<String>, title: String },
    CopyRoutingProfile { source_uuid: String, title: String },
    Submit(ClientCommand),
}

pub enum Completion {
    Imported,
    Probe(String),
    ProbeProfiles,
    RoutingSaved,
    Updated,
}

impl Action {
    pub fn parse(args: &[&str]) -> Result<Option<Self>, ()> {
        let action = match args {
            [] | ["-h" | "--help"] => return Ok(None),
            ["status"] => Self::Status,
            ["profiles"] => Self::Profiles,
            ["profiles", "test"] => Self::Execute(Operation::ProbeProfiles(None)),
            ["subscriptions"] => Self::Subscriptions,
            ["settings"] => Self::Settings,
            ["ping-settings"] => Self::PingSettings,
            ["ping-settings", rest @ ..] => {
                Self::Execute(Operation::SetProbeSettings(parse_probe_settings(rest)?))
            }
            ["local-proxy-port", port] => {
                Self::Execute(Operation::Submit(ClientCommand::SetLocalProxyPort {
                    port: port.parse().map_err(|_| ())?,
                }))
            }
            ["diagnostics"] => Self::Diagnostics,
            ["routing"] => Self::Routing(None),
            ["routing", "show", uuid] => Self::Routing(Some((*uuid).into())),
            ["routing", rest @ ..] => Self::Execute(routing_command(rest)?),
            ["proxy"] => Self::Serve(ConnectionMode::LocalProxy),
            ["tun"] => Self::Serve(ConnectionMode::Tun),
            ["system-proxy"] => Self::Serve(ConnectionMode::SystemProxy),
            ["import"] => Self::Execute(Operation::Import),
            ["profile", action, uuid] => Self::Execute(profile_command(action, uuid)?),
            ["subscription", rest @ ..] => Self::Execute(subscription_command(rest)?),
            ["language", language] => {
                Self::Execute(Operation::Submit(ClientCommand::SetLanguage {
                    language: parse_language(language)?,
                }))
            }
            ["mode", mode] => Self::Execute(Operation::Submit(ClientCommand::SetConnectionMode {
                mode: parse_mode(mode)?,
            })),
            _ => return Err(()),
        };
        Ok(Some(action))
    }
}

fn profile_command(action: &str, uuid: &str) -> Result<Operation, ()> {
    let profile_uuid = uuid.to_owned();
    match action {
        "select" => Ok(Operation::Submit(ClientCommand::SelectProfile {
            profile_uuid,
        })),
        "remove" => Ok(Operation::Submit(ClientCommand::RemoveProfile {
            profile_uuid,
        })),
        "test" => Ok(Operation::Probe(profile_uuid)),
        _ => Err(()),
    }
}

fn parse_probe_settings(args: &[&str]) -> Result<ProbeSettings, ()> {
    let [method, http_method, timeout_ms, warmup, attempts, strategy] = args else {
        return Err(());
    };
    let settings = ProbeSettings {
        method: match *method {
            "auto" => ProbeMethod::Automatic,
            "tcp" => ProbeMethod::Tcp,
            "icmp" => ProbeMethod::Icmp,
            "proxy" => ProbeMethod::Proxy,
            _ => return Err(()),
        },
        http_method: match *http_method {
            "get" => ProbeHttpMethod::Get,
            "head" => ProbeHttpMethod::Head,
            _ => return Err(()),
        },
        timeout_ms: timeout_ms.parse().map_err(|_| ())?,
        warmup: match *warmup {
            "warmup" => true,
            "cold" => false,
            _ => return Err(()),
        },
        attempts: attempts.parse().map_err(|_| ())?,
        strategy: parse_probe_strategy(strategy)?,
        ..ProbeSettings::default()
    };
    settings.validate().map_err(|_| ())?;
    Ok(settings)
}

fn parse_probe_strategy(value: &str) -> Result<ProbeStrategy, ()> {
    match value {
        "last" => Ok(ProbeStrategy::Last),
        "min" => Ok(ProbeStrategy::Minimum),
        "average" => Ok(ProbeStrategy::Average),
        "median" => Ok(ProbeStrategy::Median),
        _ => Err(()),
    }
}

fn subscription_command(args: &[&str]) -> Result<Operation, ()> {
    let command = match args {
        ["add", title] => return Ok(Operation::AddSubscription((*title).into())),
        ["edit", uuid, title] => {
            return Ok(Operation::EditSubscription {
                uuid: (*uuid).into(),
                title: (*title).into(),
            });
        }
        ["rename", uuid, title] => ClientCommand::RenameSubscription {
            subscription_uuid: (*uuid).into(),
            title: (*title).into(),
        },
        ["fallback", uuid] => return Ok(Operation::SetFallback((*uuid).into())),
        ["refresh", uuid] => ClientCommand::RefreshSubscription {
            subscription_uuid: (*uuid).into(),
        },
        ["test", uuid] => return Ok(Operation::ProbeProfiles(Some((*uuid).into()))),
        ["remove", uuid] => ClientCommand::RemoveSubscription {
            subscription_uuid: (*uuid).into(),
        },
        ["clear-fallback", uuid] => ClientCommand::SetSubscriptionFallback {
            subscription_uuid: (*uuid).into(),
            url: None,
        },
        ["approve", uuid, nonce] => ClientCommand::ApproveProviderChange {
            subscription_uuid: (*uuid).into(),
            nonce: (*nonce).into(),
        },
        ["reject", uuid, nonce] => ClientCommand::RejectProviderChange {
            subscription_uuid: (*uuid).into(),
            nonce: (*nonce).into(),
        },
        _ => return Err(()),
    };
    Ok(Operation::Submit(command))
}

fn parse_language(value: &str) -> Result<Language, ()> {
    match value {
        "ru" => Ok(Language::Russian),
        "en" => Ok(Language::English),
        _ => Err(()),
    }
}

fn routing_command(args: &[&str]) -> Result<Operation, ()> {
    let command = match args {
        ["add", title] => {
            return Ok(Operation::SaveRoutingProfile {
                uuid: None,
                title: (*title).into(),
            });
        }
        ["edit", uuid, title] => {
            return Ok(Operation::SaveRoutingProfile {
                uuid: Some((*uuid).into()),
                title: (*title).into(),
            });
        }
        ["copy", uuid, title] => {
            return Ok(Operation::CopyRoutingProfile {
                source_uuid: (*uuid).into(),
                title: (*title).into(),
            });
        }
        ["remove", uuid] => ClientCommand::RemoveRoutingProfile {
            profile_uuid: (*uuid).into(),
        },
        ["assign", scope, uuid, state] => {
            let enabled = match *state {
                "enabled" => true,
                "disabled" => false,
                _ => return Err(()),
            };
            ClientCommand::SetRoutingAssignment {
                subscription_uuid: (*scope != "global").then(|| (*scope).into()),
                assignment: RoutingAssignment {
                    profile_uuid: (*uuid != "none").then(|| (*uuid).into()),
                    enabled,
                    ignore_provider: false,
                },
            }
        }
        ["dns", uuid, policy @ ..] => ClientCommand::SetDnsPolicy {
            profile_uuid: (*uuid).into(),
            policy: parse_dns(policy)?,
        },
        _ => return Err(()),
    };
    Ok(Operation::Submit(command))
}

fn parse_dns(args: &[&str]) -> Result<DnsPolicy, ()> {
    match args {
        ["automatic"] => Ok(DnsPolicy::Automatic),
        ["system"] => Ok(DnsPolicy::System),
        ["tcp", endpoint] => Ok(DnsPolicy::Tcp {
            endpoint: endpoint.parse().map_err(|_| ())?,
        }),
        ["tls", endpoint, server_name] => Ok(DnsPolicy::Tls {
            endpoint: endpoint.parse().map_err(|_| ())?,
            server_name: (*server_name).into(),
        }),
        ["https", endpoint, server_name, path] => Ok(DnsPolicy::Https {
            endpoint: endpoint.parse().map_err(|_| ())?,
            server_name: (*server_name).into(),
            path: (*path).into(),
        }),
        _ => Err(()),
    }
}

fn read_routing(input: impl Read, title: String) -> Result<RoutingInput, ClientError> {
    let text = read_text(input, MAX_SUBSCRIPTION_BYTES)?;
    let mut dns = None;
    let mut rules = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let words: Vec<_> = line.split_whitespace().collect();
        match words.as_slice() {
            ["dns", policy @ ..] if dns.is_none() => {
                dns = Some(parse_dns(policy).map_err(|()| ClientError::InvalidInput)?);
            }
            [action, kind, value] => rules.push(parse_rule(action, kind, value)?),
            _ => return Err(ClientError::InvalidInput),
        }
    }
    Ok(RoutingInput {
        title,
        rules,
        dns: dns.unwrap_or(DnsPolicy::Automatic),
    })
}

fn parse_rule(action: &str, kind: &str, value: &str) -> Result<RouteRule, ClientError> {
    let action = match action {
        "proxy" => RouteAction::Proxy,
        "direct" => RouteAction::Direct,
        "block" => RouteAction::Block,
        _ => return Err(ClientError::InvalidInput),
    };
    let target = match kind {
        "domain" => RouteTarget::Domain(value.into()),
        "suffix" => RouteTarget::DomainSuffix(value.into()),
        "cidr" => RouteTarget::Cidr(value.into()),
        _ => return Err(ClientError::InvalidInput),
    };
    Ok(RouteRule { target, action })
}

fn parse_mode(value: &str) -> Result<ConnectionMode, ()> {
    match value {
        "proxy" => Ok(ConnectionMode::LocalProxy),
        "system-proxy" => Ok(ConnectionMode::SystemProxy),
        "tun" => Ok(ConnectionMode::Tun),
        _ => Err(()),
    }
}

impl Operation {
    pub fn request(
        self,
        stdin: impl Read,
        routing: &RoutingCatalog,
    ) -> Result<(ClientCommand, Completion), ClientError> {
        let command = match self {
            Self::Import => {
                return Ok((
                    ClientCommand::ImportText {
                        title: "Imported profile".to_owned(),
                        text: read_text(stdin, MAX_SUBSCRIPTION_BYTES)?,
                    },
                    Completion::Imported,
                ));
            }
            Self::AddSubscription(title) => ClientCommand::AddSubscription {
                title,
                url: read_url(stdin)?,
            },
            Self::EditSubscription { uuid, title } => ClientCommand::UpdateSubscription {
                subscription_uuid: uuid,
                title,
                url: read_url(stdin)?,
            },
            Self::SetFallback(uuid) => ClientCommand::SetSubscriptionFallback {
                subscription_uuid: uuid,
                url: Some(read_url(stdin)?),
            },
            Self::SetProbeSettings(mut settings) => {
                settings.url = read_url(stdin)?;
                settings.validate()?;
                ClientCommand::SetProbeSettings { settings }
            }
            Self::SaveRoutingProfile { uuid, title } => ClientCommand::SaveRoutingProfile {
                uuid,
                profile: read_routing(stdin, title)?,
            },
            Self::CopyRoutingProfile { source_uuid, title } => {
                let mut profile = routing
                    .lookup(&source_uuid)
                    .ok_or(ClientError::RoutingProfileMissing)?
                    .input
                    .clone();
                profile.title = title;
                ClientCommand::SaveRoutingProfile {
                    uuid: None,
                    profile,
                }
            }
            Self::Probe(profile_uuid) => {
                return Ok((
                    ClientCommand::ProbeProfile {
                        profile_uuid: profile_uuid.clone(),
                    },
                    Completion::Probe(profile_uuid),
                ));
            }
            Self::ProbeProfiles(subscription_uuid) => {
                return Ok((
                    ClientCommand::ProbeProfiles { subscription_uuid },
                    Completion::ProbeProfiles,
                ));
            }
            Self::Submit(command) => command,
        };
        let completion = if matches!(
            command,
            ClientCommand::SaveRoutingProfile { .. } | ClientCommand::SetDnsPolicy { .. }
        ) {
            Completion::RoutingSaved
        } else {
            Completion::Updated
        };
        Ok((command, completion))
    }
}

fn read_url(input: impl Read) -> Result<String, ClientError> {
    let incoming = read_text(input, MAX_SUBSCRIPTION_URL_BYTES)?;
    let url = incoming.trim();
    if url.is_empty() {
        return Err(ClientError::InvalidInput);
    }
    Ok(url.to_owned())
}

fn read_text(input: impl Read, limit: usize) -> Result<String, ClientError> {
    let mut bytes = Vec::new();
    input
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ClientError::InvalidInput)?;
    if bytes.len() > limit {
        return Err(ClientError::PayloadTooLarge);
    }
    String::from_utf8(bytes).map_err(|_| ClientError::InvalidInput)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_command(args: &[&str], input: &[u8]) -> Result<ProbeSettings, ClientError> {
        let Action::Execute(operation) = Action::parse(args).unwrap().unwrap() else {
            panic!("expected a settings operation");
        };
        let (command, _) = operation.request(input, &RoutingCatalog::default())?;
        let ClientCommand::SetProbeSettings { settings } = command else {
            panic!("expected the core probe-settings command");
        };
        Ok(settings)
    }

    #[test]
    fn ping_settings_reads_without_consuming_stdin_and_updates_all_preferences() {
        assert!(matches!(
            Action::parse(&["ping-settings"]),
            Ok(Some(Action::PingSettings))
        ));
        for (method, expected_method, strategy, expected_strategy) in [
            ("auto", ProbeMethod::Automatic, "last", ProbeStrategy::Last),
            ("tcp", ProbeMethod::Tcp, "min", ProbeStrategy::Minimum),
            ("icmp", ProbeMethod::Icmp, "average", ProbeStrategy::Average),
            ("proxy", ProbeMethod::Proxy, "median", ProbeStrategy::Median),
        ] {
            let settings = probe_command(
                &[
                    "ping-settings",
                    method,
                    "head",
                    "1200",
                    "cold",
                    "3",
                    strategy,
                ],
                b"https://connectivity.example/check?expected=204\r\n",
            )
            .unwrap();
            assert_eq!(settings.method, expected_method);
            assert_eq!(settings.http_method, ProbeHttpMethod::Head);
            assert_eq!(settings.timeout_ms, 1200);
            assert_eq!(settings.attempts, 3);
            assert!(!settings.warmup);
            assert!(settings.http_warmup_required());
            assert_eq!(
                settings.url,
                "https://connectivity.example/check?expected=204"
            );
            assert_eq!(settings.strategy, expected_strategy);
        }
        let settings = probe_command(
            &["ping-settings", "icmp", "get", "250", "warmup", "1", "last"],
            b"http://connectivity.example/check",
        )
        .unwrap();
        assert_eq!(settings.method, ProbeMethod::Icmp);
        assert_eq!(settings.http_method, ProbeHttpMethod::Get);
        assert!(settings.warmup);
    }

    #[test]
    fn ping_settings_rejects_invalid_syntax_bounds_and_stdin_urls() {
        for args in [
            [
                "ping-settings",
                "unknown",
                "get",
                "5000",
                "warmup",
                "1",
                "last",
            ],
            [
                "ping-settings",
                "auto",
                "post",
                "5000",
                "warmup",
                "1",
                "last",
            ],
            ["ping-settings", "auto", "get", "249", "warmup", "1", "last"],
            [
                "ping-settings",
                "auto",
                "get",
                "10001",
                "warmup",
                "1",
                "last",
            ],
            ["ping-settings", "auto", "get", "5000", "hot", "1", "last"],
            [
                "ping-settings",
                "auto",
                "get",
                "5000",
                "warmup",
                "0",
                "last",
            ],
            [
                "ping-settings",
                "auto",
                "get",
                "5000",
                "warmup",
                "6",
                "last",
            ],
            [
                "ping-settings",
                "auto",
                "get",
                "5000",
                "warmup",
                "1",
                "unknown",
            ],
        ] {
            assert!(Action::parse(&args).is_err());
        }
        for url in ["file:///private", "https://user:password@host/check"] {
            assert_eq!(
                probe_command(
                    &[
                        "ping-settings",
                        "auto",
                        "get",
                        "5000",
                        "warmup",
                        "1",
                        "last"
                    ],
                    url.as_bytes(),
                ),
                Err(ClientError::InvalidUrl)
            );
        }
    }
}

// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{collections::HashMap, sync::Arc};

use {
    axum::{
        extract::Path,
        http::StatusCode,
        routing::{get, patch, post},
        Json, Router,
    },
    log::{trace, warn},
    serde::{Deserialize, Serialize},
};

use crate::{
    cluster::Cluster,
    host::{FenceResult, Host, HostCommand, HostId},
    resource::{Resource, ResourceStatus},
    state::{Event, Record},
};

/// Main entrypoint for the command server.
///
/// This listens for commands on a unix socket and acts on them.
pub async fn server_main(
    listener: tokio::net::UnixListener,
    user_listener: Option<tokio::net::UnixListener>,
    cluster: Arc<Cluster>,
) {
    let server = Router::new()
        .route(
            "/status",
            get({
                let cluster = Arc::clone(&cluster);
                || get_status(cluster)
            }),
        )
        .route(
            "/resources/{id}",
            patch({
                let cluster = Arc::clone(&cluster);
                |path, payload| set_managed(path, payload, cluster)
            }),
        )
        .route(
            "/hosts/{id}",
            post({
                let cluster = Arc::clone(&cluster);
                |path, payload| host_post(path, payload, cluster)
            }),
        )
        .route(
            "/clear_events",
            post({
                let cluster = Arc::clone(&cluster);
                || clear_events(cluster)
            }),
        );

    if let Some(user_listener) = user_listener {
        let user_server = Router::new()
            .route(
                "/status",
                get({
                    let cluster = Arc::clone(&cluster);
                    || get_status(cluster)
                }),
            )
            .fallback(async || StatusCode::UNAUTHORIZED);
        tokio::spawn(async move {
            axum::serve(user_listener, user_server).await.unwrap();
        });
    };
    axum::serve(listener, server).await.unwrap();
}

async fn clear_events(cluster: Arc<Cluster>) -> Result<(), (StatusCode, &'static str)> {
    match cluster
        .write_record_nonblocking(Record::new(Event::Clear, "events".to_string(), None))
        .await
    {
        Ok(()) => Ok(()),
        Err(_) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to append record to statefile.",
        )),
    }
}

/// The representation of Cluster state that is communicated back to the admin using the status
/// command.
#[derive(Serialize, Deserialize, Debug)]
pub struct ClusterJson {
    pub resources: Vec<ResourceJson>,
    pub hosts: Vec<HostJson>,
    pub events: Vec<EventJson>,
}

/// The representation of Resource state that is communicated back to the admin using the status
/// command.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ResourceJson {
    pub id: String,
    pub kind: String,
    pub parameters: HashMap<String, String>,
    pub status: HashMap<String, StatusJson>,
    pub managed: bool,
    pub home_host: String,
    pub failover_host: Option<String>,
    pub dependents: Vec<String>,
    pub exclusive: bool,
}

enum StatusWhere {
    Nowhere,
    Home(Option<String>),
    Failover(Option<String>),
}

impl ResourceJson {
    fn build(
        res: &Resource,
        managed: bool,
        home_host: &HostId,
        failover_host: &Option<HostId>,
    ) -> Self {
        let status = res
            .status()
            .iter()
            .map(|(host, status)| {
                (
                    host.to_string(),
                    match status {
                        ResourceStatus::Running => StatusJson::build("Running", None),
                        ResourceStatus::Stopped => StatusJson::build("Stopped", None),
                        ResourceStatus::Unknown(reason) => {
                            StatusJson::build("Unknown", Some(reason))
                        }
                        ResourceStatus::Error(reason) => StatusJson::build("Error", Some(reason)),
                    },
                )
            })
            .collect();

        Self {
            id: res.id.to_string(),
            kind: res.kind.clone(),
            parameters: res.parameters.clone(),
            status,
            managed,
            home_host: home_host.to_string(),
            failover_host: failover_host.as_ref().map(|h| h.to_string()),
            dependents: res.dependents_names(),
            exclusive: res.count == 1,
        }
    }

    fn is_stopped_everywhere(&self) -> bool {
        self.status.values().all(|st| st.status == "Stopped")
    }

    fn has_status(&self, requested: &str) -> StatusWhere {
        let st = self.status.get(&self.home_host).unwrap();
        if st.status == requested {
            return StatusWhere::Home(st.comment.clone());
        }

        if let Some(failover_host) = &self.failover_host {
            let st = self.status.get(failover_host).unwrap();
            if st.status == requested {
                return StatusWhere::Failover(st.comment.clone());
            }
        }

        StatusWhere::Nowhere
    }

    /// Returns a single status value together with an optional comment.
    pub fn single_host_status(&self) -> (&'static str, Option<String>) {
        match self.has_status("Running") {
            StatusWhere::Home(_) => return ("Running", None),
            StatusWhere::Failover(_) => return ("Running (Failed Over)", None),
            _ => {}
        };

        if self.is_stopped_everywhere() {
            return ("Stopped", None);
        }

        for status in ["Error", "Unknown"] {
            match self.has_status(status) {
                StatusWhere::Home(comment) => return (status, comment),
                StatusWhere::Failover(comment) => return (status, comment),
                _ => {}
            }
        }

        ("Unexpected", None)
    }

    pub fn shared_host_status(&self) -> (String, Option<String>) {
        let running_on_hosts: Vec<_> = self
            .status
            .iter()
            .filter(|(_, st)| st.status == "Running")
            .map(|(host, _)| host)
            .collect();

        if !running_on_hosts.is_empty() {
            let mut hosts = "".to_owned();
            for host in running_on_hosts {
                hosts += &format!("{host},");
            }
            let hosts: nodeset::NodeSet = hosts.parse().expect("unable to parse nodeset.");

            return (format!("Running on {hosts}"), None);
        }

        if self.is_stopped_everywhere() {
            return ("Stopped".to_owned(), None);
        }

        for status in ["Error", "Unknown"] {
            match self.has_status(status) {
                StatusWhere::Home(comment) => return (status.to_owned(), comment),
                StatusWhere::Failover(comment) => return (status.to_owned(), comment),
                _ => {}
            }
        }

        ("Unexpected".to_owned(), None)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StatusJson {
    pub status: String,
    pub comment: Option<String>,
}

impl StatusJson {
    fn build(status: &str, reason: Option<&str>) -> Self {
        Self {
            status: status.to_owned(),
            comment: reason.map(|c| c.to_owned()),
        }
    }
}

/// Holds manager and admin issued events
#[derive(Serialize, Deserialize, Debug)]
pub struct EventJson {
    pub timestamp: String,
    pub event: String,
    pub obj_id: String,
    pub comment: Option<String>,
}

impl EventJson {
    fn build(rec: Record) -> Self {
        EventJson {
            timestamp: rec.timestamp.to_string(),
            event: rec.event.to_string(),
            obj_id: rec.obj_id,
            comment: rec.comment,
        }
    }

    pub fn syslog_print(&self) -> String {
        format!(
            "{} event={} object={} comment=\"{}\"",
            self.timestamp,
            self.event,
            self.obj_id,
            self.comment.as_deref().unwrap_or("")
        )
    }
}

/// The representation of Host state that is communicated back to the admin using the status
/// command.
#[derive(Serialize, Deserialize, Debug)]
pub struct HostJson {
    pub id: String,
    pub active: bool,
    pub connected: bool,
    pub fenced: bool,
}

impl HostJson {
    fn build(host: &Host) -> Self {
        Self {
            id: host.id().to_string(),
            active: host.active(),
            connected: host.connected(),
            fenced: host.fence_attempted(),
        }
    }
}

async fn get_status(cluster: Arc<Cluster>) -> Json<ClusterJson> {
    trace!("Manager handling GET /status request");
    let status = ClusterJson {
        resources: cluster
            .resource_groups()
            .flat_map(|rg| {
                let managed = rg.get_managed();
                let home_host = rg.home_node().id();
                let failover_host = rg.failover_node().map(|h| h.id());
                rg.resources()
                    .map(move |res| ResourceJson::build(res, managed, &home_host, &failover_host))
            })
            .collect(),

        hosts: cluster.hosts().map(|host| HostJson::build(host)).collect(),
        events: cluster
            .get_cluster_events()
            .into_iter()
            .map(EventJson::build)
            .collect(),
    };

    Json(status)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SetManagedArgs {
    pub managed: bool,
    pub comment: Option<String>,
}

async fn set_managed(
    Path(resource_id): Path<String>,
    Json(payload): Json<SetManagedArgs>,
    cluster: Arc<Cluster>,
) -> Result<(), (StatusCode, &'static str)> {
    if !cluster.args.manage_resources {
        return Err((
            StatusCode::CONFLICT,
            "The specified command can only be used when running in managed mode.",
        ));
    }

    for rg in cluster.resource_groups() {
        if rg.id() == resource_id.as_str() {
            warn!(
                "Resource group {}: setting managed={}",
                rg.id(),
                if payload.managed { "true" } else { "false" }
            );
            rg.update_managed(payload.managed).await;
            let event = if payload.managed {
                Event::Manage
            } else {
                Event::Unmanage
            };
            return match Arc::clone(&cluster)
                .write_record_nonblocking(Record::new(event, rg.id().to_string(), payload.comment))
                .await
            {
                Ok(()) => Ok(()),
                Err(_) => Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to append record to statefile.",
                )),
            };
        }
    }

    Err((StatusCode::NOT_FOUND, "Resource group not found."))
}

#[derive(Serialize, Deserialize, Debug)]
pub struct HostArgs {
    pub command: String,
    pub force: Option<bool>,
    pub comment: Option<String>,
}

async fn host_post(
    Path(host_id): Path<String>,
    Json(payload): Json<HostArgs>,
    cluster: Arc<Cluster>,
) -> Result<(), (StatusCode, &'static str)> {
    if !cluster.args.manage_resources {
        return Err((
            StatusCode::CONFLICT,
            "The specified command can only be used when running in managed mode.",
        ));
    }

    let Some(host) = cluster.get_host(&host_id) else {
        return Err((StatusCode::NOT_FOUND, ""));
    };

    // Check "reset" first because it doesn't require a partner host to be present in order to be a
    // valid command:
    if payload.command == "reset" {
        if !host.fence_attempted() {
            return Err((StatusCode::CONFLICT, "Host has not been fenced."));
        }

        host.set_fence_attempted(false);
        return match cluster
            .write_record_nonblocking(Record::new(
                Event::FenceReset,
                host.id().to_string(),
                payload.comment,
            ))
            .await
        {
            Ok(()) => Ok(()),
            Err(_) => Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to append record to statefile.",
            )),
        };
    }

    // The rest of the commands are only valid for hosts that have a partner:
    let Some(partner) = host.failover_partner() else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Host does not have a failover partner.",
        ));
    };

    match payload.command.as_str() {
        "failback" => {
            if !host.active() {
                return Err((
                    StatusCode::CONFLICT,
                    "Host is deactivated. Please activate it before running resources on it.",
                ));
            }

            partner.command(HostCommand::Failback).await;
        }
        "fence" => {
            if !partner.active() && payload.force != Some(true) {
                return Err((StatusCode::CONFLICT, "Partner is deactivated."));
            }
            if !partner.connected() && payload.force != Some(true) {
                return Err((StatusCode::CONFLICT, "Partner is disconnected."));
            }
            if host.fence_attempted() && payload.force != Some(true) {
                return Err((StatusCode::CONFLICT, "Host has already been fenced."));
            }
            return match host
                .submit_admin_fence_request_and_wait(payload.comment)
                .await
            {
                FenceResult::Success => Ok(()),
                FenceResult::AlreadyInProgress => Err((
                    StatusCode::CONFLICT,
                    "Another fence request is already in progress. Wait for it to finish.",
                )),
                FenceResult::PowerCommandFailed => Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Fence operation failed. The host is not powered off.",
                )),
                FenceResult::WritingStateRecordFailed => Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Fence operation succeeded, but a record was not appended to the state file.",
                )),
            };
        }
        "activate" => {
            let Some(_) = host.failover_partner() else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Host does not have a failover partner. Activate command can only be used in HA cluster.",
                ));
            };

            if host
                .update_activation_status(true, payload.comment, &cluster)
                .await
                .is_err()
            {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to append record to statefile.",
                ));
            };
        }
        "deactivate" => {
            if !partner.active() {
                return Err((
                    StatusCode::CONFLICT,
                    "Partner host is already deactivated. You cannot deactivate both hosts in a pair."
                ));
            }

            if host
                .update_activation_status(false, payload.comment, &cluster)
                .await
                .is_err()
            {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to append record to statefile.",
                ));
            };
        }
        _ => return Err((StatusCode::BAD_REQUEST, "Unsupported command.")),
    }

    Ok(())
}

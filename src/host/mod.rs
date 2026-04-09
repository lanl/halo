// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{
    fmt,
    future::Future,
    io,
    pin::Pin,
    rc::Rc,
    sync::{Arc, OnceLock},
};

use {
    futures::stream::FuturesUnordered,
    tokio::sync::{mpsc, oneshot, Notify},
};

use crate::{
    cluster::Cluster,
    commands::{Handle, HandledResult},
    halo_capnp::{self, *},
    resource::Location,
    state::{Event, Record},
};

pub mod power;
pub use power::{FenceAgent, FenceCommand, RedfishArgs};

mod ha;
mod non_ha;

#[derive(Debug, Clone)]
struct HostAddress {
    name: String,
    port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HostStatus {
    Up,
    Down,
    Unknown,
}

/// Commands which can be issued to a Host by the sysadmin using the CLI utility.
#[derive(Debug)]
pub enum HostCommand {
    /// Failback resources. If any of this host's resources are not currently home, then reclaim
    /// them from the partner and start managing them (if possible).
    Failback,

    /// Deactivate this Host - HALO must not start any resources on this Host while it is
    /// deactivated.
    Deactivate,
    Fence,
}

#[derive(Debug)]
struct FenceEvent {
    /// The reason given by the admin for fencing.
    reason: Option<String>,

    /// A channel used to communicate the result of the fence back to the server task that
    /// initiated the request. This in turn allows the result to be communicated back to the CLI
    /// request.
    result: oneshot::Sender<FenceResult>,
}

impl FenceEvent {
    fn new(reason: Option<String>, result: oneshot::Sender<FenceResult>) -> Self {
        Self { reason, result }
    }
}

#[derive(Debug)]
pub enum FenceResult {
    Success,
    AlreadyInProgress,
    PowerCommandFailed,
    WritingStateRecordFailed,
}

/// A server on which services can run.
#[derive(Debug)]
pub struct Host {
    address: HostAddress,
    fence_agent: Option<FenceAgent>,
    failover_partner: OnceLock<Option<Arc<Host>>>,

    /// The sender, receiver pair is used to send commands to the Host management task.
    sender: mpsc::Sender<HostMessage>,
    receiver: tokio::sync::Mutex<mpsc::Receiver<HostMessage>>,

    /// Whether the Host is active (i.e., resources are allowed to be ran on it)
    active: std::sync::Mutex<bool>,

    /// Whether the manager has an alive connection to the remote agent that corresponds to this
    /// Host.
    connected: std::sync::Mutex<bool>,

    /// Whether the Host has been fenced. Cleared by the admin after making the Host healthy again.
    fenced: std::sync::Mutex<bool>,

    /// Information on an in-progress fence event--or None if no admin fence request is
    /// in-progress.
    fence_event: std::sync::Mutex<Option<FenceEvent>>,
}

impl Host {
    pub fn new(name: &str, port: Option<u16>, fence_agent: Option<FenceAgent>) -> Self {
        let (sender, receiver) = mpsc::channel(1024);
        Host {
            address: HostAddress {
                name: name.to_string(),
                port: match port {
                    Some(p) => p,
                    None => crate::remote_port(),
                },
            },
            fence_agent,
            failover_partner: OnceLock::new(),
            sender,
            receiver: tokio::sync::Mutex::new(receiver),
            active: std::sync::Mutex::new(true),
            connected: std::sync::Mutex::new(false),
            fenced: std::sync::Mutex::new(false),
            fence_event: std::sync::Mutex::new(None),
        }
    }

    /// Create a Host object from a given config::Host object.
    pub fn from_config(config: &crate::config::Host) -> Self {
        let (name, port) = Self::get_host_port(&config.hostname);
        let fence_agent = config
            .fence_agent
            .as_ref()
            .map(|agent| FenceAgent::from_params(agent, &config.fence_parameters));
        Host::new(name, port, fence_agent)
    }

    /// Given a string that may be of the form "<address>:port number>", split it out into the address
    /// and port number portions.
    fn get_host_port(host_str: &str) -> (&str, Option<u16>) {
        let mut split = host_str.split(':');
        let host = split.nth(0).unwrap();
        let port = split.nth(0).map(|port| port.parse::<u16>().unwrap());
        (host, port)
    }

    /// Specify the host that should be this host's failover partner.
    ///
    /// Note that this function should be called exactly once to initialize the failover partner.
    pub fn set_failover_partner(
        &self,
        partner: Option<Arc<Self>>,
    ) -> crate::commands::HandledResult<()> {
        let new_partner = partner.map(|fp| Arc::clone(&fp));
        self.failover_partner.set(new_partner).handle_err(|_| {
            let curr_partner = match self.failover_partner.get().unwrap() {
                Some(fp) => fp.name(),
                None => "<none>",
            };
            eprintln!(
                "failed to set failover partner: host '{}' already has failover partner '{}'!",
                self.name(),
                curr_partner
            );
        })
    }

    /// Retrieve a reference to this host's failover partner.
    pub fn failover_partner(&self) -> Option<&Arc<Host>> {
        self.failover_partner
            .get()
            .expect(&format!(
                "failover partner for host '{}' has not been initialized!",
                self.name()
            ))
            .as_ref()
    }

    async fn receive_message(&self) -> HostMessage {
        let mut receiver = self.receiver.lock().await;
        receiver.recv().await.expect(&format!(
            "Host channel on {} unexpectedly closed!",
            self.id()
        ))
    }

    /// Handle a request from the CLI utility to do an action on this Host.
    pub async fn command(&self, command: HostCommand) {
        self.sender
            .send(HostMessage::Command(command))
            .await
            .expect("Sending host message {command} failed");
    }

    pub fn fence_agent(&self) -> &Option<FenceAgent> {
        &self.fence_agent
    }

    pub fn name(&self) -> &str {
        &self.address.name
    }

    pub fn port(&self) -> u16 {
        self.address.port
    }

    pub fn address(&self) -> String {
        format!("{}:{}", self.name(), self.port())
    }

    /// Get a unique identifier for this host. Typically, this will just be the hostname, but in
    /// the test environment, where Hosts do not have a unique hostname, the fencing target is used
    /// instead as a unique ID.
    pub fn id(&self) -> String {
        if let Some(FenceAgent::Test(test_args)) = &self.fence_agent {
            test_args.target.to_string()
        } else {
            self.name().to_string()
        }
    }

    pub fn set_active(&self, active: bool) {
        *self.active.lock().unwrap() = active;
    }

    pub fn active(&self) -> bool {
        *self.active.lock().unwrap()
    }

    pub fn set_connected(&self, connected: bool) {
        *self.connected.lock().unwrap() = connected;
    }

    pub fn connected(&self) -> bool {
        *self.connected.lock().unwrap()
    }

    pub fn fenced(&self) -> bool {
        *self.fenced.lock().unwrap()
    }

    pub fn set_fenced(&self, fenced: bool) {
        *self.fenced.lock().unwrap() = fenced;
    }

    async fn get_client(&self) -> io::Result<ocf_resource_agent::Client> {
        let client = halo_capnp::get_client(&self.address()).await;
        match client {
            Ok(_) => self.set_connected(true),
            Err(_) => self.set_connected(false),
        };
        client
    }

    pub async fn update_activation_status(
        self: &Arc<Self>,
        activate: bool,
        comment: Option<String>,
        cluster: &Arc<Cluster>,
    ) -> HandledResult<()> {
        self.set_active(activate);
        let event = if activate {
            Event::Activate
        } else {
            Event::Deactivate
        };
        cluster
            .write_record_nonblocking(Record::new(event, self.id(), comment))
            .await?;
        if !activate {
            self.command(HostCommand::Deactivate).await;
        }
        Ok(())
    }

    /// True if an admin fence request is in progress. This only applies to admin-initiated
    /// fencing, not automatic manager fencing.
    pub fn fence_request_in_progress(&self) -> bool {
        (*self.fence_event.lock().unwrap()).is_some()
    }

    /// Submit a fence request in response to an admin CLI command, and wait for the request to
    /// complete.
    ///
    /// This creates a tokio oneshot channel that is used to communicate the fence result back to
    /// this task. Because the fence is performed on different tasks, running concurrently with
    /// this one, the channel is needed to allow this task to wait until fencing is finished.
    pub async fn submit_admin_fence_request_and_wait(
        self: &Arc<Self>,
        comment: Option<String>,
    ) -> FenceResult {
        if self.fence_request_in_progress() {
            return FenceResult::AlreadyInProgress;
        }

        // Create a channel for transmitting the result of the fence request back to this task:
        let (tx, rx) = oneshot::channel::<FenceResult>();

        {
            let fence_event = FenceEvent::new(comment, tx);
            *self.fence_event.lock().unwrap() = Some(fence_event);
        }

        self.command(HostCommand::Fence).await;

        rx.await
            .expect("Recieving on oneshot channel should not fail.")
    }

    /// Returns the reason for fencing--if one is available.
    ///
    /// Returns None if no admin fence request is currently in progress, and also if there is an
    /// admin fence request in progress but the admin did not specify a reason. If distinguishing
    /// these cases is important, the caller must check self.fence_request_in_progress().
    pub fn fence_reason(&self) -> Option<String> {
        (*self.fence_event.lock().unwrap())
            .as_ref()
            .and_then(|event| event.reason.clone())
    }

    /// Complete a fence operation by informing the waiting task of the result.
    ///
    /// If a fence operation occurred as a result of admin CLI request, there is a task waiting on
    /// the result of that operation before it can return the result to the CLI program. This
    /// routine is used to send the result back to that task, if such a task exists.
    pub fn finish_admin_fence_request(&self, res: FenceResult) {
        if !self.fence_request_in_progress() {
            return;
        }

        let fence_event = (*self.fence_event.lock().unwrap())
            .take()
            .expect("Fence event must be set in order for admin fence request to be in progress.");

        fence_event
            .result
            .send(res)
            .expect("Sending on oneshot channel should not fail.");
    }

    async fn remote_liveness_check(&self, cluster: &Cluster) {
        loop {
            if self.get_client().await.is_ok() {
                return;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(cluster.args.sleep_time)).await;
        }
    }

    /// Mint a ResourceToken for each ResourceGroup whose home is this Host. This is the only place
    /// that ResourceTokens can be created - and they must never be destroyed.
    fn mint_resource_tokens(&self, cluster: &Cluster) -> Vec<ResourceToken> {
        cluster
            .host_home_resource_groups(self)
            .map(|rg| ResourceToken {
                id: rg.id().to_string(),
                location: Location::Home,
            })
            .collect()
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // In the test environment, a Host is more usefully identified via its "target" name which
        // is defined in its Fence Agent parameters. Otherwise, in a real environment, just use the
        // hostname.
        if let Some(FenceAgent::Test(test_args)) = &self.fence_agent {
            write!(f, "{} ({}:{})", test_args.target, self.name(), self.port())
        } else {
            write!(f, "{}", self.name())
        }
    }
}

/// A set of tasks that the Host management task is currently running.
// TODO: This Pin<Box<_>> stuff is gross... Can I use `Either` instead and make this nicer?
type ManagementTasks<'a> = FuturesUnordered<Pin<Box<dyn Future<Output = HostMessage> + 'a>>>;

#[derive(Debug)]
enum HostMessage {
    /// A command from the admin, via the CLI utility.
    Command(HostCommand),

    /// A message either from a partner Host task, or a child Resource task, indicating an action
    /// that should be taken for a particular ResourceGroup.
    Resource(ResourceMessage),

    /// A message from a child task indicating that it exited normally, and no further action is
    /// needed from this Host on the ResourceGroup that the task had been managing.
    ///
    /// Normally this would be because the child task passed the ResourceToken over to the partner
    /// host. (If it hadn't passed on the ResourceToken, then the ResourceToken would need to be
    /// returned in a HostMessage::Resource.)
    None(String),
}

#[derive(Debug)]
struct ResourceMessage {
    resource_group: ResourceToken,
    kind: Message,
}

/// The commands that can be sent to a Host management task.
#[derive(Debug)]
enum Message {
    /// Check the status of the resource group to determine if it is running or not. This message
    /// carries the assumption that the resource group should be started on the home node, if
    /// it is found to not be running on either node in the pair.
    CheckResourceGroup,

    /// Begin management of the resource group.
    ManageResourceGroup,

    /// Begin observation of the resource group -- check on its status, but don't start it if
    /// stopped.
    ObserveResourceGroup,

    /// A resource management task has observed a condition like "connection timed out" and
    /// failover should be triggered.
    RequestFailover,

    /// A resource management task has received a cancellation request and has exited.
    TaskCanceled,

    /// A resource management task has been told to stop and pass mangement over to the partner
    /// host.
    SwitchHost,

    /// A resource management task reported that this resource has an error which prevents the
    /// service from managing it.
    ResourceError,
}

fn new_message(rg: ResourceToken, kind: Message) -> HostMessage {
    HostMessage::Resource(ResourceMessage {
        resource_group: rg,
        kind,
    })
}

/// This object is shared between a parent Host task, and a ResourceGroup task that the Host task
/// has launched. The inner Notify objects are triggered by the Host task when it wants to cancel
/// the Resource task that holds it.
#[derive(Clone)]
struct ResourceTaskCancel {
    /// The ID of the ResourceGroup that this Cancel object is for.
    id: String,

    /// A notification mechanism used to tell a resource group management task to stop because
    /// connection was lost to the remote agent, and a fence action might be initiated.
    lost_connection: Rc<Notify>,

    /// A notification mechanism to tell a resource group management task to stop so that
    /// management can be handed over to the partner host.
    switch_host: Rc<Notify>,
}

impl ResourceTaskCancel {
    fn new(id: String) -> Self {
        Self {
            id,
            lost_connection: Rc::new(Notify::new()),
            switch_host: Rc::new(Notify::new()),
        }
    }
}

/// A ResourceToken represents the current state of a resource management task within the Host
/// management system.
///
/// At any given time, a ResourceToken is either:
/// - held by the management routine (i.e., the `manage_resource_group()` method on `Host`), if the
///   resource group is currently being managed; or
/// - held by a HostMessage object, if the resource is in some kind of transational state, like
///   waiting for failover to occur, or discovering its initial status on manager startup.
///
/// The ResourceToken is passed along from state to state to ensure that no ResourceGroup is ever
/// "dropped" and thus forgotten about.
///
/// To ensure that a ResourceGroup is never forgotten about, the drop() implementation panics, so
/// that it is a runtime error for a ResourceGroup to transition to an unexpected state.
#[derive(Debug)]
struct ResourceToken {
    id: String,
    location: Location,
}

impl Drop for ResourceToken {
    fn drop(&mut self) {
        panic!("Resource token {self:?} was illegally dropped!");
    }
}

// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{io, sync::Arc};

use {
    capnp::capability::Promise,
    capnp_rpc::{pry, rpc_twoparty_capnp, twoparty, RpcSystem},
    futures::AsyncReadExt,
};

use crate::{
    cluster,
    commands::{Handle, HandledResult},
    halo_capnp::halo_mgmt,
    LogStream,
};

/// An object that can be passed to manager functions holding some state that should be shared
/// between these functions.
#[derive(Debug)]
pub struct MgrContext {
    pub out_stream: LogStream,
    pub args: crate::commands::Cli,
}

impl MgrContext {
    pub fn new(args: crate::commands::Cli) -> Self {
        MgrContext {
            out_stream: crate::LogStream::new_stdout(),
            args,
        }
    }
}

struct HaloMgmtImpl {
    cluster: Arc<cluster::Cluster>,
}

/// Implementation of the server side of the Management (CLI to local daemon) RPC interface.
impl halo_mgmt::Server for HaloMgmtImpl {
    fn monitor(
        &mut self,
        _params: halo_mgmt::MonitorParams,
        mut results: halo_mgmt::MonitorResults,
    ) -> Promise<(), ::capnp::Error> {
        let cluster = &self.cluster;
        let mut message = ::capnp::message::Builder::new_default();
        let mut message = message.init_root::<halo_mgmt::cluster::Builder>();

        let mut resource_messages = message
            .reborrow()
            // TODO: store the total number of resources in Cluster so that this extra iteration
            // isn't necessary:
            .init_resources(cluster.resources().collect::<Vec<_>>().len() as u32);

        for (i, res) in cluster.resources().enumerate() {
            let mut message = resource_messages.reborrow().get(i as u32);
            message.set_status(res.get_status().into());
            message.set_managed(res.get_managed());
            let mut parameters = message
                .reborrow()
                .init_parameters(res.parameters.len() as u32);
            for (i, (k, v)) in res.parameters.iter().enumerate() {
                let mut param = parameters.reborrow().get(i as u32);
                param.set_key(k);
                param.set_value(v);
            }
        }

        match results.get().set_status(message.into_reader()) {
            Ok(_) => Promise::ok(()),
            Err(e) => Promise::err(e),
        }
    }
    fn set_managed(
        &mut self,
        params: halo_mgmt::SetManagedParams,
        mut results: halo_mgmt::SetManagedResults,
    ) -> Promise<(), ::capnp::Error> {
        let params = pry!(params.get());
        let resource = pry!(params.get_resource());
        let managed = params.get_managed();

        let mut error: Option<String> = Some(format!("Resource {:?} not found", resource));
        for res in self.cluster.resources() {
            if res.id == resource {
                error = None;
                if res.get_managed() == managed {
                    error = Some(format!(
                        "Resource {:?} is already {}",
                        resource,
                        if managed { "managed" } else { "unmanaged" }
                    ));
                } else {
                    res.set_managed(managed);
                }
            }
        }
        match error {
            Some(e) => pry!(results.get().get_res()).set_err(e),
            None => pry!(results.get().get_res()).set_ok(()),
        };

        Promise::ok(())
    }
}

/// Get a unix socket listener from a given socket path.
///
/// To avoid clobbering an already-in-use unix socket, a connection is attempted to an existing
/// unix socket first. If this fails, a new socket listener can be returned, since an existing
/// in-use socket was determined to be absent at the given location.
async fn prepare_unix_socket(addr: &String) -> io::Result<tokio::net::UnixListener> {
    // Check for existing socket in use
    match tokio::net::UnixStream::connect(&addr).await {
        Ok(_) => {
            eprintln!("Address already in use: {addr}");
            return Err(io::Error::from(io::ErrorKind::AddrInUse));
        }
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            eprintln!("Unexpected error while preparing unix socket '{addr}': {e}");
            return Err(e);
        }
    };
    match std::fs::remove_file(addr) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            eprintln!("error removing old socket: {e}");
            return Err(e);
        }
    };
    // Create new socket
    match tokio::net::UnixListener::bind(addr) {
        Ok(l) => Ok(l),
        Err(e) => {
            eprintln!("error binding to socket '{addr}': {e}");
            Err(e)
        }
    }
}

/// Main entrypoint for the command server.
///
/// This listens for commands on a unix socket and acts on them.
async fn server_main(listener: tokio::net::UnixListener, cluster: Arc<cluster::Cluster>) {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let mgmt_client: halo_mgmt::Client = capnp_rpc::new_client(HaloMgmtImpl { cluster });

            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(e) => {
                        // XXX: why might accept() fail? How to properly handle error here?
                        eprintln!("Could not accept connection: {e}");
                        continue;
                    }
                };
                let (reader, writer) =
                    tokio_util::compat::TokioAsyncReadCompatExt::compat(stream).split();
                let network = twoparty::VatNetwork::new(
                    futures::io::BufReader::new(reader),
                    futures::io::BufWriter::new(writer),
                    rpc_twoparty_capnp::Side::Server,
                    Default::default(),
                );

                let rpc_system =
                    RpcSystem::new(Box::new(network), Some(mgmt_client.clone().client));

                tokio::task::spawn_local(rpc_system);
            }
        })
        .await
}

/// Main entrypoint for the management service, which monitors and controls the state of
/// the cluster.
async fn manager_main(cluster: Arc<cluster::Cluster>) {
    cluster.main_loop().await;
}

/// Rust client management daemon -
///
/// This launches two "services".
///
/// - A manager service which continuously monitors the state of the cluster.
///   The monitoring service takes actions based on cluster status, such as migrating resources,
///   fencing nodes, etc.
///
/// - A server that listens on a unix socket (/var/run/halo.socket) for
///   commands from the command line interface.
pub fn main(cluster: cluster::Cluster) -> HandledResult<()> {
    let cluster = Arc::new(cluster);

    let manager_rt = tokio::runtime::Runtime::new()
        .handle_err(|e| eprintln!("Could not launch manager runtime: {e}"))?;

    let cli_rt = tokio::runtime::Runtime::new()
        .handle_err(|e| eprintln!("Could not launch CLI server runtime: {e}"))?;

    std::thread::scope(|s| {
        // Launch the Management thread:
        s.spawn(|| {
            manager_rt.block_on(async {
                manager_main(Arc::clone(&cluster)).await;
            });
        });

        // Launch the CLI Server process to listen for CLI commands:
        cli_rt.block_on(async {
            let addr = match &cluster.context.args.socket {
                Some(s) => s,
                None => &crate::default_socket(),
            };
            let listener = match prepare_unix_socket(addr).await {
                Ok(l) => l,
                Err(_) => {
                    std::process::exit(1);
                }
            };
            if cluster.context.args.verbose {
                eprintln!("listening on socket '{addr}'");
            }
            server_main(listener, Arc::clone(&cluster)).await;
        })
    });

    Ok(())
}

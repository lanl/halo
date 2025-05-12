// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use capnp::capability::Promise;
use capnp_rpc::{rpc_twoparty_capnp, twoparty, RpcSystem};
use futures::AsyncReadExt;
use std::sync::Arc;

use crate::cluster;
use crate::halo_capnp::halo_mgmt;
use crate::LogStream;

/// An object that can be passed to manager functions holding some state that should be shared
/// between these functions.
#[derive(Debug)]
pub struct MgrContext {
    pub out_stream: LogStream,
    pub args: crate::commands::Cli,
}

impl MgrContext {
    pub fn new(args: crate::commands::Cli) -> Self {
        let mut context = Self::default();
        context.args = args;
        context
    }
}

impl Default for MgrContext {
    fn default() -> MgrContext {
        MgrContext {
            out_stream: crate::LogStream::new_stdout(),
            args: crate::commands::Cli::default(),
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
///     The monitoring service takes actions based on cluster status, such as migrating resources,
///     fencing nodes, etc.
///
/// - A server that listens on a unix socket (/var/run/halo.socket) for
///     commands from the command line interface.
pub fn main(cluster: cluster::Cluster) -> crate::commands::Result {
    let cluster = Arc::new(cluster);

    let manager_rt = tokio::runtime::Runtime::new()
        .inspect_err(|e| eprintln!("Could not launch manager runtime: {e}"))?;

    let cli_rt = tokio::runtime::Runtime::new()
        .inspect_err(|e| eprintln!("Could not launch CLI server runtime: {e}"))?;

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
            // check for errors? (ENOENT expected)
            let _ = std::fs::remove_file(&addr);
            let listener = tokio::net::UnixListener::bind(&addr).unwrap_or_else(|e| {
                eprintln!("Could not listen on \"{addr}\": {e}");
                std::process::exit(1);
            });
            if cluster.context.args.verbose {
                eprintln!("listening on socket '{addr}'");
            }
            server_main(listener, Arc::clone(&cluster)).await;
        })
    });

    Ok(())
}

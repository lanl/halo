// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{io, sync::Arc};

use {clap::Parser, log::info};

use crate::{
    cluster,
    commands::{Handle, HandledResult},
    LogStream,
};

pub mod http;

#[derive(Parser, Debug, Default)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[arg(long)]
    pub config: Option<String>,

    #[arg(long)]
    pub socket: Option<String>,

    #[arg(long)]
    pub mtls: bool,

    #[arg(long)]
    pub verbose: bool,

    /// Whether to run in Observe mode (Default, only check on resource status, don't actively
    /// start/stop resources), or Manage mode (actively manage resource state)
    #[arg(long)]
    pub manage_resources: bool,

    /// Whether to treat network errors like "Connection Reset" or "Connection Refused" as
    /// fencable. This is ONLY for use in the test environment; in production environments, such
    /// errors indicate a configuration issue that needs to be resolved.
    #[arg(long, hide = true)]
    pub fence_on_connection_close: bool,
}

/// An object that can be passed to manager functions holding some state that should be shared
/// between these functions.
#[derive(Debug)]
pub struct MgrContext {
    pub out_stream: LogStream,
    pub args: Cli,
}

impl MgrContext {
    pub fn new(args: Cli) -> Self {
        MgrContext {
            out_stream: crate::LogStream::new_stdout(),
            args,
        }
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
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .handle_err(|e| eprintln!("Could not launch manager runtime: {e}"))?;

    rt.block_on(tokio::task::LocalSet::new().run_until(async {
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

        info!("listening on socket '{addr}'");

        let cluster = Arc::new(cluster);

        futures::join!(
            http::server_main(listener, Arc::clone(&cluster)),
            manager_main(cluster)
        );
    }));

    Ok(())
}

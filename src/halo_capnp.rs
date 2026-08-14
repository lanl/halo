// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{env, io};

use futures::AsyncReadExt;

use crate::{cluster, host, remote::ocf, resource::Resource};

use capnp_rpc::{rpc_twoparty_capnp, twoparty, RpcSystem};

include!(concat!(env!("OUT_DIR"), "/halo_capnp.rs"));

/// Alias for a capnp operation RPC, client side
type OperationRequest = ::capnp::capability::Request<
    ocf_resource_agent::operation_params::Owned,
    ocf_resource_agent::operation_results::Owned,
>;

type OcfOperationResults =
    ::capnp::capability::Response<ocf_resource_agent::operation_results::Owned>;

#[derive(Debug)]
pub enum AgentReply {
    /// A reply from the remote agent, indicating that the operation was attempted. The ocf::Status
    /// contains the result of attempting the operation.
    Success(ocf::Status),

    /// A reply from the remote agent, indicating that the operation could not be attempted, due to
    /// an error on the remote server.
    Error(String),
}

/// Sends an OCF request to perform `op` to the remote agent reached by `client`.
///
/// Returns a `Result` that contains whether an error occurred while attempting the remote
/// operation, or contains the result of the operation if the request was succesful.
///
/// Note that an `Ok(_)` variant does *not* mean that the operation completed succesfully! It
/// simply means that the client was able to succesfully communicate with the remote agent. An
/// error could have occurred while the remote agent attempted the operation, and such an error is
/// held in the `Ok(_)` variant.
///
/// An `Err(_)` variant means that succesful communication did not occur, so it is unknown whether
/// the operation was attempted or what the outcome was if it was attempted.
pub async fn remote_ocf_operation_given_client(
    res: &Resource,
    client: &host::Client,
    op: ocf_resource_agent::Operation,
) -> Result<AgentReply, capnp::Error> {
    let client = &client.client;
    let mut request = client.operation_request();
    prep_request(&mut request, res, op);

    let reply = request.send().promise.await?;

    get_status(reply)
}

fn get_status(reply: OcfOperationResults) -> Result<AgentReply, capnp::Error> {
    let status = reply.get()?.get_result()?;

    Ok(match status.which()? {
        ocf_resource_agent::result::Ok(inner_result) => match inner_result?.which()? {
            ocf_resource_agent::inner_result::InnerOk(()) => {
                AgentReply::Success(ocf::Status::Success)
            }
            ocf_resource_agent::inner_result::InnerErr(e) => {
                let e = e?;
                let code = e.get_code();
                let message = e.get_message()?.to_str()?;
                AgentReply::Success(ocf::Status::Error(code.into(), message.into()))
            }
        },
        ocf_resource_agent::result::Err(e) => AgentReply::Error(e?.to_str()?.into()),
    })
}

/// Prepare a capnp operation RPC request.
fn prep_request(request: &mut OperationRequest, res: &Resource, op: ocf_resource_agent::Operation) {
    let mut request = request.get();

    request.set_op(op);

    request.set_resource(res.kind.clone());
    let mut args = request.init_args(res.parameters.len() as u32);
    for (i, param) in res.parameters.iter().enumerate() {
        let mut arg = args.reborrow().get(i as u32);
        arg.set_key(param.0.clone());
        arg.set_value(param.1.clone());
    }
}

/// Attempt to establish a TCP stream to the given socket address from the source address.
async fn tcp_try_connect_one(
    from_addr: std::net::SocketAddr,
    to_addr: std::net::SocketAddr,
) -> io::Result<tokio::net::TcpStream> {
    let sock = tokio::net::TcpSocket::new_v4().inspect_err(|e| {
        log::debug!("could not create new tcpv4 socket: {e}");
    })?;
    sock.set_reuseaddr(true).unwrap();
    sock.bind(from_addr).map_err(|e| {
        log::debug!("could not bind to address {from_addr}: {e}");
        e
    })?;
    log::debug!("connecting to address: {to_addr}");
    sock.connect(to_addr).await
}

/// Attempt to establish a TCP stream to the given TCP address that may resolve to multiple IP addresses.
async fn tcp_try_connect(
    from_addr: std::net::SocketAddr,
    to_addr: &str,
) -> io::Result<tokio::net::TcpStream> {
    let to_addrs = tokio::net::lookup_host(to_addr).await.inspect_err(|e| {
        log::debug!("cannot parse host '{to_addr}' as an address: {e}");
    })?;
    let mut stream = None;
    for addr in to_addrs {
        match tcp_try_connect_one(from_addr, addr).await {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => {
                log::debug!("could not connect to host '{addr}': {e}");
            }
        }
    }
    let stream = stream.ok_or_else(|| {
        log::debug!("could not connect to any address for host '{to_addr}'");
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("connection failed to host '{to_addr}'"),
        )
    })?;

    Ok(stream)
}

pub async fn get_client(
    address: &str,
    cluster: &cluster::Cluster,
) -> io::Result<ocf_resource_agent::Client> {
    // Bind to specific cluster address if it has been specified.
    let stream = if !cluster.args.use_insecure_port {
        let Some(cluster_sock) = &cluster.address else {
            panic!("cluster.address should be Some when cluster.args.use_insecure_port == false");
        };
        tcp_try_connect(cluster_sock.address(), address).await?
    } else {
        tokio::net::TcpStream::connect(address).await?
    };
    stream.set_nodelay(true).expect("setting nodelay failed.");

    match &cluster.tls_args {
        Some(args) => {
            // Perform mtls handshake
            let mtls_stream = args
                .tls_connector
                .connect(args.domain.clone(), stream)
                .await?;

            let (reader, writer) =
                tokio_util::compat::TokioAsyncReadCompatExt::compat(mtls_stream).split();
            let rpc_network = Box::new(twoparty::VatNetwork::new(
                futures::io::BufReader::new(reader),
                futures::io::BufWriter::new(writer),
                rpc_twoparty_capnp::Side::Client,
                Default::default(),
            ));
            let mut rpc_system = RpcSystem::new(rpc_network, None);
            let client: ocf_resource_agent::Client =
                rpc_system.bootstrap(rpc_twoparty_capnp::Side::Server);

            tokio::task::spawn_local(rpc_system);

            Ok(client)
        }
        None => {
            let (reader, writer) =
                tokio_util::compat::TokioAsyncReadCompatExt::compat(stream).split();
            let rpc_network = Box::new(twoparty::VatNetwork::new(
                futures::io::BufReader::new(reader),
                futures::io::BufWriter::new(writer),
                rpc_twoparty_capnp::Side::Client,
                Default::default(),
            ));
            let mut rpc_system = RpcSystem::new(rpc_network, None);
            let client: ocf_resource_agent::Client =
                rpc_system.bootstrap(rpc_twoparty_capnp::Side::Server);

            tokio::task::spawn_local(rpc_system);

            Ok(client)
        }
    }
}

// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{env, io};

use {futures::AsyncReadExt, rustls::pki_types::ServerName};

use crate::{
    remote::ocf,
    resource::{Location, Resource},
    tls::get_connector,
};

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

#[derive(Debug)]
pub enum AgentError {
    /// An IO error occurred while trying to send/receive.
    Io(io::Error),

    /// An error occurred in the RPC protocol.
    Rpc(capnp::Error),
}

impl From<io::Error> for AgentError {
    fn from(e: io::Error) -> Self {
        AgentError::Io(e)
    }
}

impl From<capnp::Error> for AgentError {
    fn from(e: capnp::Error) -> Self {
        AgentError::Rpc(e)
    }
}

/// Sends an OCF request to perform `op` to a remote agent, determined by `res` and `loc`.
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
pub async fn remote_ocf_operation(
    res: &Resource,
    loc: Location,
    op: ocf_resource_agent::Operation,
) -> Result<AgentReply, AgentError> {
    let request = get_ocf_request(res, loc, op).await?;

    let reply = request.send().promise.await?;

    Ok(get_status(reply)?)
}

pub async fn remote_ocf_operation_given_client(
    res: &Resource,
    client: &ocf_resource_agent::Client,
    op: ocf_resource_agent::Operation,
) -> Result<AgentReply, capnp::Error> {
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

/// Create a capnp RPC client and set up the client to perform the operation() RPC.
async fn get_ocf_request(
    res: &Resource,
    loc: Location,
    op: ocf_resource_agent::Operation,
) -> io::Result<OperationRequest> {
    let hostname = match loc {
        Location::Home => res.home_node.address(),
        Location::Away => res
            .failover_node
            .as_ref()
            .expect("Called operation on failover node for resource without failover node")
            .address(),
    };
    let stream = tokio::net::TcpStream::connect(hostname).await?;
    stream.set_nodelay(true).expect("Setting nodelay failed.");

    if res.context.args.mtls {
        // Create mtls connector
        let mtls_connector = get_connector();

        // Set domain/hostname of server we intend to connect to
        let domain = ServerName::try_from(
            env::var("HALO_SERVER_DOMAIN_NAME").expect("HALO_SERVER_DOMAIN_NAME not set."),
        )
        .unwrap();

        // Perform mtls handshake
        let mtls_stream = mtls_connector.connect(domain, stream).await?;

        Ok(__get_ocf_request(mtls_stream, res, op))
    } else {
        Ok(__get_ocf_request(stream, res, op))
    }
}

fn __get_ocf_request<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + 'static>(
    stream: S,
    res: &Resource,
    op: ocf_resource_agent::Operation,
) -> OperationRequest {
    let (reader, writer) = tokio_util::compat::TokioAsyncReadCompatExt::compat(stream).split();
    let rpc_network = Box::new(twoparty::VatNetwork::new(
        futures::io::BufReader::new(reader),
        futures::io::BufWriter::new(writer),
        rpc_twoparty_capnp::Side::Client,
        Default::default(),
    ));
    let mut rpc_system = RpcSystem::new(rpc_network, None);
    let client: ocf_resource_agent::Client = rpc_system.bootstrap(rpc_twoparty_capnp::Side::Server);

    tokio::task::spawn_local(rpc_system);

    let mut request = client.operation_request();
    prep_request(&mut request, res, op);

    request
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

pub async fn get_client(address: &str) -> io::Result<ocf_resource_agent::Client> {
    let stream = tokio::net::TcpStream::connect(address).await?;
    stream.set_nodelay(true).expect("setting nodelay failed.");

    let (reader, writer) = tokio_util::compat::TokioAsyncReadCompatExt::compat(stream).split();

    let rpc_network = Box::new(twoparty::VatNetwork::new(
        futures::io::BufReader::new(reader),
        futures::io::BufWriter::new(writer),
        rpc_twoparty_capnp::Side::Client,
        Default::default(),
    ));
    let mut rpc_system = RpcSystem::new(rpc_network, None);
    let client: ocf_resource_agent::Client = rpc_system.bootstrap(rpc_twoparty_capnp::Side::Server);

    tokio::task::spawn_local(rpc_system);

    Ok(client)
}

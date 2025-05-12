// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use futures::AsyncReadExt;
use rustls::pki_types::ServerName;
use std::env;
use std::error::Error;
use std::fmt;

use crate::resource::{self, Location, Resource};
use crate::tls::get_connector;
use capnp_rpc::{rpc_twoparty_capnp, twoparty, RpcSystem};

include!(concat!(env!("OUT_DIR"), "/halo_capnp.rs"));

/// Alias for a capnp operation RPC, client side
type OperationRequest = ::capnp::capability::Request<
    ocf_resource_agent::operation_params::Owned,
    ocf_resource_agent::operation_results::Owned,
>;

pub type OcfOperationResults =
    ::capnp::capability::Response<ocf_resource_agent::operation_results::Owned>;

impl std::convert::From<resource::ResourceStatus> for halo_mgmt::Status {
    fn from(stat: resource::ResourceStatus) -> Self {
        match stat {
            resource::ResourceStatus::Unknown => halo_mgmt::Status::Unknown,
            resource::ResourceStatus::CheckingHome => halo_mgmt::Status::CheckingHome,
            resource::ResourceStatus::RunningOnHome => halo_mgmt::Status::RunningOnHome,
            resource::ResourceStatus::Stopped => halo_mgmt::Status::Stopped,
            resource::ResourceStatus::CheckingAway => halo_mgmt::Status::CheckingAway,
            resource::ResourceStatus::RunningOnAway => halo_mgmt::Status::RunningOnAway,
            resource::ResourceStatus::Unrunnable => halo_mgmt::Status::Unrunnable,
        }
    }
}

impl fmt::Display for halo_mgmt::Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                halo_mgmt::Status::Unknown => "Unknown",
                halo_mgmt::Status::CheckingHome => "Checking on home",
                halo_mgmt::Status::RunningOnHome => "Home",
                halo_mgmt::Status::Stopped => "Stopped",
                halo_mgmt::Status::CheckingAway => "Checking on failover",
                halo_mgmt::Status::RunningOnAway => "Failed over",
                halo_mgmt::Status::Unrunnable => "Can't run anywhere",
            }
        )
    }
}

/// Prepare a capnp operation RPC request.
/// res: The resource that the operation will be performed on.
/// op: The operation to perform.
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

/// Create a capnp RPC client and set up the client to perform the operation() RPC.
async fn get_ocf_request(
    res: &Resource,
    loc: Location,
    op: ocf_resource_agent::Operation,
) -> Result<OperationRequest, Box<dyn Error>> {
    let hostname = match loc {
        Location::Home => res.home_node.to_string(),
        Location::Away => res
            .failover_node
            .as_ref()
            .expect("Called operation on failover node for resource without failover node")
            .to_string(),
    };
    let stream = tokio::net::TcpStream::connect(hostname).await?;
    stream.set_nodelay(true)?;

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

        __get_ocf_request(mtls_stream, res, op)
    } else {
        __get_ocf_request(stream, res, op)
    }
}

fn __get_ocf_request<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + 'static>(
    stream: S,
    res: &Resource,
    op: ocf_resource_agent::Operation,
) -> Result<OperationRequest, Box<dyn Error>> {
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

    Ok(request)
}

pub async fn do_ocf_request<'a>(
    res: &Resource,
    loc: Location,
    op: ocf_resource_agent::Operation,
) -> Result<OcfOperationResults, Box<dyn Error>> {
    let request = get_ocf_request(res, loc, op).await?;

    let reply = request.send().promise.await?;
    Ok(reply)
}

// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{error::Error, net::Ipv4Addr, str::FromStr};

use {
    capnp::capability::Promise,
    capnp_rpc::{pry, rpc_twoparty_capnp, twoparty, RpcSystem},
    clap::Parser,
    futures::AsyncReadExt,
    nix::ifaddrs,
};

use crate::{halo_capnp::ocf_resource_agent, tls::get_acceptor};

pub mod ocf;

struct OcfResourceAgentImpl {
    cli: Cli,
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    /// If a CIDR network is specified, the agent will only listen on an IP address in that
    /// network, or will fail to start if there is no such IP address.
    #[arg(long)]
    pub network: Option<String>,

    #[arg(long)]
    pub port: Option<u16>,

    #[arg(short, long)]
    pub verbose: bool,

    /// For the test environment, a remote agent can be given an ID to assist with identifying
    /// multiple agents running on the same system.
    #[arg(long)]
    pub test_id: Option<String>,

    /// The directory that holds the OCF resource agent scripts.
    #[arg(long)]
    pub ocf_root: Option<String>,

    ///Enable mTLS, must also be enabled on client side to function
    #[arg(long)]
    pub mtls: bool,
}

/// Launches the remote agent, which listens on an IP address in `network` using `port`.
pub fn agent_main(args: Cli) -> Result<(), Box<dyn Error>> {
    crate::test_env::maybe_identify_agent_for_test_fence(&args);

    let network = args.network.clone().unwrap_or(crate::default_network());
    let network = cidr::Ipv4Cidr::from_str(&network).unwrap();
    let port = args.port.unwrap_or(crate::remote_port());
    let addr = match get_listening_address(network) {
        Some(addr) => addr,
        None => {
            eprintln!("Could not find address matching {} to listen on.", network);
            eprintln!("Try specifying management network in environment as HALO_NET=$net.");
            return Err(From::from(std::io::Error::from(
                std::io::ErrorKind::AddrNotAvailable,
            )));
        }
    };

    let addr = format!("{addr}:{}", port);

    let rt = tokio::runtime::Runtime::new().expect("Failed to launch runtime.");
    rt.block_on(async { __agent_main(args, &addr).await })?;

    Ok(())
}

/// Given a `network` in CIDR form, tries to find an IP address on the system in that network.
fn get_listening_address(network: cidr::Ipv4Cidr) -> Option<Ipv4Addr> {
    let ifaddrs = ifaddrs::getifaddrs().unwrap();
    for ifa in ifaddrs {
        if let Some(addr) = ifa.address {
            if let Some(addr) = addr.as_sockaddr_in() {
                let addr = addr.ip();
                if network.contains(&addr) {
                    return Some(addr);
                }
            }
        }
    }

    None
}

async fn __agent_main(args: Cli, addr: &str) -> Result<(), Box<dyn Error>> {
    let mtls = args.mtls;
    tokio::task::LocalSet::new()
        .run_until(async move {
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .inspect_err(|e| eprintln!("Could not listen on address \"{addr}\": {e}"))?;
            if args.verbose {
                eprintln!("Listening on {addr}");
            }

            let agent_client: ocf_resource_agent::Client =
                capnp_rpc::new_client(OcfResourceAgentImpl { cli: args });

            loop {
                let (stream, _) = listener.accept().await?;
                stream.set_nodelay(true)?;
                if mtls {
                    //Create mtls acceptor
                    let mtls_acceptor = get_acceptor();

                    //mTLS handshake
                    let mtls_stream = match mtls_acceptor.accept(stream).await {
                        Ok(s) => s,
                        Err(e) => {
                            return Err(format!("mTLS accept error: {}", e).into());
                        }
                    };
                    __agent_rpc_main(mtls_stream, agent_client.clone());
                } else {
                    __agent_rpc_main(stream, agent_client.clone());
                }
            }
        })
        .await
}

fn __agent_rpc_main<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + 'static>(
    stream: S,
    agent_client: ocf_resource_agent::Client,
) {
    let (reader, writer) = tokio_util::compat::TokioAsyncReadCompatExt::compat(stream).split();
    let network = twoparty::VatNetwork::new(
        futures::io::BufReader::new(reader),
        futures::io::BufWriter::new(writer),
        rpc_twoparty_capnp::Side::Server,
        Default::default(),
    );

    let rpc_system = RpcSystem::new(Box::new(network), Some(agent_client.client));

    tokio::task::spawn_local(rpc_system);
}

impl ocf_resource_agent::Server for OcfResourceAgentImpl {
    fn operation(
        &mut self,
        params: ocf_resource_agent::OperationParams,
        mut results: ocf_resource_agent::OperationResults,
    ) -> Promise<(), ::capnp::Error> {
        let params = pry!(params.get());
        let resource = pry!(params.get_resource());
        let resource = pry!(resource.to_str());

        let op = pry!(params.get_op());
        let op = match op {
            ocf_resource_agent::Operation::Monitor => ocf::Operation::Monitor,
            ocf_resource_agent::Operation::Start => ocf::Operation::Start,
            ocf_resource_agent::Operation::Stop => ocf::Operation::Stop,
        };

        let args = pry!(params.get_args());
        let mut ocf_args: Vec<(&str, &str)> = Vec::new();
        for i in 0..args.len() {
            let arg = args.get(i);
            let key = pry!(pry!(arg.get_key()).to_str());
            let value = pry!(pry!(arg.get_value()).to_str());
            ocf_args.push((key, value));
        }
        let ocf_args = ocf::Arguments::from(&ocf_args);

        if self.cli.verbose {
            log_operation(&op, &ocf_args);
        }

        match ocf::do_operation(resource, op, &ocf_args, &self.cli) {
            Ok((exit_code, error_output)) => {
                let mut ok_result = pry!(results.get().get_result());
                let mut inner_result = ::capnp::message::Builder::new_default();
                let mut inner_result =
                    inner_result.init_root::<ocf_resource_agent::inner_result::Builder>();

                if exit_code == 0 {
                    inner_result.set_inner_ok(());
                } else {
                    let mut ocf_error = ::capnp::message::Builder::new_default();
                    let mut ocf_error =
                        ocf_error.init_root::<ocf_resource_agent::ocf_error::Builder>();
                    ocf_error.set_code(exit_code);
                    ocf_error.set_message(error_output);
                    pry!(inner_result.set_inner_err(ocf_error.into_reader()));
                }
                pry!(ok_result.set_ok(inner_result.into_reader()));
            }
            Err(e) => {
                pry!(results.get().get_result()).set_err(e);
            }
        };

        Promise::ok(())
    }
}

/// Print a message to stderr with the operation and arguments, for debugging.
fn log_operation(op: &ocf::Operation, ocf_args: &ocf::Arguments) {
    let mut msg = format!("Got operation request: {op}\n");
    for (k, v) in ocf_args.args.iter() {
        msg.push_str(&format!("    {}: {}\n", k, v));
    }
    eprintln!("{msg}");
}

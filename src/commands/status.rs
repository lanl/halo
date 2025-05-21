// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use capnp_rpc::{rpc_twoparty_capnp, twoparty, RpcSystem};
use clap::Args;
use futures::AsyncReadExt;

use crate::commands::Cli;
use crate::halo_capnp::halo_mgmt;

#[derive(Args, Debug, Clone)]
pub struct StatusArgs {
    #[arg(short = 'x')]
    exclude_normal: bool,
}

pub async fn status(cli: &Cli, args: &StatusArgs) -> Result<(), Box<dyn std::error::Error>> {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let addr = match &cli.socket {
                Some(s) => s,
                None => &crate::default_socket(),
            };
            let stream = tokio::net::UnixStream::connect(addr)
                .await
                .inspect_err(|e| eprintln!("Could not connect to socket \"{addr}\": {e}"))?;
            let (reader, writer) =
                tokio_util::compat::TokioAsyncReadCompatExt::compat(stream).split();
            let rpc_network = Box::new(twoparty::VatNetwork::new(
                futures::io::BufReader::new(reader),
                futures::io::BufWriter::new(writer),
                rpc_twoparty_capnp::Side::Client,
                Default::default(),
            ));
            let mut rpc_system = RpcSystem::new(rpc_network, None);
            let client: halo_mgmt::Client = rpc_system.bootstrap(rpc_twoparty_capnp::Side::Server);

            tokio::task::spawn_local(rpc_system);

            let request = client.monitor_request();

            let reply = request.send().promise.await?;
            let cluster_status = reply.get()?.get_status()?;

            print_status(cluster_status, args)?;

            Ok(())
        })
        .await
}

fn print_status(
    response: halo_mgmt::cluster::Reader,
    _args: &StatusArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let resources = response.get_resources()?;
    for i in 0..resources.len() {
        let res = resources.get(i);
        let status = match res.get_status()? {
            halo_mgmt::Status::RunningOnHome => "OK".to_string(),
            other => format!("{}", other),
        };
        print!("{}: [", status);

        let params = res.get_parameters()?;
        for i in 0..params.len() {
            if i > 0 {
                print!(", ");
            }
            let param = params.get(i);
            print!(
                "{}: {}",
                param.get_key()?.to_str()?,
                param.get_value()?.to_str()?
            );
        }

        println!("]");
    }

    Ok(())
}

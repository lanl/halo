// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

use crate::commands::{self, get_rpc_client, Cli};
use crate::halo_capnp::halo_mgmt;

#[derive(Args, Debug, Clone)]
pub struct StatusArgs {
    #[arg(short = 'x')]
    exclude_normal: bool,
}

pub async fn status(cli: &Cli, args: &StatusArgs) -> commands::Result {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let client = get_rpc_client(cli).await?;
            let request = client.monitor_request();

            let reply = request.send().promise.await?;
            let cluster_status = reply.get()?.get_status()?;

            if let Err(e) = print_status(cluster_status, args) {
                eprintln!("Could not get status: {e}");
                commands::err()
            } else {
                Ok(())
            }
        })
        .await
}

fn print_status(response: halo_mgmt::cluster::Reader, _args: &StatusArgs) -> commands::Result {
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

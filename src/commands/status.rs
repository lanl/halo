// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

use crate::{
    commands::{get_rpc_client, Cli, Handle, HandledResult},
    halo_capnp::{halo_mgmt, MonitorResults},
};

#[derive(Args, Debug, Clone)]
pub struct StatusArgs {
    #[arg(short = 'x')]
    exclude_normal: bool,
}

pub async fn status(cli: &Cli, args: &StatusArgs) -> HandledResult<()> {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let client = get_rpc_client(cli).await?;
            let request = client.monitor_request();

            let reply = request
                .send()
                .promise
                .await
                .handle_err(|e| eprintln!("Error sending RPC request: {e}"))?;

            get_and_print_status(reply, args)
                .handle_err(|e| eprintln!("Could not get cluster status: {e}"))
        })
        .await
}

fn get_and_print_status(reply: MonitorResults, _args: &StatusArgs) -> Result<(), capnp::Error> {
    let cluster_status = reply.get()?.get_status()?;

    let resources = cluster_status.get_resources()?;
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

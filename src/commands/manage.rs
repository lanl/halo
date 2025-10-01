// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

use crate::commands::{get_rpc_client, Cli, Handle, HandledResult};
use crate::halo_capnp::halo_mgmt::{command_result, set_managed_results};

#[derive(Args, Debug, Clone)]
pub struct ManageArgs {
    /// Resource to manage
    resource_id: String,
}

#[derive(Args, Debug, Clone)]
pub struct UnManageArgs {
    /// Resource to manage
    resource_id: String,
}

pub async fn manage(cli: &Cli, args: &ManageArgs) -> HandledResult<()> {
    send_command(cli, &args.resource_id, true).await
}

pub async fn unmanage(cli: &Cli, args: &UnManageArgs) -> HandledResult<()> {
    send_command(cli, &args.resource_id, false).await
}

async fn send_command(cli: &Cli, resource: &str, manage: bool) -> HandledResult<()> {
    tokio::task::LocalSet::new()
        .run_until(async move {
            let client = get_rpc_client(cli).await?;
            let mut request = client.set_managed_request();
            let mut request_args = request.get();
            request_args.set_managed(manage);
            request_args.set_resource(resource);
            let reply = request.send().promise.await.handle_err(|e| {
                eprintln!(
                    "Failed to send {} request: {e}",
                    if manage { "manage" } else { "unmanage" }
                )
            })?;
            let response =
                decode_reply(&reply).handle_err(|e| eprintln!("Failed to decode response: {e}"))?;
            if let Some(error_message) = response {
                eprintln!("{error_message}");
                crate::commands::handled_error()
            } else {
                Ok(())
            }
        })
        .await
}

fn decode_reply(
    reply: &::capnp::capability::Response<set_managed_results::Owned>,
) -> Result<Option<&str>, capnp::Error> {
    let reply = reply.get()?.get_res()?;

    Ok(match reply.which()? {
        command_result::Ok(()) => None,
        command_result::Err(e) => Some(e?.to_str()?),
    })
}

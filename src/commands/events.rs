// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use {clap::Args, reqwest::StatusCode};

use crate::{commands::*, handled_error, Handle, HandledResult};

#[derive(Args, Debug)]
pub struct EventsArgs {
    #[command(subcommand)]
    pub command: EventsCommand,
}

#[derive(Subcommand, Debug)]
pub enum EventsCommand {
    /// Clear the events history so that the `status` command does not print out old events.
    Clear,

    /// Print the entire events history.
    Show,
}

pub fn events(cli: &Cli, args: &EventsArgs) -> HandledResult<()> {
    match args.command {
        EventsCommand::Clear => clear(cli),
        EventsCommand::Show => show(cli),
    }?;

    Ok(())
}

fn clear(cli: &Cli) -> HandledResult<()> {
    let client = get_http_client(cli.socket.as_deref())?;

    let response = client
        .post("http://halo_manager/clear_events")
        .send()
        .handle_err(|e| eprintln!("Error making HTTP request: {e}"))?;

    match response.status() {
        StatusCode::OK => return Ok(()),
        StatusCode::INTERNAL_SERVER_ERROR => {
            eprint!("Could not clear events: ");
            match response.text() {
                Ok(text) => eprintln!("{text}"),
                Err(e) => eprintln!("Error decoding response: {e}"),
            };
        }
        other => eprintln!("Could not clear events: unexpected error: {other}"),
    }

    handled_error()
}

fn show(cli: &Cli) -> HandledResult<()> {
    let cluster = super::status::get_status(cli.socket.as_deref())?;

    for e in cluster.events.iter().filter(|rec| rec.event != "clear") {
        println!("{}", e.syslog_print());
    }

    Ok(())
}

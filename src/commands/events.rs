// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

use crate::{commands::*, HandledResult};

#[derive(Args, Debug)]
pub struct EventsArgs {
    #[command(subcommand)]
    pub command: EventsCommand,
}

#[derive(Subcommand, Debug)]
pub enum EventsCommand {
    /// Print the entire events history.
    Show,
}

pub fn events(cli: &Cli, args: &EventsArgs) -> HandledResult<()> {
    match args.command {
        EventsCommand::Show => show(cli),
    }?;

    Ok(())
}

fn show(cli: &Cli) -> HandledResult<()> {
    let cluster = super::status::get_status(cli.socket.as_deref())?;

    for e in cluster.events {
        println!("{}", e.syslog_print());
    }

    Ok(())
}

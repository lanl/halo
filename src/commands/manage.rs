// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use clap::Args;

//use crate::{commands, resource};
use crate::commands;

#[derive(Args, Debug, Clone)]
pub struct ManageArgs {
    /// Resource to manage
    #[arg(long)]
    resource: String,
}

pub fn manage(_args: &ManageArgs) -> commands::Result{
    todo!("Implement logic");
}
// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

pub mod cluster;
pub mod commands;
pub mod config;
pub mod halo_capnp;
pub mod host;
pub mod manager;
pub mod remote;
pub mod resource;
pub mod state;
pub mod test_env;
pub mod tls;

/// Gets the port that the remote server should be listening on.
pub fn remote_port() -> u16 {
    match std::env::var("HALO_PORT") {
        Ok(port) => port
            .parse::<u16>()
            .expect("HALO_PORT must be a valid port number"),
        Err(_) => 8000,
    }
}

pub fn default_socket() -> String {
    match std::env::var("HALO_SOCKET") {
        Ok(sock) => sock,
        Err(_) => "/var/run/halo.socket".to_string(),
    }
}

pub fn default_config_path() -> String {
    match std::env::var("HALO_CONFIG") {
        Ok(conf) => conf,
        Err(_) => "/etc/halo/halo.conf".to_string(),
    }
}

pub fn default_statefile_path() -> String {
    match std::env::var("HALO_STATEFILE") {
        Ok(statefile) => statefile,
        Err(_) => "/etc/halo/halo.state".to_string(),
    }
}

pub fn default_server_cert() -> String {
    match std::env::var("HALO_SERVER_CERT") {
        Ok(cert) => cert,
        Err(_) => "/etc/halo/server.crt".to_string(),
    }
}

pub fn default_server_key() -> String {
    match std::env::var("HALO_SERVER_KEY") {
        Ok(key) => key,
        Err(_) => "/etc/halo/server.key".to_string(),
    }
}

pub fn default_client_cert() -> String {
    match std::env::var("HALO_CLIENT_CERT") {
        Ok(cert) => cert,
        Err(_) => "/etc/halo/client.crt".to_string(),
    }
}

pub fn default_client_key() -> String {
    match std::env::var("HALO_CLIENT_KEY") {
        Ok(key) => key,
        Err(_) => "/etc/halo/client.key".to_string(),
    }
}

pub fn default_ca_cert() -> String {
    match std::env::var("HALO_CA_CERT") {
        Ok(cert) => cert,
        Err(_) => "/etc/halo/ca.crt".to_string(),
    }
}

pub fn default_network() -> String {
    match std::env::var("HALO_NET") {
        Ok(net) => net,
        Err(_) => "192.168.1.0/24".to_string(),
    }
}

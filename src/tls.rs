// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{fs::File, io::BufReader, path::PathBuf, sync::Arc};

use {
    rustls::{
        pki_types::{CertificateDer, PrivateKeyDer},
        server::WebPkiClientVerifier,
        ClientConfig, RootCertStore, ServerConfig,
    },
    rustls_pemfile::{certs, private_key},
    tokio_rustls::{TlsAcceptor, TlsConnector},
};

fn load_private_key(path: PathBuf) -> Result<PrivateKeyDer<'static>, Box<dyn std::error::Error>> {
    let key_file = File::open(path)?;
    let mut reader = BufReader::new(key_file);
    private_key(&mut reader)?.ok_or_else(|| "No private key found".into())
}

fn load_cert(path: PathBuf) -> Vec<CertificateDer<'static>> {
    let cert_file = &mut BufReader::new(File::open(path).unwrap());
    let certs: Vec<CertificateDer<'static>> = certs(cert_file)
        .collect::<Result<_, _>>()
        .expect("Issue getting certs");
    certs
}

pub fn get_acceptor() -> TlsAcceptor {
    // Load server certificate and private key
    let server_cert = load_cert(PathBuf::from(crate::default_server_cert()));
    let server_key = load_private_key(PathBuf::from(crate::default_server_key()));

    // Load CA root certificate
    let ca_cert = load_cert(PathBuf::from(crate::default_ca_cert()));

    // Load CA cert into root store, I.E. trust it
    let mut root_store = RootCertStore::empty();
    root_store.add_parsable_certificates(ca_cert);

    // Create a client certificiate verifier, mTLS part of the code
    let client_verifier = WebPkiClientVerifier::builder(Arc::new(root_store))
        .build()
        .expect("Failure to build client verifier");

    // Build server config
    let config = ServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_single_cert(server_cert, server_key.unwrap())
        .unwrap();

    // return TLS acceptor
    TlsAcceptor::from(Arc::new(config))
}

pub fn get_connector() -> TlsConnector {
    // Load cient certificate adn private key
    let client_cert = load_cert(PathBuf::from(crate::default_client_cert()));
    let client_key = load_private_key(PathBuf::from(crate::default_client_key()));

    // Load CA root certificate
    let ca_cert = load_cert(PathBuf::from(crate::default_ca_cert()));

    // Load the CA cert into the root store, I.E. trust it
    let mut root_store = RootCertStore::empty();
    root_store.add_parsable_certificates(ca_cert);

    // Build client config
    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_client_auth_cert(client_cert, client_key.unwrap())
        .unwrap();

    // Return TLS connector
    TlsConnector::from(Arc::new(config))
}

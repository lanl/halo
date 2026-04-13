// SPDX-License-Identifier: MIT
// Copyright 2025. Triad National Security, LLC.

use std::{fs::File, io::BufReader, sync::Arc};

use {
    rustls::{
        pki_types::{CertificateDer, PrivateKeyDer},
        server::WebPkiClientVerifier,
        ClientConfig, RootCertStore, ServerConfig,
    },
    rustls_pemfile::{certs, private_key},
    tokio_rustls::{TlsAcceptor, TlsConnector},
};

use crate::commands::{handled_error, Handle, HandledResult};

fn load_private_key(path: &str) -> HandledResult<PrivateKeyDer<'static>> {
    let key_file =
        File::open(path).handle_err(|e| eprintln!("Could not open private key '{path}': {e}"))?;
    let mut reader = BufReader::new(key_file);

    let key =
        private_key(&mut reader).handle_err(|e| eprintln!("Could not create private key: {e}"))?;

    match key {
        Some(key) => Ok(key),
        None => {
            eprintln!("No private key found in file '{path}'.");
            handled_error()
        }
    }
}

fn load_cert(path: &str) -> HandledResult<Vec<CertificateDer<'static>>> {
    let cert_file = &mut BufReader::new(File::open(path).handle_err(|e| {
        eprintln!("Could not open certificate '{path}': {e}");
    })?);

    let certs: Vec<CertificateDer<'static>> = certs(cert_file)
        .collect::<Result<_, _>>()
        .handle_err(|e| eprintln!("Could not create certificates: {e}"))?;

    Ok(certs)
}

pub fn get_acceptor() -> HandledResult<TlsAcceptor> {
    // Load server certificate and private key
    let server_cert = load_cert(&crate::default_server_cert())?;
    let server_key = load_private_key(&crate::default_server_key())?;

    // Load CA root certificate
    let ca_cert = load_cert(&crate::default_ca_cert())?;

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
        .with_single_cert(server_cert, server_key)
        .unwrap();

    // return TLS acceptor
    Ok(TlsAcceptor::from(Arc::new(config)))
}

pub fn get_connector() -> HandledResult<TlsConnector> {
    let client_cert = load_cert(&crate::default_client_cert())?;
    let client_key = load_private_key(&crate::default_client_key())?;

    let ca_cert = load_cert(&crate::default_ca_cert())?;

    // Trust the CA cert
    let mut root_store = RootCertStore::empty();
    root_store.add_parsable_certificates(ca_cert);

    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_client_auth_cert(client_cert, client_key)
        .unwrap();

    Ok(TlsConnector::from(Arc::new(config)))
}

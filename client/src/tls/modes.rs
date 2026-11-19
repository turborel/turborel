//! \[DESIGN\] TLS by mode (RULES.md D204), with the feature `tls`: the
//! client's TLS for each `TlsMode`. `prefer` and `require` encrypt and
//! check that the server holds the key of the certificate it presents,
//! but not who signed it, so they keep out an eavesdropper and not a
//! server in the middle; `verify-ca` checks the certificate against the
//! authorities trusted, not its name; `verify-full` checks both, as
//! `tls::trusting` does. Each presents the client's certificate if one is
//! given.

use std::sync::Arc;

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, SignatureScheme};

use crate::connection::{Result, failed};
use crate::settings::{TlsFiles, TlsMode};

use super::{builder, roots, unreadable};

/// The cryptography the client's TLS is made of: aws-lc-rs's (RULES.md
/// D26).
fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// \[DESIGN\] The verifier of `prefer` and `require`: any certificate,
/// the handshake's signatures checked against its key.
#[derive(Debug)]
struct Unchecked(Arc<CryptoProvider>);

impl ServerCertVerifier for Unchecked {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// \[DESIGN\] The verifier of `verify-ca`: the certificate checked as
/// `verify-full` checks it, but a name it is not of taken.
#[derive(Debug)]
struct AnyName(Arc<WebPkiServerVerifier>);

/// Whether `e` is a certificate's refusal for the name alone.
fn of_another_name(e: &rustls::Error) -> bool {
    matches!(e, rustls::Error::InvalidCertificate(CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. }))
}

impl ServerCertVerifier for AnyName {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let checked = self.0.verify_server_cert(end_entity, intermediates, server_name, ocsp, now);
        checked.or_else(|e| if of_another_name(&e) { Ok(ServerCertVerified::assertion()) } else { Err(e) })
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_verify_schemes()
    }
}

/// The verifier of the authorities `files` trust.
fn trusted(files: &TlsFiles) -> Result<Arc<WebPkiServerVerifier>> {
    let store = roots(files.roots.as_deref(), files.system_roots)?;
    let built = WebPkiServerVerifier::builder_with_provider(Arc::new(store), provider()).build();
    built.map_err(|e| failed(&format!("[DESIGN] cannot make TLS: {e}")))
}

/// The client's certificate chain and key `files` name, if both are.
type Identity = (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>);

/// The identity `files` name: none if they name no certificate.
fn identity(files: &TlsFiles) -> Result<Option<Identity>> {
    let Some(chain) = files.certificate.as_deref() else { return Ok(None) };
    let key = files.key.as_deref().ok_or_else(|| failed("[DESIGN] a client certificate is presented with its key, and none is named"))?;
    let certificates = CertificateDer::pem_file_iter(chain).map_err(unreadable(chain))?;
    let certificates = certificates.collect::<std::result::Result<Vec<_>, _>>().map_err(unreadable(chain))?;
    Ok(Some((certificates, PrivateKeyDer::from_pem_file(key).map_err(unreadable(key))?)))
}

/// The client's TLS of `mode`, trusting and presenting what `files` name:
/// none of `TlsMode::Disable`.
///
/// # Errors
///
/// A connection failure if a file cannot be read, `verify-ca` or
/// `verify-full` is asked for with no authority trusted, or rustls
/// cannot make it.
pub fn config(mode: TlsMode, files: &TlsFiles) -> Result<Option<Arc<ClientConfig>>> {
    let verifier: Arc<dyn ServerCertVerifier> = match mode {
        TlsMode::Disable => return Ok(None),
        TlsMode::Prefer | TlsMode::Require => Arc::new(Unchecked(provider())),
        TlsMode::VerifyCa => Arc::new(AnyName(trusted(files)?)),
        TlsMode::VerifyFull => trusted(files)?,
    };
    let wants = builder()?.dangerous().with_custom_certificate_verifier(verifier);
    let config = match identity(files)? {
        Some((chain, key)) => {
            wants.with_client_auth_cert(chain, key).map_err(|e| failed(&format!("[DESIGN] cannot present the certificate: {e}")))?
        }
        None => wants.with_no_client_auth(),
    };
    Ok(Some(Arc::new(config)))
}

//! The host's TLS identity (PROTOCOL §2): a self-signed EC P-256
//! certificate generated once and persisted, presented with rustls (ring
//! provider). Phones pin its SHA-256 fingerprint at pairing, so it must never
//! change silently.

use std::fmt;
use std::fs;
use std::sync::Arc;

use rcgen::{CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sidecast_core::security::certificate_fingerprint;

use crate::data::{DataDir, write_private};

#[derive(Debug)]
pub enum TlsError {
    Generate(rcgen::Error),
    Io(std::io::Error),
    Rustls(rustls::Error),
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TlsError::Generate(e) => write!(f, "cannot generate the TLS certificate: {e}"),
            TlsError::Io(e) => write!(f, "cannot store the TLS identity: {e}"),
            TlsError::Rustls(e) => write!(f, "invalid TLS identity: {e}"),
        }
    }
}

impl std::error::Error for TlsError {}

/// Certificate + private key, both DER.
#[derive(Clone)]
pub struct TlsIdentity {
    certificate_der: Vec<u8>,
    private_key_der: Vec<u8>,
    fingerprint: [u8; 32],
}

impl fmt::Debug for TlsIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsIdentity")
            .field("fingerprint", &sidecast_core::security::to_hex(&self.fingerprint))
            .finish_non_exhaustive()
    }
}

impl TlsIdentity {
    const VALIDITY_YEARS: i32 = 20;

    /// Loads the identity from `dir`, or creates and saves a new one. An
    /// unreadable identity is replaced (phones must then pair again), as on the Mac.
    pub fn load_or_create(dir: &DataDir) -> Result<TlsIdentity, TlsError> {
        if let (Ok(cert), Ok(key)) = (fs::read(dir.certificate_path()), fs::read(dir.private_key_path())) {
            let identity = Self::from_der(cert, key);
            match identity.server_config() {
                Ok(_) => return Ok(identity),
                Err(e) => log::error!("TLS identity unreadable ({e}); generating a new one — phones must pair again"),
            }
        }
        let identity = Self::generate()?;
        dir.ensure().map_err(TlsError::Io)?;
        write_private(&dir.private_key_path(), &identity.private_key_der).map_err(TlsError::Io)?;
        write_private(&dir.certificate_path(), &identity.certificate_der).map_err(TlsError::Io)?;
        log::info!("generated a new TLS identity in {}", dir.root().display());
        Ok(identity)
    }

    /// A fresh self-signed certificate: CN=Sidecast, P-256/SHA-256, end entity,
    /// digitalSignature, serverAuth, valid for 20 years (like the Mac's).
    pub fn generate() -> Result<TlsIdentity, TlsError> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(TlsError::Generate)?;
        let mut params = CertificateParams::new(Vec::<String>::new()).map_err(TlsError::Generate)?;
        let mut name = DistinguishedName::new();
        name.push(DnType::CommonName, "Sidecast");
        params.distinguished_name = name;
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let year = current_year();
        params.not_before = rcgen::date_time_ymd(year - 1, 1, 1);
        params.not_after = rcgen::date_time_ymd(year + Self::VALIDITY_YEARS, 1, 1);
        let certificate = params.self_signed(&key).map_err(TlsError::Generate)?;
        Ok(Self::from_der(certificate.der().to_vec(), key.serialize_der()))
    }

    fn from_der(certificate_der: Vec<u8>, private_key_der: Vec<u8>) -> TlsIdentity {
        let fingerprint = certificate_fingerprint(&certificate_der);
        TlsIdentity { certificate_der, private_key_der, fingerprint }
    }

    /// SHA-256 of the certificate DER: what phones pin and prove the pairing code over.
    pub fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }

    pub fn certificate_der(&self) -> &[u8] {
        &self.certificate_der
    }

    /// TLS 1.3 preferred, 1.2 minimum, no client certificates.
    pub fn server_config(&self) -> Result<Arc<rustls::ServerConfig>, TlsError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .map_err(TlsError::Rustls)?
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(self.certificate_der.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.private_key_der.clone())),
            )
            .map_err(TlsError::Rustls)?;
        Ok(Arc::new(config))
    }
}

/// Gregorian year from the system clock (exact enough for certificate validity).
fn current_year() -> i32 {
    let days = (crate::clock::unix_now() / 86_400) as i64;
    // Howard Hinnant's civil_from_days, year only.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_persisted_and_reloaded_unchanged() {
        let root = std::env::temp_dir().join(format!(
            "sidecast-tls-{}",
            sidecast_core::security::uuid_v4(&mut sidecast_core::security::SystemRandom)
        ));
        let dir = DataDir::new(&root);
        let first = TlsIdentity::load_or_create(&dir).unwrap();
        let again = TlsIdentity::load_or_create(&dir).unwrap();
        assert_eq!(first.fingerprint(), again.fingerprint());
        assert_eq!(first.fingerprint(), certificate_fingerprint(again.certificate_der()));
        assert!(first.server_config().is_ok());
        // A damaged key is replaced by a new identity.
        fs::write(dir.private_key_path(), b"junk").unwrap();
        assert_ne!(TlsIdentity::load_or_create(&dir).unwrap().fingerprint(), first.fingerprint());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn current_year_is_plausible() {
        assert!((2025..2200).contains(&current_year()));
    }
}

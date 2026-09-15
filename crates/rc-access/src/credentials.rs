use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use fs2::FileExt;
use rand::{rngs::OsRng, RngCore};
use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use ring::signature::{
    EcdsaKeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_ASN1, ECDSA_P256_SHA256_ASN1_SIGNING,
};
use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer},
    ClientConfig, RootCertStore, ServerConfig,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;

pub const SERVER_NAME: &str = "shunyi-device.local";
const STATE_VERSION: u8 = 1;
const MAX_FILE: u64 = 128 * 1024;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn random_secret() -> String {
    let mut b = [0u8; 24];
    OsRng.fill_bytes(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}
pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn certs(pem: &str) -> Result<Vec<CertificateDer<'static>>> {
    let certs =
        rustls_pemfile::certs(&mut pem.as_bytes()).collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(!certs.is_empty() && certs.len() <= 3, "证书内容无效");
    Ok(certs)
}
fn private_key(pem: &str) -> Result<PrivateKeyDer<'static>> {
    rustls_pemfile::private_key(&mut pem.as_bytes())?.context("缺少私钥")
}
pub fn device_id(ca_pem: &str) -> Result<String> {
    Ok(format!("rc-{}", digest(certs(ca_pem)?[0].as_ref())))
}
pub fn validate_identity(expected: &str, ca_pem: &str) -> Result<()> {
    ensure!(
        expected.len() == 67 && device_id(ca_pem)? == expected,
        "设备证书与设备 ID 不匹配，已拒绝连接"
    );
    Ok(())
}

/// This export contains a private key. Never put its contents in host-list exports or logs.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessCertificate {
    pub version: u8,
    pub device_id: String,
    pub ca_pem: String,
    pub certificate_pem: String,
    pub private_key_pem: String,
}
impl AccessCertificate {
    pub fn read(path: &Path) -> Result<Self> {
        let value: Self = serde_json::from_slice(&read_private(path)?)?;
        ensure!(value.version == STATE_VERSION, "不支持的连接证书版本");
        validate_identity(&value.device_id, &value.ca_pem)?;
        value.client_config()?;
        Ok(value)
    }
    pub fn client_config(&self) -> Result<Arc<ClientConfig>> {
        let builder = client_builder(&self.ca_pem)?;
        Ok(Arc::new(builder.with_client_auth_cert(
            certs(&self.certificate_pem)?,
            private_key(&self.private_key_pem)?,
        )?))
    }
}
fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}
fn client_builder(
    ca: &str,
) -> Result<rustls::ConfigBuilder<ClientConfig, rustls::client::WantsClientCert>> {
    let mut roots = RootCertStore::empty();
    for cert in certs(ca)? {
        roots.add(cert)?;
    }
    Ok(ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(roots))
}
pub fn temporary_client_config(device: &str, ca: &str) -> Result<Arc<ClientConfig>> {
    validate_identity(device, ca)?;
    Ok(Arc::new(client_builder(ca)?.with_no_client_auth()))
}

#[derive(Clone, Serialize, Deserialize)]
struct Temporary {
    salt: String,
    hash: String,
    expires_at: u64,
    used: bool,
}
#[derive(Clone, Serialize, Deserialize)]
struct State {
    version: u8,
    ca_pem: String,
    ca_key: String,
    server_pem: String,
    server_key: String,
    access: AccessCertificate,
    rotated_at: u64,
    rotation_hours: Option<u64>,
    temporary: Option<Temporary>,
}
#[derive(Clone)]
pub struct CredentialStore {
    root: PathBuf,
}
#[derive(Serialize)]
pub struct CredentialStatus {
    pub device_id: String,
    pub certificate_fingerprint: String,
    pub certificate_rotated_at: u64,
    pub rotation_hours: Option<u64>,
    pub temporary_state: String,
    pub temporary_expires_at: Option<u64>,
}

impl CredentialStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let store = Self { root: root.into() };
        if !store.root.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&store.root)?;
            }
            #[cfg(not(unix))]
            fs::create_dir_all(&store.root)?;
        }
        check_private(&store.root, true)?;
        let lock = store.lock()?;
        if !store.path().exists() {
            store.save(&new_state()?)?;
        } else {
            let state = store.load()?;
            ensure!(state.version == STATE_VERSION, "设备凭据版本不受支持");
        }
        drop(lock);
        Ok(store)
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn path(&self) -> PathBuf {
        self.root.join("identity.json")
    }
    fn lock(&self) -> Result<File> {
        let file = secure_open(&self.root.join("credentials.lock"), false)?;
        file.lock_exclusive()?;
        Ok(file)
    }
    fn load(&self) -> Result<State> {
        Ok(serde_json::from_slice(&read_private(&self.path())?)?)
    }
    fn save(&self, state: &State) -> Result<()> {
        let path = self.root.join(format!(".identity-{}.tmp", random_secret()));
        let bytes = serde_json::to_vec_pretty(state)?;
        let result = (|| {
            let mut file = secure_open(&path, true)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&path, self.path())?;
            File::open(&self.root)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&path);
        }
        result
    }
    /// Keep this guard for the lifetime of an agent, including reconnects.
    pub fn lock_agent(&self) -> Result<File> {
        let file = secure_open(&self.root.join("agent.lock"), false)?;
        file.try_lock_exclusive()
            .context("同一设备已经有 Agent 在运行")?;
        Ok(file)
    }
    pub fn status(&self) -> Result<CredentialStatus> {
        let _lock = self.lock()?;
        let s = self.load()?;
        Ok(CredentialStatus {
            device_id: s.access.device_id,
            certificate_fingerprint: digest(certs(&s.access.certificate_pem)?[0].as_ref()),
            certificate_rotated_at: s.rotated_at,
            rotation_hours: s.rotation_hours,
            temporary_state: match &s.temporary {
                None => "none",
                Some(t) if t.used => "used",
                Some(t) if t.expires_at <= now() => "expired",
                Some(_) => "unused",
            }
            .into(),
            temporary_expires_at: s.temporary.map(|t| t.expires_at),
        })
    }
    pub fn public_identity(&self) -> Result<(String, String)> {
        let _lock = self.lock()?;
        let s = self.load()?;
        Ok((s.access.device_id, s.ca_pem))
    }
    pub fn export_certificate(&self, destination: &Path) -> Result<()> {
        let _lock = self.lock()?;
        let s = self.load()?;
        let mut file = secure_open(destination, true)?;
        file.write_all(&serde_json::to_vec_pretty(&s.access)?)?;
        file.sync_all()?;
        Ok(())
    }
    pub fn rotate_certificate(&self) -> Result<()> {
        let _lock = self.lock()?;
        let mut s = self.load()?;
        s.access = new_access(&s.ca_pem, &s.ca_key)?;
        s.rotated_at = now();
        self.save(&s)
    }
    pub fn set_rotation(&self, hours: Option<u64>) -> Result<()> {
        ensure!(
            hours.is_none_or(|v| (1..=8760).contains(&v)),
            "轮换周期需要为 1–8760 小时"
        );
        let _lock = self.lock()?;
        let mut s = self.load()?;
        s.rotation_hours = hours;
        self.save(&s)
    }
    pub fn rotate_if_due(&self) -> Result<bool> {
        let _lock = self.lock()?;
        let mut s = self.load()?;
        if s.rotation_hours
            .is_some_and(|h| now().saturating_sub(s.rotated_at) >= h * 3600)
        {
            s.access = new_access(&s.ca_pem, &s.ca_key)?;
            s.rotated_at = now();
            self.save(&s)?;
            return Ok(true);
        }
        Ok(false)
    }
    pub fn create_temporary(&self, ttl_seconds: u64) -> Result<String> {
        ensure!(
            (60..=86400).contains(&ttl_seconds),
            "临时密码有效期需要为 1 分钟至 24 小时"
        );
        let password = random_secret();
        let salt = random_secret();
        let _lock = self.lock()?;
        let mut s = self.load()?;
        s.temporary = Some(Temporary {
            hash: digest(format!("shunyi-temp-v1:{salt}:{password}").as_bytes()),
            salt,
            expires_at: now() + ttl_seconds,
            used: false,
        });
        self.save(&s)?;
        Ok(password)
    }
    pub fn revoke_temporary(&self) -> Result<()> {
        let _lock = self.lock()?;
        let mut s = self.load()?;
        s.temporary = None;
        self.save(&s)
    }
    /// The one successful claim is committed before sending Authenticated or opening a PTY.
    /// Used credentials never become usable again, including after crashes or restarts.
    pub fn claim_temporary(&self, password: &str) -> Result<()> {
        ensure!(password.len() <= 128, "临时密码无效或已失效");
        let _lock = self.lock()?;
        let mut s = self.load()?;
        let t = s.temporary.as_mut().context("临时密码无效或已失效")?;
        let candidate = digest(format!("shunyi-temp-v1:{}:{password}", t.salt).as_bytes());
        ensure!(
            !t.used
                && t.expires_at > now()
                && bool::from(candidate.as_bytes().ct_eq(t.hash.as_bytes())),
            "临时密码无效或已失效"
        );
        t.used = true;
        self.save(&s)
    }
    pub fn authorize_certificate(&self, der: &[u8]) -> Result<()> {
        let _lock = self.lock()?;
        let s = self.load()?;
        ensure!(
            digest(der) == digest(certs(&s.access.certificate_pem)?[0].as_ref()),
            "连接证书已轮换或不属于此设备"
        );
        Ok(())
    }
    pub fn server_config(&self) -> Result<Arc<ServerConfig>> {
        let _lock = self.lock()?;
        let s = self.load()?;
        let mut roots = RootCertStore::empty();
        for cert in certs(&s.ca_pem)? {
            roots.add(cert)?;
        }
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots),
            provider(),
        )
        .allow_unauthenticated()
        .build()?;
        Ok(Arc::new(
            ServerConfig::builder_with_provider(provider())
                .with_protocol_versions(&[&rustls::version::TLS13])?
                .with_client_cert_verifier(verifier)
                .with_single_cert(certs(&s.server_pem)?, private_key(&s.server_key)?)?,
        ))
    }
    pub fn sign_registration(&self, challenge: &str, name: &str) -> Result<String> {
        let _lock = self.lock()?;
        let s = self.load()?;
        let rng = ring::rand::SystemRandom::new();
        let key = private_key(&s.ca_key)?;
        let pair =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, key.secret_der(), &rng)
                .map_err(|_| anyhow::anyhow!("设备签名密钥无效"))?;
        Ok(B64.encode(
            pair.sign(&rng, &registration_payload(challenge, name))
                .map_err(|_| anyhow::anyhow!("设备签名失败"))?
                .as_ref(),
        ))
    }
}
fn registration_payload(challenge: &str, name: &str) -> Vec<u8> {
    format!("shunyi-agent-registration-v1\0{challenge}\0{name}").into_bytes()
}
pub fn verify_registration(
    id: &str,
    ca: &str,
    challenge: &str,
    name: &str,
    signature: &str,
) -> Result<()> {
    validate_identity(id, ca)?;
    ensure!(
        name.len() <= 128 && ca.len() <= 8192 && signature.len() <= 256,
        "设备信息过长"
    );
    let certificates = certs(ca)?;
    let (_, parsed) = x509_parser::parse_x509_certificate(&certificates[0])
        .map_err(|_| anyhow::anyhow!("设备证书无效"))?;
    UnparsedPublicKey::new(
        &ECDSA_P256_SHA256_ASN1,
        parsed.public_key().subject_public_key.data.as_ref(),
    )
    .verify(
        &registration_payload(challenge, name),
        &B64.decode(signature)?,
    )
    .map_err(|_| anyhow::anyhow!("设备身份签名无效"))
}
fn params(client: bool) -> Result<CertificateParams> {
    let mut p = CertificateParams::new(if client {
        vec![]
    } else {
        vec![SERVER_NAME.into()]
    })?;
    p.not_before = time::OffsetDateTime::now_utc() - time::Duration::days(1);
    p.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(3650);
    p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    p.extended_key_usages = vec![if client {
        ExtendedKeyUsagePurpose::ClientAuth
    } else {
        ExtendedKeyUsagePurpose::ServerAuth
    }];
    Ok(p)
}
fn new_access(ca_pem: &str, ca_key: &str) -> Result<AccessCertificate> {
    let issuer = Issuer::from_ca_cert_pem(ca_pem, KeyPair::from_pem(ca_key)?)?;
    let key = KeyPair::generate()?;
    let cert = params(true)?.signed_by(&key, &issuer)?;
    Ok(AccessCertificate {
        version: STATE_VERSION,
        device_id: device_id(ca_pem)?,
        ca_pem: ca_pem.into(),
        certificate_pem: cert.pem(),
        private_key_pem: key.serialize_pem(),
    })
}
fn new_state() -> Result<State> {
    let key = KeyPair::generate()?;
    let mut p = params(false)?;
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.extended_key_usages.clear();
    p.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::CrlSign,
    ];
    let ca = p.self_signed(&key)?.pem();
    let ca_key = key.serialize_pem();
    let issuer = Issuer::from_ca_cert_pem(&ca, key)?;
    let server_key = KeyPair::generate()?;
    let server = params(false)?.signed_by(&server_key, &issuer)?;
    Ok(State {
        version: STATE_VERSION,
        access: new_access(&ca, &ca_key)?,
        ca_pem: ca,
        ca_key,
        server_pem: server.pem(),
        server_key: server_key.serialize_pem(),
        rotated_at: now(),
        rotation_hours: None,
        temporary: None,
    })
}
fn check_private(path: &Path, directory: bool) -> Result<()> {
    let m = fs::symlink_metadata(path)?;
    ensure!(
        !m.file_type().is_symlink() && m.is_dir() == directory,
        "凭据路径不能是符号链接或特殊文件"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0,
            "凭据必须仅限当前用户访问：请设置目录权限 700、文件权限 600"
        );
    }
    Ok(())
}
fn secure_open(path: &Path, exclusive: bool) -> Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true).write(true);
    if exclusive {
        opts.create_new(true);
    } else {
        opts.create(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = opts.open(path)?;
    check_private(path, false)?;
    Ok(file)
}
fn read_private(path: &Path) -> Result<Vec<u8>> {
    check_private(path, false)?;
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = opts.open(path)?;
    ensure!(file.metadata()?.is_file(), "凭据必须是普通文件");
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE {
        bail!("凭据文件过大");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, CredentialStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::open(dir.path().join("identity")).unwrap();
        (dir, store)
    }
    #[test]
    fn expired_or_replaced_passwords_cannot_be_claimed() {
        let (_dir, store) = fixture();
        let old = store.create_temporary(60).unwrap();
        let new = store.create_temporary(60).unwrap();
        assert!(store.claim_temporary(&old).is_err());
        {
            let _lock = store.lock().unwrap();
            let mut state = store.load().unwrap();
            state.temporary.as_mut().unwrap().expires_at = now() - 1;
            store.save(&state).unwrap();
        }
        assert!(store.claim_temporary(&new).is_err());
        let revoked = store.create_temporary(60).unwrap();
        store.revoke_temporary().unwrap();
        assert!(store.claim_temporary(&revoked).is_err());
    }
    #[test]
    fn signature_binds_device_challenge_and_metadata() {
        let (_dir, store) = fixture();
        let (id, ca) = store.public_identity().unwrap();
        let challenge = random_secret();
        let signature = store.sign_registration(&challenge, "device A").unwrap();
        verify_registration(&id, &ca, &challenge, "device A", &signature).unwrap();
        assert!(verify_registration(&id, &ca, &random_secret(), "device A", &signature).is_err());
        assert!(verify_registration(&id, &ca, &challenge, "device B", &signature).is_err());
        assert!(validate_identity(&format!("rc-{}", "0".repeat(64)), &ca).is_err());
    }
    #[test]
    fn scheduled_rotation_keeps_device_identity_but_revokes_old_certificate() {
        let (_dir, store) = fixture();
        let (id, _) = store.public_identity().unwrap();
        let old_cert = certs(&store.load().unwrap().access.certificate_pem)
            .unwrap()
            .remove(0);
        store.set_rotation(Some(24)).unwrap();
        assert!(!store.rotate_if_due().unwrap());
        {
            let _lock = store.lock().unwrap();
            let mut s = store.load().unwrap();
            s.rotated_at = now() - 25 * 3600;
            store.save(&s).unwrap();
        }
        assert!(store.rotate_if_due().unwrap());
        assert_eq!(store.public_identity().unwrap().0, id);
        assert!(store.authorize_certificate(&old_cert).is_err());
    }
    #[test]
    fn secrets_are_private_and_never_overwrite_exports() {
        let (dir, store) = fixture();
        let path = dir.path().join("access.shunyi-cert");
        store.export_certificate(&path).unwrap();
        AccessCertificate::read(&path).unwrap();
        assert!(store.export_certificate(&path).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let link = dir.path().join("linked.shunyi-cert");
            symlink(&path, &link).unwrap();
            assert!(AccessCertificate::read(&link).is_err());
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(AccessCertificate::read(&path).is_err());
        }
        let lock = store.lock_agent().unwrap();
        assert!(store.lock_agent().is_err());
        drop(lock);
        store.lock_agent().unwrap();
    }
}

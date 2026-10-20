//! Credentials for the local proxy, and the policies that decide how a
//! client's traffic is carried.
//!
//! A program that points at the local proxy is given a credential, and the
//! credential carries a policy: how the proxy underneath is chosen, which
//! exit countries are acceptable, which protocols may be used. One operator
//! can therefore hand out a credential that keeps a session on one exit
//! address for a login, and another that changes the address on every
//! request for a crawler, without either of them touching the engine.
//!
//! The password is generated, shown once, and never stored: what is kept is
//! a salted digest. The password carries enough randomness that a single
//! round of SHA-256 is the right amount of work — there is no dictionary to
//! attack, and the salt rules out a precomputed table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::proxy::ProxyProtocol;

/// How the proxy underneath is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    /// The default: a client keeps one proxy for as long as its session
    /// lasts.
    #[default]
    /// The proxy in front is replaced on a schedule.
    ///
    /// Every request inside the window is carried by the same proxy, and the
    /// next window uses another one. This is what a crawler wants: the exit
    /// address changes, but not in the middle of a request.
    Time,
    /// Every request may be carried by a different proxy.
    ///
    /// The traffic is spread over the pool, and a site that sees the address
    /// change from one request to the next learns nothing from the pattern.
    PerRequest,
    /// A client keeps one proxy for as long as its session lasts.
    ///
    /// A login needs the address to stay the same, and so does anything else
    /// that remembers where it was.
    Sticky,
}

impl std::fmt::Display for Rotation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rotation::Time => write!(f, "time"),
            Rotation::PerRequest => write!(f, "per_request"),
            Rotation::Sticky => write!(f, "sticky"),
        }
    }
}

/// What a credential's traffic is allowed to do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProxyPolicy {
    /// How the proxy underneath is chosen.
    pub rotation: Rotation,
    /// How long a client keeps its proxy, in seconds.
    ///
    /// Only for a sticky session.
    #[serde(default = "default_sticky_secs")]
    pub sticky_secs: u64,
    /// How often the proxy in front is replaced, in seconds.
    ///
    /// Only for rotation by time.
    #[serde(default = "default_rotate_interval_secs")]
    pub rotate_interval_secs: u64,
    /// The countries the exit address may be in.
    ///
    /// ISO 3166-1 alpha-2 codes. Empty means any country is allowed.
    #[serde(default)]
    pub countries: Vec<String>,
    /// The protocols that may be used.
    ///
    /// Empty means any protocol is allowed.
    #[serde(default)]
    pub protocols: Vec<ProxyProtocol>,
    /// The score a proxy has to reach to be used.
    ///
    /// The score comes from the measured latency, so this is a statement
    /// about how slow is too slow.
    #[serde(default)]
    pub min_score: f64,
}

fn default_sticky_secs() -> u64 {
    600
}

fn default_rotate_interval_secs() -> u64 {
    300
}

impl Default for ProxyPolicy {
    fn default() -> Self {
        Self {
            rotation: Rotation::default(),
            sticky_secs: default_sticky_secs(),
            rotate_interval_secs: default_rotate_interval_secs(),
            countries: Vec::new(),
            protocols: Vec::new(),
            min_score: 0.0,
        }
    }
}

impl ProxyPolicy {
    /// Whether a proxy with these properties may be used.
    pub fn allows(&self, proxy: &crate::proxy::ProxyInfo) -> bool {
        if !self.countries.is_empty() {
            let country = proxy.country.as_deref().unwrap_or_default();
            if !self
                .countries
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(country))
            {
                return false;
            }
        }

        if !self.protocols.is_empty() && !self.protocols.contains(&proxy.protocol) {
            return false;
        }

        if proxy.score < self.min_score {
            return false;
        }

        true
    }
}

/// A named policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProxyPolicyRecord {
    /// The name the credential refers to.
    pub name: String,
    /// What the policy allows.
    pub settings: ProxyPolicy,
    /// Whether the policy came with the engine.
    ///
    /// A built-in policy cannot be deleted, but it can be changed: the
    /// operator's decision about what "sticky" means belongs to them.
    pub builtin: bool,
}

/// A credential.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProxyCredential {
    /// The name the client authenticates with.
    pub username: String,
    /// The policy the credential carries.
    pub policy: String,
    /// Whether the credential is still accepted.
    pub enabled: bool,
    /// When the credential was created.
    pub created_at: DateTime<Utc>,
    /// The password, when it was just created.
    ///
    /// It is never stored and never shown again: what the engine keeps is a
    /// digest. The operator copies it into the proxy URL once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// The store the credentials and the policies live in.
///
/// The store keeps its own connection to the database and hands it out one
/// request at a time. The local proxy checks a credential on every
/// connection, so the check has to be cheap: opening the database again for
/// every request is not.
pub struct CredentialStore {
    storage: tokio::sync::Mutex<crate::storage::Storage>,
    /// The names of the credentials, kept for a short while.
    ///
    /// A client's name is read on every connection, and the names are
    /// needed to tell the credential from the parameters behind it. The list
    /// changes rarely, so it is kept for a minute rather than read again for
    /// every connection.
    names: tokio::sync::RwLock<(Vec<String>, std::time::Instant)>,
}

impl CredentialStore {
    /// Open the store on a database file.
    pub fn open(path: &std::path::Path) -> anyhow::Result<Self> {
        Ok(Self {
            storage: tokio::sync::Mutex::new(crate::storage::Storage::open(path)?),
            names: tokio::sync::RwLock::new((Vec::new(), std::time::Instant::now())),
        })
    }

    /// Make sure the built-in policies are there.
    pub async fn ensure_builtin_policies(&self) -> anyhow::Result<()> {
        let storage = self.storage.lock().await;
        for policy in builtin_policies() {
            if storage.policy(&policy.name)?.is_none() {
                storage.save_policy(&policy)?;
            }
        }
        Ok(())
    }

    /// Every policy.
    pub async fn list_policies(&self) -> anyhow::Result<Vec<ProxyPolicyRecord>> {
        let storage = self.storage.lock().await;
        storage.list_policies()
    }

    /// One policy.
    pub async fn policy(&self, name: &str) -> anyhow::Result<Option<ProxyPolicyRecord>> {
        let storage = self.storage.lock().await;
        storage.policy(name)
    }

    /// Write a policy down.
    pub async fn save_policy(&self, policy: &ProxyPolicyRecord) -> anyhow::Result<()> {
        let storage = self.storage.lock().await;
        storage.save_policy(policy)
    }

    /// Remove a policy.
    pub async fn delete_policy(&self, name: &str) -> anyhow::Result<bool> {
        let storage = self.storage.lock().await;
        storage.delete_policy(name)
    }

    /// The names of the credentials.
    ///
    /// The list is kept for a minute: a client's name is read on every
    /// connection, and the names are needed to tell the credential from the
    /// parameters behind it.
    pub async fn credential_names(&self) -> Vec<String> {
        {
            let cached = self.names.read().await;
            // An empty list means the names have not been read yet.
            if !cached.0.is_empty() && cached.1.elapsed() < std::time::Duration::from_secs(60) {
                return cached.0.clone();
            }
        }

        let names: Vec<String> = self
            .list_credentials()
            .await
            .map(|credentials| credentials.into_iter().map(|c| c.username).collect())
            .unwrap_or_default();

        *self.names.write().await = (names.clone(), std::time::Instant::now());
        names
    }

    /// Every credential, without its secret.
    pub async fn list_credentials(&self) -> anyhow::Result<Vec<ProxyCredential>> {
        let storage = self.storage.lock().await;
        storage.list_credentials()
    }

    /// Create a credential, and return it with the password that was made
    /// for it.
    ///
    /// The password is shown once and never stored: the engine keeps a
    /// digest of it.
    pub async fn create_credential(
        &self,
        username: &str,
        policy: &str,
    ) -> anyhow::Result<ProxyCredential> {
        let password = generate_password();
        let salt = generate_salt();
        let hash = digest(&salt, &password);

        let credential = ProxyCredential {
            username: username.to_string(),
            policy: policy.to_string(),
            enabled: true,
            created_at: Utc::now(),
            password: Some(password),
        };

        let storage = self.storage.lock().await;
        storage.save_credential(&credential, &salt, &hash)?;
        drop(storage);

        // The name of a new credential has to be known at once: a client
        // that authenticates with it must not be refused while the list of
        // the names is still the old one.
        self.forget_names().await;

        Ok(credential)
    }

    /// Remove a credential.
    pub async fn delete_credential(&self, username: &str) -> anyhow::Result<bool> {
        let storage = self.storage.lock().await;
        let removed = storage.delete_credential(username);
        drop(storage);

        self.forget_names().await;

        removed
    }

    /// Forget the list of the names, so it is read again.
    async fn forget_names(&self) {
        *self.names.write().await = (Vec::new(), std::time::Instant::now());
    }

    /// The policy a credential carries, when the password is right.
    pub async fn verify(&self, username: &str, password: &str) -> Option<ProxyPolicyRecord> {
        let storage = self.storage.lock().await;
        storage.verify_credential(username, password).ok().flatten()
    }
}

/// What a client calls itself, and what else the name carries.
///
/// The name a proxy client sends is not only the name of the credential: it
/// also says which session the client wants. This is how the proxy services
/// that are worth using have always done it — the name carries the
/// parameters, separated by dashes, and the password stays the secret of the
/// account:
///
/// ```text
/// http://work-session-3:secret@127.0.0.1:9098   a third session of "work"
/// http://work-session-4:secret@127.0.0.1:9098   a fourth, with another address
/// ```
///
/// One credential therefore carries as many sessions as the client asks for,
/// each with an address of its own, and the sessions expire on their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientName {
    /// The credential the name belongs to.
    pub credential: String,
    /// The parameters the name carries, as they were written.
    pub parameters: Vec<(String, String)>,
}

impl ClientName {
    /// The value of a parameter, if the name carries it.
    pub fn parameter(&self, key: &str) -> Option<&str> {
        self.parameters
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    /// The session the client asked for.
    ///
    /// A client that asks for nothing is one session of its own.
    pub fn session(&self) -> &str {
        self.parameter("session").unwrap_or("default")
    }

    /// The key the session is held under.
    ///
    /// The credential is part of it: two credentials that ask for the same
    /// session name are still two accounts, and their traffic is held apart.
    pub fn session_key(&self) -> String {
        format!("{}:{}", self.credential, self.session())
    }
}

/// The parameters a client's name may carry.
pub const PARAMETER_KEYS: [&str; 4] = ["session", "country", "protocol", "sticky"];

/// Read the parameters out of the tail of a client's name.
///
/// The parameters come in pairs, the key and the value separated by a dash:
/// `session-3`, `country-de`. A value may carry a dash of its own, so a part
/// that is not a key belongs to the value before it.
fn parse_parameters(rest: &str) -> Vec<(String, String)> {
    let mut parameters: Vec<(String, String)> = Vec::new();
    let mut current: Option<String> = None;

    for part in rest.split('-').filter(|part| !part.is_empty()) {
        // A part that names a parameter starts a new one. The value of the
        // parameter before it is finished.
        if PARAMETER_KEYS
            .iter()
            .any(|key| key.eq_ignore_ascii_case(part))
        {
            if let Some(key) = current.take() {
                parameters.push((key, String::new()));
            }
            current = Some(part.to_string());
            continue;
        }

        match current.take() {
            Some(key) => parameters.push((key, part.to_string())),
            None => parameters.push(("session".to_string(), part.to_string())),
        }
    }

    if let Some(key) = current.take() {
        parameters.push((key, String::new()));
    }

    parameters
}

/// Read a client's name: the credential it belongs to, and what else it says.
///
/// The credential is the longest known name the client's name starts with.
/// A name that matches nothing is taken as it stands, so the check that
/// follows can say so in one place.
pub fn parse_client_name(username: &str, known: &[String]) -> ClientName {
    let name = username.trim();

    let credential = known
        .iter()
        .filter(|candidate| {
            name == candidate.as_str()
                || (name.len() > candidate.len()
                    && name.starts_with(candidate.as_str())
                    && name.as_bytes()[candidate.len()] == b'-')
        })
        .max_by_key(|candidate| candidate.len())
        .cloned()
        .unwrap_or_else(|| name.to_string());

    let rest = name
        .strip_prefix(credential.as_str())
        .unwrap_or_default()
        .trim_start_matches('-');

    let parameters = parse_parameters(rest);

    ClientName {
        credential,
        parameters,
    }
}

/// The built-in policies, and the names they are known by.
pub const POLICY_DEFAULT: &str = "default";
pub const POLICY_ROTATING: &str = "rotating";
pub const POLICY_TIMED: &str = "timed";

/// The policies every engine starts with.
pub fn builtin_policies() -> Vec<ProxyPolicyRecord> {
    vec![
        ProxyPolicyRecord {
            name: POLICY_DEFAULT.to_string(),
            settings: ProxyPolicy {
                rotation: Rotation::Sticky,
                sticky_secs: 600,
                ..ProxyPolicy::default()
            },
            builtin: true,
        },
        ProxyPolicyRecord {
            name: POLICY_ROTATING.to_string(),
            settings: ProxyPolicy {
                rotation: Rotation::PerRequest,
                ..ProxyPolicy::default()
            },
            builtin: true,
        },
        ProxyPolicyRecord {
            name: POLICY_TIMED.to_string(),
            settings: ProxyPolicy {
                rotation: Rotation::Time,
                rotate_interval_secs: 300,
                ..ProxyPolicy::default()
            },
            builtin: true,
        },
    ]
}

/// Generate a password for a new credential.
///
/// The alphabet leaves out the characters that are easy to confuse, because
/// the password is copied by hand into a proxy URL.
pub fn generate_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut password = String::with_capacity(24);
    for _ in 0..24 {
        let index = rand::random_range(0..ALPHABET.len());
        password.push(ALPHABET[index] as char);
    }
    password
}

/// A random salt, as hexadecimal.
pub fn generate_salt() -> String {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The digest of a password under a salt.
pub fn digest(salt: &str, password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(password.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether a password matches a stored digest.
///
/// The comparison takes the same time whatever the answer is, so a caller
/// cannot learn the digest by watching how long the check took.
pub fn verify(salt: &str, password: &str, expected: &str) -> bool {
    let actual = digest(salt, password);
    constant_time_eq(actual.as_bytes(), expected.as_bytes())
}

/// Compare two byte strings without an early exit.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        difference |= a ^ b;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::{Anonymity, ProxyInfo};

    fn proxy(country: Option<&str>, protocol: ProxyProtocol, score: f64) -> ProxyInfo {
        ProxyInfo {
            id: "id".to_string(),
            host: "198.51.100.1".to_string(),
            port: 8080,
            protocol,
            anonymity: Anonymity::Unknown,
            latency_ms: Some(100),
            country: country.map(|c| c.into()),
            country_reported: None,
            last_checked: None,
            last_success: None,
            first_seen: Utc::now(),
            next_probe_at: Utc::now(),
            password: None,
            supports_tunnel: true,
            exit_ip: None,
            consecutive_failures: 0,
            score,
        }
    }

    #[test]
    fn a_password_is_checked_against_its_digest() {
        let salt = generate_salt();
        let password = generate_password();
        let stored = digest(&salt, &password);

        assert!(verify(&salt, &password, &stored));
        assert!(!verify(&salt, "another", &stored));
        assert!(!verify(&generate_salt(), &password, &stored));
    }

    #[test]
    fn a_client_name_carries_its_session() {
        let known = vec!["work".to_string(), "work-session-1".to_string()];
        let name = parse_client_name("work-session-3", &known);

        assert_eq!(name.credential, "work");
        assert_eq!(name.session(), "3");
        assert_eq!(name.session_key(), "work:3");
    }

    #[test]
    fn the_longest_known_name_wins() {
        // Two credentials where one is the prefix of the other.
        let known = vec!["work".to_string(), "work-eu".to_string()];
        let name = parse_client_name("work-eu-session-2", &known);

        assert_eq!(name.credential, "work-eu");
        assert_eq!(name.session(), "2");
    }

    #[test]
    fn a_client_without_a_session_has_the_default_one() {
        let known = vec!["work".to_string()];
        let name = parse_client_name("work", &known);

        assert_eq!(name.credential, "work");
        assert_eq!(name.session(), "default");
    }

    #[test]
    fn a_name_that_matches_nothing_is_taken_as_it_stands() {
        let known = vec!["work".to_string()];
        let name = parse_client_name("stranger", &known);

        assert_eq!(name.credential, "stranger");
    }

    #[test]
    fn a_policy_limits_the_countries() {
        let policy = ProxyPolicy {
            countries: vec!["DE".to_string()],
            ..ProxyPolicy::default()
        };

        assert!(policy.allows(&proxy(Some("DE"), ProxyProtocol::Socks5, 1.0)));
        assert!(!policy.allows(&proxy(Some("NL"), ProxyProtocol::Socks5, 1.0)));
        assert!(!policy.allows(&proxy(None, ProxyProtocol::Socks5, 1.0)));
    }

    #[test]
    fn a_policy_limits_the_protocols() {
        let policy = ProxyPolicy {
            protocols: vec![ProxyProtocol::Socks5],
            ..ProxyPolicy::default()
        };

        assert!(policy.allows(&proxy(Some("DE"), ProxyProtocol::Socks5, 1.0)));
        assert!(!policy.allows(&proxy(Some("DE"), ProxyProtocol::Http, 1.0)));
    }

    #[test]
    fn an_empty_policy_allows_everything() {
        let policy = ProxyPolicy::default();
        assert!(policy.allows(&proxy(Some("NL"), ProxyProtocol::Http, 0.0)));
        assert!(policy.allows(&proxy(None, ProxyProtocol::Trojan, 0.0)));
    }
}

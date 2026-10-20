use crate::proxy::{Anonymity, ProxyInfo, ProxyProtocol};
use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

pub struct Storage {
    conn: Connection,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS proxies (
                id TEXT PRIMARY KEY,
                host TEXT NOT NULL,
                port INTEGER NOT NULL,
                protocol TEXT NOT NULL,
                anonymity TEXT NOT NULL,
                latency_ms INTEGER,
                country TEXT,
                score REAL NOT NULL DEFAULT 0,
                last_checked TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS sources (
                url TEXT PRIMARY KEY,
                enabled INTEGER NOT NULL DEFAULT 1,
                last_scraped TEXT
            );
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS proxy_policies (
                name TEXT PRIMARY KEY,
                settings TEXT NOT NULL,
                builtin INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS proxy_credentials (
                username TEXT PRIMARY KEY,
                salt TEXT NOT NULL,
                password_hash TEXT NOT NULL,
                policy TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )?;

        // The protocol a source is about was added later, so an existing
        // database gains the column here.
        if !self.has_column("sources", "protocol")? {
            self.conn
                .execute_batch("ALTER TABLE sources ADD COLUMN protocol TEXT")?;
        }

        // A pool restored from disk is only useful if it knows what has already
        // been confirmed. Without `last_success` every proxy reads as unverified
        // on boot and the whole pool is re-probed from scratch after each
        // restart; without the failure streak the eviction policy starts over.
        //
        // SQLite has no `ADD COLUMN IF NOT EXISTS`, so each column is added only
        // when it is genuinely missing — otherwise reopening the database fails.
        for (column, definition) in [
            ("last_success", "TEXT"),
            ("consecutive_failures", "INTEGER NOT NULL DEFAULT 0"),
            ("first_seen", "TEXT"),
            ("next_probe_at", "TEXT"),
            ("exit_ip", "TEXT"),
            ("password", "TEXT"),
            ("country_reported", "TEXT"),
            ("supports_tunnel", "INTEGER NOT NULL DEFAULT 1"),
        ] {
            if !self.has_column("proxies", column)? {
                self.conn.execute_batch(&format!(
                    "ALTER TABLE proxies ADD COLUMN {column} {definition}"
                ))?;
            }
        }
        Ok(())
    }

    /* ── Policies and credentials ────────────────────────────────────────── */

    /// Write a policy down.
    pub fn save_policy(&self, policy: &crate::credentials::ProxyPolicyRecord) -> Result<()> {
        let settings = serde_json::to_string(&policy.settings)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO proxy_policies (name, settings, builtin) VALUES (?1, ?2, ?3)",
            rusqlite::params![policy.name, settings, policy.builtin as i64],
        )?;
        Ok(())
    }

    /// Every policy.
    pub fn list_policies(&self) -> Result<Vec<crate::credentials::ProxyPolicyRecord>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT name, settings, builtin FROM proxy_policies ORDER BY name")?;
        let mut rows = stmt.query([])?;

        let mut policies = Vec::new();
        while let Some(row) = rows.next()? {
            let name: String = row.get(0)?;
            let settings: String = row.get(1)?;
            let builtin: i64 = row.get(2)?;
            policies.push(crate::credentials::ProxyPolicyRecord {
                name,
                settings: serde_json::from_str(&settings)?,
                builtin: builtin != 0,
            });
        }
        Ok(policies)
    }

    /// One policy by name.
    pub fn policy(&self, name: &str) -> Result<Option<crate::credentials::ProxyPolicyRecord>> {
        Ok(self
            .list_policies()?
            .into_iter()
            .find(|policy| policy.name == name))
    }

    /// Remove a policy.
    ///
    /// A built-in policy stays: it is the one a credential falls back to, and
    /// the engine is expected to have it.
    pub fn delete_policy(&self, name: &str) -> Result<bool> {
        if let Some(policy) = self.policy(name)? {
            if policy.builtin {
                anyhow::bail!("the policy {name} is built in and cannot be removed");
            }
        }
        let removed = self.conn.execute(
            "DELETE FROM proxy_policies WHERE name = ?1 AND builtin = 0",
            rusqlite::params![name],
        )?;
        Ok(removed > 0)
    }

    /// Every credential, without its secret.
    pub fn list_credentials(&self) -> Result<Vec<crate::credentials::ProxyCredential>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT username, policy, enabled, created_at FROM proxy_credentials ORDER BY username",
        )?;
        let mut rows = stmt.query([])?;

        let mut credentials = Vec::new();
        while let Some(row) = rows.next()? {
            credentials.push(crate::credentials::ProxyCredential {
                username: row.get(0)?,
                policy: row.get(1)?,
                enabled: row.get::<_, i64>(2)? != 0,
                created_at: parse_time(row.get::<_, Option<String>>(3)?)
                    .unwrap_or_else(chrono::Utc::now),
                password: None,
            });
        }
        Ok(credentials)
    }

    /// Write a credential down.
    pub fn save_credential(
        &self,
        credential: &crate::credentials::ProxyCredential,
        salt: &str,
        password_hash: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO proxy_credentials
             (username, salt, password_hash, policy, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                credential.username,
                salt,
                password_hash,
                credential.policy,
                credential.enabled as i64,
                credential.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Remove a credential.
    pub fn delete_credential(&self, username: &str) -> Result<bool> {
        let removed = self.conn.execute(
            "DELETE FROM proxy_credentials WHERE username = ?1",
            rusqlite::params![username],
        )?;
        Ok(removed > 0)
    }

    /// The policy a credential carries, when the password is right.
    ///
    /// The check is the only thing standing between the local proxy and an
    /// open door, so it is done here, in one place, and the answer says
    /// nothing about which part was wrong.
    pub fn verify_credential(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Option<crate::credentials::ProxyPolicyRecord>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT salt, password_hash, policy, enabled FROM proxy_credentials WHERE username = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![username])?;

        let Some(row) = rows.next()? else {
            return Ok(None);
        };

        let salt: String = row.get(0)?;
        let stored: String = row.get(1)?;
        let policy: String = row.get(2)?;
        let enabled: i64 = row.get(3)?;

        if enabled == 0 {
            return Ok(None);
        }

        if !crate::credentials::verify(&salt, password, &stored) {
            return Ok(None);
        }

        self.policy(&policy)
    }

    /// Remember a setting.
    ///
    /// The active proxy is a setting: it is a choice the operator made, and
    /// a restart that forgets it leaves the routing policy pointing at
    /// nothing, so every destination the policy covers is refused.
    pub fn save_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }

    /// Read a setting back.
    pub fn load_setting(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT value FROM settings WHERE key = ?1")?;
        let mut rows = stmt.query(rusqlite::params![key])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    fn has_column(&self, table: &str, column: &str) -> Result<bool> {
        let mut stmt = self.conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            if name == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn save_proxies(&self, proxies: &[ProxyInfo]) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(
            "INSERT OR REPLACE INTO proxies
             (id, host, port, protocol, anonymity, latency_ms, country, country_reported, score, last_checked, last_success, consecutive_failures, first_seen, next_probe_at, exit_ip, password, supports_tunnel)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, $15, $16, $17)",
        )?;

        for p in proxies {
            stmt.execute(rusqlite::params![
                p.id,
                p.host,
                p.port,
                format!("{:?}", p.protocol),
                format!("{:?}", p.anonymity),
                p.latency_ms,
                p.country,
                p.country_reported,
                p.score,
                p.last_checked.map(|t| t.to_rfc3339()),
                p.last_success.map(|t| t.to_rfc3339()),
                p.consecutive_failures as i64,
                p.first_seen.to_rfc3339(),
                p.next_probe_at.to_rfc3339(),
                p.exit_ip.map(|ip| ip.to_string()),
                p.password,
                p.supports_tunnel as i64,
            ])?;
        }
        Ok(())
    }

    /// Replace the stored pool with `proxies`.
    ///
    /// A plain upsert would leave evicted rows behind forever, so the table is
    /// cleared first: what is on disk must be exactly what is in memory. The
    /// delete and the inserts share one transaction, so a crash mid-write
    /// leaves the previous pool intact rather than an empty table.
    pub fn replace_proxies(&self, proxies: &[ProxyInfo]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM proxies", [])?;

        let mut stmt = tx.prepare_cached(
            "INSERT INTO proxies
             (id, host, port, protocol, anonymity, latency_ms, country, country_reported, score, last_checked, last_success, consecutive_failures, first_seen, next_probe_at, exit_ip, password, supports_tunnel)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, $15, $16, $17)",
        )?;

        for p in proxies {
            stmt.execute(rusqlite::params![
                p.id,
                p.host,
                p.port,
                format!("{:?}", p.protocol),
                format!("{:?}", p.anonymity),
                p.latency_ms,
                p.country,
                p.country_reported,
                p.score,
                p.last_checked.map(|t| t.to_rfc3339()),
                p.last_success.map(|t| t.to_rfc3339()),
                p.consecutive_failures as i64,
                p.first_seen.to_rfc3339(),
                p.next_probe_at.to_rfc3339(),
                p.exit_ip.map(|ip| ip.to_string()),
                p.password,
                p.supports_tunnel as i64,
            ])?;
        }

        drop(stmt);
        tx.commit()?;
        Ok(())
    }

    /// Store the configured scrape sources.
    ///
    /// The list was held in memory only, so a source added through the API
    /// was gone after the next restart, and the configuration file was the
    /// only truth.
    pub fn save_sources(&self, sources: &[crate::scraper::SourceSpec]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM sources", [])?;

        let mut stmt = tx.prepare("INSERT INTO sources (url, protocol) VALUES (?1, ?2)")?;
        for spec in sources {
            stmt.execute(rusqlite::params![
                spec.url,
                spec.protocol.map(|p| format!("{p:?}"))
            ])?;
        }

        drop(stmt);
        tx.commit()?;
        Ok(())
    }

    /// The configured scrape sources.
    pub fn load_sources(&self) -> Result<Vec<crate::scraper::SourceSpec>> {
        let mut stmt = self
            .conn
            .prepare("SELECT url, protocol FROM sources ORDER BY url")?;
        let rows = stmt.query_map([], |row| {
            let url: String = row.get(0)?;
            let protocol: Option<String> = row.get(1)?;

            Ok(crate::scraper::SourceSpec {
                url,
                protocol: protocol.and_then(|p| p.parse().ok()),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Restore the pool, including what has already been verified.
    pub fn load_proxies(&self) -> Result<Vec<ProxyInfo>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, host, port, protocol, anonymity, latency_ms, country, score,
                    last_checked, last_success, consecutive_failures, first_seen, next_probe_at, exit_ip, password, supports_tunnel, country_reported
             FROM proxies",
        )?;

        let rows = stmt.query_map([], |row| {
            let protocol: String = row.get(3)?;
            let anonymity: String = row.get(4)?;
            let latency: Option<i64> = row.get(5)?;
            let last_checked: Option<String> = row.get(8)?;
            let last_success: Option<String> = row.get(9)?;
            let first_seen: Option<String> = row.get(11)?;
            let next_probe_at: Option<String> = row.get(12)?;
            let exit_ip: Option<String> = row.get(13)?;
            let country: Option<String> = row.get(6)?;
            let stored_reported: Option<String> = row.get(16)?;

            // The two columns used to be one. A row written before the
            // country was measured holds the list's wording in `country`, and
            // that wording is not a code: it goes to the column that keeps
            // what the list said, and the measured country is worked out
            // again from the exit address.
            let looks_like_a_code = country.as_deref().is_some_and(|value| {
                value.len() == 2 && value.chars().all(|c| c.is_ascii_alphabetic())
            });

            let (country, country_reported) = if looks_like_a_code {
                (country, stored_reported)
            } else {
                (None, country.or(stored_reported))
            };
            let password: Option<String> = row.get(14)?;

            Ok(ProxyInfo {
                id: row.get(0)?,
                host: row.get(1)?,
                port: row.get::<_, i64>(2)? as u16,
                protocol: parse_enum(&protocol).unwrap_or(ProxyProtocol::Http),
                anonymity: parse_enum(&anonymity).unwrap_or(Anonymity::Unknown),
                latency_ms: latency.map(|v| v.max(0) as u64),
                country: country.map(Into::into),
                country_reported: country_reported.map(Into::into),
                last_checked: parse_time(last_checked.clone()),
                last_success: parse_time(last_success),
                // A row written before this column existed has no first-seen
                // time; fall back to its last check so it ages out normally
                // instead of living forever.
                first_seen: parse_time(first_seen)
                    .or_else(|| parse_time(last_checked))
                    .unwrap_or_else(chrono::Utc::now),
                // A proxy restored without a schedule is due at once: it has
                // not been probed under this schedule yet.
                next_probe_at: parse_time(next_probe_at).unwrap_or_else(chrono::Utc::now),
                exit_ip: exit_ip.and_then(|raw| raw.parse().ok()),
                password: password.map(Into::into),
                // A row written before the column existed has never been
                // probed, so it is assumed to tunnel until a probe says
                // otherwise.
                supports_tunnel: row.get::<_, Option<i64>>(15)?.unwrap_or(1) != 0,
                consecutive_failures: row.get::<_, Option<i64>>(10)?.unwrap_or(0).max(0) as u32,
                score: row.get(7)?,
            })
        })?;

        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

/// Parse a stored `Debug` name back into an enum.
fn parse_enum<T: std::str::FromStr>(raw: &str) -> Option<T> {
    raw.parse().ok()
}

fn parse_time(raw: Option<String>) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(&raw?)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::{Anonymity, ProxyProtocol};

    fn sample(id: &str, last_success: Option<chrono::DateTime<chrono::Utc>>) -> ProxyInfo {
        ProxyInfo {
            id: id.to_string(),
            host: "198.51.100.1".to_string(),
            port: 8080,
            protocol: ProxyProtocol::Socks5,
            anonymity: Anonymity::Elite,
            latency_ms: Some(120),
            country: Some("NL".into()),
            country_reported: None,
            last_checked: Some(chrono::Utc::now()),
            last_success,
            first_seen: chrono::Utc::now(),
            next_probe_at: chrono::Utc::now(),
            exit_ip: None,
            password: None,
            supports_tunnel: true,
            consecutive_failures: 2,
            score: 0.75,
        }
    }

    #[test]
    fn round_trips_the_verification_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pool.db");
        let now = chrono::Utc::now();

        {
            let storage = Storage::open(&path).expect("open");
            storage
                .replace_proxies(&[sample("a:1", Some(now)), sample("b:2", None)])
                .expect("save");
        }

        let storage = Storage::open(&path).expect("reopen");
        let loaded = storage.load_proxies().expect("load");
        assert_eq!(loaded.len(), 2);

        // `last_success` is what decides whether a proxy must be re-probed, so
        // losing it is exactly the bug that made every restart start from zero.
        let confirmed = loaded.iter().find(|p| p.id == "a:1").expect("a:1");
        assert!(
            confirmed.last_success.is_some(),
            "last_success must survive"
        );
        assert_eq!(confirmed.consecutive_failures, 2);
        assert_eq!(confirmed.protocol, ProxyProtocol::Socks5);
        assert_eq!(confirmed.anonymity, Anonymity::Elite);
        assert_eq!(confirmed.latency_ms, Some(120));

        let unverified = loaded.iter().find(|p| p.id == "b:2").expect("b:2");
        assert!(
            unverified.last_success.is_none(),
            "a proxy that never worked must stay unverified"
        );
    }

    #[test]
    fn replace_drops_proxies_that_left_the_pool() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pool.db");
        let storage = Storage::open(&path).expect("open");

        storage
            .replace_proxies(&[sample("a:1", None), sample("b:2", None)])
            .expect("first save");
        storage
            .replace_proxies(&[sample("a:1", None)])
            .expect("second save");

        let loaded = storage.load_proxies().expect("load");
        assert_eq!(loaded.len(), 1, "an evicted proxy must not linger on disk");
    }

    #[test]
    fn reopening_a_migrated_database_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pool.db");

        // The added columns must not be re-added on the second open.
        Storage::open(&path).expect("first open");
        let storage = Storage::open(&path).expect("second open");
        assert!(storage.load_proxies().expect("load").is_empty());
    }

    #[test]
    fn a_credential_is_written_down_and_checked() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("pool.db");
        let storage = Storage::open(&path).expect("the storage opens");

        let policy = crate::credentials::ProxyPolicyRecord {
            name: "germany".to_string(),
            settings: crate::credentials::ProxyPolicy {
                rotation: crate::credentials::Rotation::Sticky,
                countries: vec!["DE".to_string()],
                ..Default::default()
            },
            builtin: false,
        };
        storage
            .save_policy(&policy)
            .expect("the policy is written down");

        let credential = crate::credentials::ProxyCredential {
            username: "client".to_string(),
            policy: "germany".to_string(),
            enabled: true,
            created_at: chrono::Utc::now(),
            password: None,
        };
        let salt = crate::credentials::generate_salt();
        let password = crate::credentials::generate_password();
        let hash = crate::credentials::digest(&salt, &password);
        storage
            .save_credential(&credential, &salt, &hash)
            .expect("the credential is written down");

        // The password is not stored: what is checked is the digest.
        let checked = storage
            .verify_credential("client", &password)
            .expect("the check runs")
            .expect("the credential is accepted");
        assert_eq!(checked.name, "germany");
        assert_eq!(checked.settings.countries, vec!["DE".to_string()]);

        // The wrong password is refused, and it says nothing about which part
        // was wrong.
        assert!(storage
            .verify_credential("client", "another")
            .expect("the check runs")
            .is_none());
        assert!(storage
            .verify_credential("nobody", &password)
            .expect("the check runs")
            .is_none());

        // The list never carries the secret.
        let listed = storage.list_credentials().expect("the list runs");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].password.is_none());
    }
}

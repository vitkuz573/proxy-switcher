use crate::proxy::ProxyInfo;
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
            );",
        )?;
        Ok(())
    }

    pub fn save_proxies(&self, proxies: &[ProxyInfo]) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(
            "INSERT OR REPLACE INTO proxies (id, host, port, protocol, anonymity, latency_ms, country, score, last_checked)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
                p.score,
                p.last_checked.map(|t| t.to_rfc3339()),
            ])?;
        }
        Ok(())
    }
}

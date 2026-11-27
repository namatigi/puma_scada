//! SQL Server forwarder (cargo feature `mssql`): tiberius over rustls.
//!
//! Tables are created on first use (`dbo` schema):
//! - historians: `MovicloneHistory (Seq BIGINT PRIMARY KEY, Historian, TagName,
//!   RecordTime DATETIME2(3) UTC, Value FLOAT NULL, ValueText NVARCHAR NULL, Quality)`
//! - data loggers: the logger's `table`, `(Seq BIGINT PRIMARY KEY, RecordTime,
//!   <one FLOAT NULL column per logger column>)`; columns added to the logger
//!   later are added to the table. Text values are not representable there and
//!   are stored as NULL.
//!
//! Inserts skip `Seq`s already present, so a batch resent after a crash does
//! not duplicate rows.
//!
//! Connection: `server` may be `host`, `host,port` or `host\INSTANCE` (named
//! instances are resolved through the SQL Browser service). Passwords come
//! only from `secret://` references (see [`crate::secret`]). TLS is required;
//! set `MOVICLONE_MSSQL_TRUST_SERVER_CERT=true` to accept a self-signed server
//! certificate (the usual SQL Express setup), like ADO's `TrustServerCertificate`.

use crate::forward::{BoxFuture, ForwardData, ForwardEntry, Forwarder};
use crate::secret;
use anyhow::{bail, Context};
use moviclone_model::{Database, DbProvider, Project};
use moviclone_tagdb::{Quality, Value};
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tiberius::{AuthMethod, Client, Config, EncryptionLevel, SqlBrowser, ToSql};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

pub const HISTORY_TABLE: &str = "MovicloneHistory";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// SQL Server accepts at most 2100 parameters per statement.
const MAX_PARAMS: usize = 2000;

/// One forwarder per SQL Server database that an enabled historian or data
/// logger writes to.
pub fn forwarders(project: &Project) -> Vec<Arc<dyn Forwarder>> {
    let used: BTreeSet<&str> = project
        .data
        .historians
        .iter()
        .filter(|h| h.enabled)
        .map(|h| h.database.as_str())
        .chain(project.data.loggers.iter().filter(|l| l.enabled).map(|l| l.database.as_str()))
        .collect();
    project
        .data
        .databases
        .iter()
        .filter(|d| d.provider == DbProvider::Mssql && used.contains(d.name.as_str()))
        .map(|d| Arc::new(MssqlForwarder::new(d.clone())) as Arc<dyn Forwarder>)
        .collect()
}

type Conn = Client<Compat<TcpStream>>;

struct Session {
    client: Conn,
    /// Tables (and their columns) known to exist on this connection.
    ensured: HashSet<String>,
}

pub struct MssqlForwarder {
    name: String,
    db: Database,
    session: tokio::sync::Mutex<Option<Session>>,
}

impl MssqlForwarder {
    pub fn new(db: Database) -> Self {
        MssqlForwarder { name: format!("mssql:{}", db.name), db, session: tokio::sync::Mutex::new(None) }
    }

    async fn connect(&self) -> anyhow::Result<Conn> {
        let target = Target::parse(&self.db.server)?;
        let mut config = Config::new();
        config.host(&target.host);
        if let Some(port) = target.port {
            config.port(port);
        }
        if let Some(instance) = &target.instance {
            config.instance_name(instance);
        }
        config.database(&self.db.database);
        config.application_name("Moviclone");
        config.encryption(EncryptionLevel::Required);
        if trust_server_cert() {
            config.trust_cert();
        }
        config.authentication(self.auth()?);
        let tcp = tokio::time::timeout(CONNECT_TIMEOUT, async {
            if target.instance.is_some() && target.port.is_none() {
                TcpStream::connect_named(&config).await.map_err(anyhow::Error::from)
            } else {
                TcpStream::connect(config.get_addr()).await.map_err(anyhow::Error::from)
            }
        })
        .await
        .context("timed out connecting")??;
        tcp.set_nodelay(true)?;
        let client = tokio::time::timeout(CONNECT_TIMEOUT, Client::connect(config, tcp.compat_write()))
            .await
            .context("timed out logging in")??;
        tracing::info!(forwarder = %self.name, server = %self.db.server, database = %self.db.database, "connected to SQL Server");
        Ok(client)
    }

    fn auth(&self) -> anyhow::Result<AuthMethod> {
        if self.db.integrated_auth {
            #[cfg(windows)]
            return Ok(AuthMethod::Integrated);
            #[cfg(not(windows))]
            bail!("integrated authentication is only supported on Windows");
        }
        let user = self.db.user.as_deref().context("database has no user and integrated_auth is off")?;
        let password = self.db.password.as_ref().context("database has no password secret")?;
        Ok(AuthMethod::sql_server(user, secret::resolve(password)?))
    }

    async fn write(&self, session: &mut Session, batch: &[ForwardEntry]) -> anyhow::Result<()> {
        let mut samples = Vec::new();
        let mut rows: Vec<(&str, &ForwardEntry)> = Vec::new();
        for e in batch {
            match &e.data {
                ForwardData::Sample { .. } => samples.push(e),
                ForwardData::Row { table, .. } => rows.push((table.as_str(), e)),
            }
        }
        if !samples.is_empty() {
            let key = HISTORY_TABLE.to_string();
            if !session.ensured.contains(&key) {
                session.client.simple_query(history_ddl()).await?.into_results().await?;
                session.ensured.insert(key);
            }
            for chunk in samples.chunks(MAX_PARAMS / HISTORY_PARAMS) {
                let sql = history_insert(chunk.len());
                let params = history_params(chunk);
                let refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();
                session.client.execute(sql, &refs).await?;
            }
        }
        // Group consecutive rows of the same table and column set.
        let mut i = 0;
        while i < rows.len() {
            let (table, first) = rows[i];
            let columns = row_columns(first);
            let mut j = i + 1;
            while j < rows.len() && rows[j].0 == table && row_columns(rows[j].1) == columns {
                j += 1;
            }
            let key = format!("{table}({})", columns.join(","));
            if !session.ensured.contains(&key) {
                session.client.simple_query(logger_ddl(table, &columns)?).await?.into_results().await?;
                session.ensured.insert(key);
            }
            let per_row = 2 + columns.len();
            let group: Vec<_> = rows[i..j].iter().map(|(_, e)| *e).collect();
            for chunk in group.chunks((MAX_PARAMS / per_row).max(1)) {
                let sql = logger_insert(table, &columns, chunk.len())?;
                let params = logger_params(chunk);
                let refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();
                session.client.execute(sql, &refs).await?;
            }
            i = j;
        }
        Ok(())
    }
}

impl Forwarder for MssqlForwarder {
    fn name(&self) -> &str {
        &self.name
    }

    // The project's database *entry* name (what historians refer to), not the SQL catalog name.
    #[allow(clippy::misnamed_getters)]
    fn database(&self) -> &str {
        &self.db.name
    }

    fn send<'a>(&'a self, batch: &'a [ForwardEntry]) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let mut guard = self.session.lock().await;
            if guard.is_none() {
                let client = self.connect().await?;
                *guard = Some(Session { client, ensured: HashSet::new() });
            }
            let Some(session) = guard.as_mut() else { bail!("no SQL Server session") };
            let result = self.write(session, batch).await;
            if result.is_err() {
                // Reconnect next time: the connection may be in an unknown state.
                *guard = None;
            }
            result
        })
    }
}

fn trust_server_cert() -> bool {
    std::env::var("MOVICLONE_MSSQL_TRUST_SERVER_CERT").map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")).unwrap_or(false)
}

#[derive(Debug, PartialEq, Eq)]
struct Target {
    host: String,
    port: Option<u16>,
    instance: Option<String>,
}

impl Target {
    /// `host`, `host,port`, `host\INSTANCE`, `host\INSTANCE,port`; `.` and
    /// `(local)` mean this machine.
    fn parse(server: &str) -> anyhow::Result<Target> {
        let server = server.trim().trim_start_matches("tcp:");
        let (rest, port) = match server.rsplit_once(',') {
            Some((r, p)) => (r, Some(p.trim().parse::<u16>().with_context(|| format!("invalid port in server {server:?}"))?)),
            None => (server, None),
        };
        let (host, instance) = match rest.split_once('\\') {
            Some((h, i)) if !i.is_empty() => (h, Some(i.to_string())),
            Some((h, _)) => (h, None),
            None => (rest, None),
        };
        let host = match host.trim() {
            "" => bail!("server {server:?} has no host"),
            "." | "(local)" | "(localdb)" => "localhost".to_string(),
            h => h.to_string(),
        };
        Ok(Target { host, port, instance })
    }
}

// ---------------------------------------------------------------- SQL text

/// Bracket-quotes an identifier (`]` doubled), rejecting what SQL Server
/// would not accept as a name.
fn ident(name: &str) -> anyhow::Result<String> {
    if name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control) {
        bail!("{name:?} is not a valid SQL Server identifier");
    }
    Ok(format!("[{}]", name.replace(']', "]]")))
}

/// N'…' literal for names inside OBJECT_ID / COL_LENGTH.
fn literal(s: &str) -> String {
    format!("N'{}'", s.replace('\'', "''"))
}

const HISTORY_PARAMS: usize = 7;

/// Milliseconds since the epoch to DATETIME2 without overflowing DATEADD's int argument.
fn ms_to_datetime(expr: &str) -> String {
    format!("DATEADD(millisecond, CAST({expr} % 86400000 AS INT), DATEADD(day, CAST({expr} / 86400000 AS INT), CAST('1970-01-01' AS DATETIME2(3))))")
}

fn history_ddl() -> String {
    let t = format!("dbo.{HISTORY_TABLE}");
    format!(
        "IF OBJECT_ID({}, N'U') IS NULL CREATE TABLE [dbo].[{HISTORY_TABLE}] (\
         Seq BIGINT NOT NULL PRIMARY KEY, Historian NVARCHAR(128) NOT NULL, TagName NVARCHAR(256) NOT NULL, \
         RecordTime DATETIME2(3) NOT NULL, Value FLOAT NULL, ValueText NVARCHAR(4000) NULL, Quality NVARCHAR(16) NOT NULL); \
         IF NOT EXISTS (SELECT 1 FROM sys.indexes WHERE name = N'IX_{HISTORY_TABLE}_Tag_Time') \
         CREATE INDEX [IX_{HISTORY_TABLE}_Tag_Time] ON [dbo].[{HISTORY_TABLE}] (TagName, RecordTime);",
        literal(&t)
    )
}

fn values_clause(rows: usize, per_row: usize) -> String {
    (0..rows)
        .map(|r| {
            let ps: Vec<_> = (1..=per_row).map(|c| format!("@P{}", r * per_row + c)).collect();
            format!("({})", ps.join(", "))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn history_insert(rows: usize) -> String {
    format!(
        "INSERT INTO [dbo].[{HISTORY_TABLE}] (Seq, Historian, TagName, RecordTime, Value, ValueText, Quality) \
         SELECT v.Seq, v.Historian, v.TagName, {}, v.Value, v.ValueText, v.Quality \
         FROM (VALUES {}) AS v(Seq, Historian, TagName, Ms, Value, ValueText, Quality) \
         WHERE NOT EXISTS (SELECT 1 FROM [dbo].[{HISTORY_TABLE}] h WHERE h.Seq = v.Seq)",
        ms_to_datetime("v.Ms"),
        values_clause(rows, HISTORY_PARAMS)
    )
}

fn row_columns(e: &ForwardEntry) -> Vec<String> {
    match &e.data {
        ForwardData::Row { columns, .. } => columns.iter().map(|c| c.column.clone()).collect(),
        ForwardData::Sample { .. } => Vec::new(),
    }
}

fn logger_ddl(table: &str, columns: &[String]) -> anyhow::Result<String> {
    let t = ident(table)?;
    let obj = literal(&format!("dbo.{t}"));
    let mut sql = format!(
        "IF OBJECT_ID({obj}, N'U') IS NULL CREATE TABLE [dbo].{t} (Seq BIGINT NOT NULL PRIMARY KEY, RecordTime DATETIME2(3) NOT NULL{}); ",
        columns.iter().map(|c| ident(c).map(|c| format!(", {c} FLOAT NULL"))).collect::<anyhow::Result<String>>()?
    );
    for c in columns {
        sql.push_str(&format!("IF COL_LENGTH({obj}, {}) IS NULL ALTER TABLE [dbo].{t} ADD {} FLOAT NULL; ", literal(c), ident(c)?));
    }
    Ok(sql)
}

fn logger_insert(table: &str, columns: &[String], rows: usize) -> anyhow::Result<String> {
    let t = ident(table)?;
    let cols: Vec<String> = columns.iter().map(|c| ident(c)).collect::<anyhow::Result<_>>()?;
    // Aliases for the VALUES derived table: C1..Cn avoid clashes with Seq/Ms.
    let aliases: Vec<String> = (1..=cols.len()).map(|i| format!("C{i}")).collect();
    let target_cols = if cols.is_empty() { String::new() } else { format!(", {}", cols.join(", ")) };
    let select_cols: String = aliases.iter().map(|a| format!(", v.{a}")).collect();
    let alias_list: String = aliases.iter().map(|a| format!(", {a}")).collect();
    Ok(format!(
        "INSERT INTO [dbo].{t} (Seq, RecordTime{target_cols}) \
         SELECT v.Seq, {}{select_cols} \
         FROM (VALUES {}) AS v(Seq, Ms{alias_list}) \
         WHERE NOT EXISTS (SELECT 1 FROM [dbo].{t} x WHERE x.Seq = v.Seq)",
        ms_to_datetime("v.Ms"),
        values_clause(rows, 2 + cols.len())
    ))
}

fn seq_param(seq: u64) -> i64 {
    i64::try_from(seq).unwrap_or(i64::MAX)
}

fn quality_name(q: Quality) -> &'static str {
    match q {
        Quality::Good => "good",
        Quality::Uncertain => "uncertain",
        Quality::Bad => "bad",
    }
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Text(_) | Value::Null => None,
        v => v.as_f64().filter(|f| f.is_finite()),
    }
}

fn history_params(chunk: &[&ForwardEntry]) -> Vec<Box<dyn ToSql>> {
    let mut out: Vec<Box<dyn ToSql>> = Vec::with_capacity(chunk.len() * HISTORY_PARAMS);
    for e in chunk {
        if let ForwardData::Sample { historian, tag, value, quality } = &e.data {
            out.push(Box::new(seq_param(e.seq)));
            out.push(Box::new(historian.clone()));
            out.push(Box::new(tag.clone()));
            out.push(Box::new(e.ts));
            out.push(Box::new(number(value)));
            out.push(Box::new(match value {
                Value::Text(s) => Some(s.clone()),
                _ => None,
            }));
            out.push(Box::new(quality_name(*quality).to_string()));
        }
    }
    out
}

fn logger_params(chunk: &[&ForwardEntry]) -> Vec<Box<dyn ToSql>> {
    let mut out: Vec<Box<dyn ToSql>> = Vec::new();
    for e in chunk {
        if let ForwardData::Row { columns, .. } = &e.data {
            out.push(Box::new(seq_param(e.seq)));
            out.push(Box::new(e.ts));
            for c in columns {
                out.push(Box::new(if c.quality == Quality::Bad { None } else { number(&c.value) }));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forward::LoggedCell;

    #[test]
    fn parses_server_names() {
        assert_eq!(
            Target::parse(r"DESKTOP-OVIJ9HJ\SQLEXPRESS").unwrap(),
            Target { host: "DESKTOP-OVIJ9HJ".into(), port: None, instance: Some("SQLEXPRESS".into()) }
        );
        assert_eq!(Target::parse("db.plant.local,1444").unwrap(), Target { host: "db.plant.local".into(), port: Some(1444), instance: None });
        assert_eq!(Target::parse(r".\SQLEXPRESS").unwrap().host, "localhost");
        assert!(Target::parse("host,notaport").is_err());
        assert!(Target::parse("").is_err());
    }

    #[test]
    fn identifiers_are_quoted_safely() {
        assert_eq!(ident("FM01").unwrap(), "[FM01]");
        assert_eq!(ident("a]b; DROP TABLE x--").unwrap(), "[a]]b; DROP TABLE x--]");
        assert!(ident("").is_err());
        assert!(ident("a\nb").is_err());
        assert_eq!(literal("it's"), "N'it''s'");
    }

    #[test]
    fn insert_statements_number_parameters() {
        let sql = history_insert(2);
        assert!(sql.contains("(@P1, @P2, @P3, @P4, @P5, @P6, @P7), (@P8, @P9, @P10, @P11, @P12, @P13, @P14)"));
        assert!(sql.contains("WHERE NOT EXISTS"));
        let sql = logger_insert("FM01", &["FM_01_Temp".into(), "FM_01_Vol".into()], 1).unwrap();
        assert!(sql.starts_with("INSERT INTO [dbo].[FM01] (Seq, RecordTime, [FM_01_Temp], [FM_01_Vol])"));
        assert!(sql.contains("AS v(Seq, Ms, C1, C2)"));
        assert!(sql.contains("(@P1, @P2, @P3, @P4)"));
        let ddl = logger_ddl("FM01", &["FM_01_Temp".into()]).unwrap();
        assert!(ddl.contains("CREATE TABLE [dbo].[FM01] (Seq BIGINT NOT NULL PRIMARY KEY, RecordTime DATETIME2(3) NOT NULL, [FM_01_Temp] FLOAT NULL)"));
        assert!(ddl.contains("ALTER TABLE [dbo].[FM01] ADD [FM_01_Temp] FLOAT NULL"));
    }

    #[test]
    fn parameters_match_placeholders() {
        let sample = ForwardEntry {
            seq: 3,
            ts: 1_795_400_000_000,
            data: ForwardData::Sample { historian: "FM01".into(), tag: "T".into(), value: Value::Text("x".into()), quality: Quality::Good },
        };
        assert_eq!(history_params(&[&sample, &sample]).len(), 2 * HISTORY_PARAMS);
        let row = ForwardEntry {
            seq: 4,
            ts: 0,
            data: ForwardData::Row {
                logger: "FM01".into(),
                table: "FM01".into(),
                columns: vec![LoggedCell { column: "a".into(), value: Value::Float(1.0), quality: Quality::Good }],
            },
        };
        assert_eq!(logger_params(&[&row]).len(), 3);
        assert_eq!(row_columns(&row), vec!["a".to_string()]);
    }

    #[test]
    fn forwarders_only_for_used_mssql_databases() {
        let mut p = Project::default();
        let db = |name: &str, provider| Database {
            name: name.into(),
            provider,
            server: "localhost".into(),
            database: "d".into(),
            user: Some("u".into()),
            password: None,
            integrated_auth: false,
        };
        p.data.databases = vec![db("a", DbProvider::Mssql), db("b", DbProvider::Mssql), db("c", DbProvider::Postgres)];
        p.data.historians = vec![moviclone_model::Historian { name: "H".into(), enabled: true, database: "a".into(), max_age_s: 1 }];
        let f = forwarders(&p);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].name(), "mssql:a");
        assert_eq!(f[0].database(), "a");
    }
}

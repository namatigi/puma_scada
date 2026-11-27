//! `UFUAServer/`: tags, drivers, databases, historians and data loggers.
//!
//! Enum values below were read from the platform's own assemblies
//! (`UFUAModel.DataType`, `DriverCodeBase.Enumerators.LinkType`,
//! `ModbusTCP.FunctionCodes`, `ModbusTCP.AddressTypes`).

use crate::report::{Level, Report, SecretNeeded};
use crate::util::{guid_of, parse_conn_string, read_dataset, rows, slug, Row, Tables};
use crate::TagIndex;
use anyhow::{Context, Result};
use moviclone_model::*;
use std::collections::HashMap;
use std::path::Path;

pub struct ServerImport {
    pub tags: Vec<Tag>,
    pub drivers: Vec<Driver>,
    pub data: DataConfig,
}

pub fn import(dir: &Path, rep: &mut Report) -> Result<(ServerImport, TagIndex)> {
    let server_dir = dir.join("UFUAServer");
    let file = server_dir.join("Server.UFUAServer");
    let t = read_dataset(&file).context("reading the I/O server configuration")?;
    let src = "UFUAServer/Server.UFUAServer";

    let (drivers, station_driver) = import_drivers(&server_dir, &t, rep)?;
    let mut tags = import_tags(&t, &station_driver, &drivers, rep);
    tags.sort_by(|a, b| a.name.cmp(&b.name));

    let mut index = TagIndex::default();
    for tag in &tags {
        if let Some(g) = &tag.legacy_id {
            index.by_guid.insert(g.clone(), tag.name.clone());
        }
        index.names.insert(tag.name.clone());
        if let Some(h) = &tag.historian {
            index.historians.entry(h.clone()).or_default().push(tag.name.clone());
        }
    }
    let removed = server_dir.join("Server.RemovedTags");
    if removed.exists() {
        index.load_removed(&removed)?;
    }

    let mut dbs = Databases::default();
    let mut data = import_data(&t, &mut dbs, &index, rep);

    // Server-wide default connections only matter if something used them, but
    // they still hold credentials worth reporting.
    for cfg in rows(&t, "ConfigurationBase") {
        for col in ["HistorianDefaultConnection", "EventDefaultConnection", "AuditTraceDefaultConnection"] {
            if let Some(cs) = cfg.get(col) {
                if parse_conn_string(cs).contains_key("password") {
                    rep.add(Level::Security, src, format!(
                        "{col} contains a plain-text SQL password; it was not imported (server defaults are not used by Moviclone)"));
                }
            }
        }
        if let Some(app) = cfg.get("ApplicationName") {
            rep.add(Level::Info, src, format!("Movicon I/O server application name: {app}"));
        }
    }
    for a in rows(&t, "AddressBase") {
        if let (Some(tr), Some(port)) = (a.get("Transport"), a.get("Port")) {
            rep.add(Level::NotImported, src, format!(
                "OPC UA endpoint {tr}://{}:{port}; the Moviclone OPC UA gateway is configured separately (phase 2)",
                a.get("Server").unwrap_or("localhost")));
        }
    }
    for (table, what) in [
        ("UFUAAlarmThreshold", "alarm thresholds"),
        ("UFUAAlarmDefinition", "alarm definitions"),
        ("UFUAEngineeringUnit", "engineering units"),
        ("UFUAView", "tag views"),
        ("UFUATagPrototype", "tag prototypes (structures)"),
    ] {
        let n = rows(&t, table).len();
        if n > 0 {
            rep.add(Level::NotImported, src, format!("{n} {what}"));
        }
    }
    if !rows(&t, "UFUAFolder").is_empty() {
        rep.add(Level::Warning, src, "tag folders were flattened; tag names are unchanged");
    }

    rep.secrets.extend(dbs.secrets);
    data.databases = dbs.list;
    Ok((ServerImport { tags, drivers, data }, index))
}

// ------------------------------------------------------------------ drivers

fn import_drivers(dir: &Path, t: &Tables, rep: &mut Report) -> Result<(Vec<Driver>, HashMap<String, String>)> {
    let mut drivers = Vec::new();
    let mut station_driver = HashMap::new();
    let mut registered = Vec::new();

    for d in rows(t, "UFUACommunicationDriverEx") {
        let Some(name) = d.get("Name") else { continue };
        registered.push(format!("{name}.drvSettings"));
        if name != "ModbusTCP" {
            rep.add(Level::NotImported, format!("UFUAServer/Drivers/{name}.drvSettings"),
                format!("driver {name} ({}) is not supported yet", d.get("FriendlyName").unwrap_or(name)));
            continue;
        }
        let path = dir.join("Drivers").join("ModbusTCP.drvSettings");
        let src = "UFUAServer/Drivers/ModbusTCP.drvSettings";
        let s = read_dataset(&path)?;
        let jobs = {
            let p = dir.join("Drivers").join("ModbusTCP.dynJobs");
            if p.exists() { read_dataset(&p)? } else { Tables::new() }
        };

        for ch in rows(&s, "ChannelSettings") {
            let ch_name = ch.get("Name").unwrap_or("modbus");
            let oid = ch.get("OID");
            let tcp = rows(&s, "TcpChannelSettings").iter().find(|r| r.get("OID") == oid);
            let Some(host) = tcp.and_then(|r| r.get("TcpChannelSettingsHostName")) else {
                rep.add(Level::Warning, src, format!("channel {ch_name:?} has no TCP host; skipped"));
                continue;
            };
            let driver_name = slug(ch_name);
            let mut stations = Vec::new();
            for st in rows(&s, "StationSettings").iter().filter(|r| r.get("Channel") == Some(ch_name)) {
                let Some(st_name) = st.get("Name") else { continue };
                let unit_id = st.int("StationID").unwrap_or(1);
                let unit_id = u8::try_from(unit_id).unwrap_or_else(|_| {
                    rep.add(Level::Problem, src, format!("station {st_name:?} has unit id {unit_id}, outside 0..=255; set to 1"));
                    1
                });
                stations.push(ModbusStation {
                    name: st_name.to_string(),
                    unit_id,
                    addressing: if st.int("AddressType") == Some(0) { Addressing::ZeroBased } else { Addressing::OneBased },
                    max_retries: st.int("MaxRetriesBeforeError").unwrap_or(3).max(0) as u32,
                });
                station_driver.insert(st_name.to_string(), driver_name.clone());
            }
            // Movicon stores the poll interval per compiled job; use the fastest.
            let poll_ms = rows(&jobs, "CommJobSettings")
                .iter()
                .filter(|j| stations.iter().any(|s| Some(s.name.as_str()) == j.get("Station")))
                .filter_map(|j| j.int("SamplingInterval"))
                .min()
                .unwrap_or(500) as u32;
            if rows(&jobs, "CommJobSettings").iter().any(|j| j.flag("SwapBytes") || j.flag("SwapWords") || j.flag("SwapDWords")) {
                rep.add(Level::NotImported, "UFUAServer/Drivers/ModbusTCP.dynJobs",
                    "some jobs swap bytes or words; byte order options arrive with the runtime driver (phase 2)");
            }
            rep.add(Level::Info, src, format!(
                "channel {ch_name:?} → drivers/{driver_name}.toml ({} stations, Movicon compiled {} poll jobs)",
                stations.len(), rows(&jobs, "CommJobSettings").len()));
            drivers.push(Driver {
                name: driver_name,
                config: DriverConfig::ModbusTcp(ModbusTcp {
                    description: Some(format!("Imported from Movicon channel \"{ch_name}\"")),
                    host: host.to_string(),
                    port: tcp.and_then(|r| r.int("TcpChannelSettingsHostPort")).unwrap_or(502) as u16,
                    poll_ms,
                    timeout_ms: ch.int("Timeout").unwrap_or(5000) as u32,
                    keep_open: ch.get("KeepOpened") != Some("false"),
                    stations,
                }),
            });
        }
    }

    if let Ok(entries) = std::fs::read_dir(dir.join("Drivers")) {
        let mut leftovers: Vec<String> = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.ends_with(".drvSettings") && !registered.contains(n))
            .collect();
        leftovers.sort();
        for n in leftovers {
            rep.add(Level::Info, format!("UFUAServer/Drivers/{n}"), "settings for a driver that is not registered in the project; ignored");
        }
    }
    Ok((drivers, station_driver))
}

// ------------------------------------------------------------------ tags

fn data_type(code: i64) -> Option<DataType> {
    Some(match code {
        0 => DataType::Bool,
        1 => DataType::I8,
        2 => DataType::U8,
        3 => DataType::I16,
        4 => DataType::U16,
        5 => DataType::I32,
        6 => DataType::U32,
        7 => DataType::I64,
        8 => DataType::U64,
        9 => DataType::F32,
        10 => DataType::F64,
        11 => DataType::String,
        _ => return None,
    })
}

fn import_tags(t: &Tables, station_driver: &HashMap<String, String>, drivers: &[Driver], rep: &mut Report) -> Vec<Tag> {
    let src = "UFUAServer/Server.UFUAServer";
    let fallback_driver = drivers.first().map(|d| d.name.clone());
    let mut tags = Vec::new();
    for r in rows(t, "UFUATag") {
        let Some(name) = r.get("Name") else { continue };
        let Some(ty) = r.int("DataType").and_then(data_type) else {
            rep.add(Level::Warning, src, format!("tag {name:?}: unknown data type {:?}; skipped", r.get("DataType")));
            continue;
        };
        if r.int("ArrayDimension").unwrap_or(0) > 0 {
            rep.add(Level::NotImported, src, format!("tag {name:?} is an array; arrays are not supported yet, imported as a scalar"));
        }
        let (address, access) = match r.get("DynamicSettings") {
            Some(dyn_settings) => parse_dynamic(name, dyn_settings, station_driver, fallback_driver.as_deref(), rep),
            None => (None, Access::ReadWrite),
        };
        let description = r.get("Description").filter(|d| *d != name).map(str::to_string);
        tags.push(Tag {
            name: name.to_string(),
            description,
            data_type: ty,
            access,
            address,
            historian: r.get("HistorianSettings").map(str::to_string),
            legacy_id: r.get("NodeId").and_then(guid_of),
        });
        if r.flag("IsRetentive") {
            rep.add(Level::NotImported, src, format!("tag {name:?} is retentive; retained values arrive with the runtime (phase 2)"));
        }
    }
    tags
}

/// `ModbusTCP.Station=Flow_Meter_1|LinkType=0|FC=5|SA=247|File=0`
fn parse_dynamic(
    tag: &str,
    s: &str,
    station_driver: &HashMap<String, String>,
    fallback_driver: Option<&str>,
    rep: &mut Report,
) -> (Option<Address>, Access) {
    let src = "UFUAServer/Server.UFUAServer";
    let (driver_kind, rest) = s.split_once('.').unwrap_or(("", s));
    let kv: HashMap<&str, &str> = rest.split('|').filter_map(|p| p.split_once('=')).collect();

    let access = match kv.get("LinkType").and_then(|v| v.parse::<u8>().ok()) {
        Some(0) | None => Access::Read,
        Some(1) => Access::ReadWrite,
        Some(_) => Access::Write, // ExceptionOutput / UnconditionalOutput
    };
    if driver_kind != "ModbusTCP" {
        rep.add(Level::NotImported, src, format!("tag {tag:?} uses driver {driver_kind:?}, which is not supported yet; address dropped"));
        return (None, access);
    }
    let Some(station) = kv.get("Station") else {
        rep.add(Level::Problem, src, format!("tag {tag:?} has a Modbus link without a station; address dropped"));
        return (None, access);
    };
    let (area, write) = match kv.get("FC").and_then(|v| v.parse::<u8>().ok()) {
        Some(0) => (ModbusArea::Coil, ModbusWrite::Multiple),
        Some(1) => (ModbusArea::DiscreteInput, ModbusWrite::Multiple),
        Some(2) => (ModbusArea::HoldingRegister, ModbusWrite::Multiple),
        Some(3) => (ModbusArea::InputRegister, ModbusWrite::Multiple),
        Some(4) => (ModbusArea::Coil, ModbusWrite::Single),
        Some(5) => (ModbusArea::HoldingRegister, ModbusWrite::Single),
        Some(8) => (ModbusArea::HoldingRegister, ModbusWrite::Mask),
        other => {
            rep.add(Level::NotImported, src, format!("tag {tag:?}: Modbus area code {other:?} (file record / exception status) is not supported; address dropped"));
            return (None, access);
        }
    };
    let Some(register) = kv.get("SA").and_then(|v| v.parse::<u16>().ok()) else {
        rep.add(Level::Problem, src, format!("tag {tag:?} has no valid start address; address dropped"));
        return (None, access);
    };
    if kv.get("File").is_some_and(|f| *f != "0") {
        rep.add(Level::Warning, src, format!(
            "tag {tag:?} has file number {} set, which only applies to file records; ignored", kv["File"]));
    }
    let driver = match station_driver.get(*station) {
        Some(d) => d.clone(),
        None => {
            rep.add(Level::Problem, src, format!(
                "tag {tag:?} refers to station {station:?}, which no channel defines; kept so `movi build` reports it"));
            fallback_driver.unwrap_or("modbus").to_string()
        }
    };
    (Some(Address::Modbus(ModbusAddress { driver, station: station.to_string(), area, register, write })), access)
}

// ------------------------------------------------------------------ data

#[derive(Default)]
struct Databases {
    list: Vec<Database>,
    secrets: Vec<SecretNeeded>,
}

impl Databases {
    /// Returns the database name for a connection string, adding it on first use.
    /// The password is replaced by a secret reference and never stored.
    fn get(&mut self, cs: &str, used_by: &str, rep: &mut Report) -> Option<String> {
        let kv = parse_conn_string(cs);
        let provider_raw = kv.get("xpoprovider").map(String::as_str).unwrap_or("MSSqlServer");
        let provider = match provider_raw.to_ascii_lowercase().as_str() {
            "mssqlserver" | "mssql" => DbProvider::Mssql,
            "postgres" | "postgresql" => DbProvider::Postgres,
            "sqlite" => DbProvider::Sqlite,
            _ => {
                rep.add(Level::NotImported, used_by, format!("database provider {provider_raw:?} is not supported"));
                return None;
            }
        };
        let server = kv.get("data source").or_else(|| kv.get("server"))?.clone();
        let database = kv.get("initial catalog").or_else(|| kv.get("database"))?.clone();
        let user = kv.get("user id").or_else(|| kv.get("uid")).cloned();
        let integrated = kv.get("integrated security").is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "sspi" | "true" | "yes"));

        if let Some(db) = self.list.iter().find(|d| d.server == server && d.database == database && d.user == user) {
            return Some(db.name.clone());
        }
        let base = slug(&database);
        let mut name = base.clone();
        let mut n = 2;
        while self.list.iter().any(|d| d.name == name) {
            name = format!("{base}-{n}");
            n += 1;
        }
        let password = kv.get("password").or_else(|| kv.get("pwd")).map(|_| {
            rep.add(Level::Security, used_by, format!(
                "plain-text password for SQL user {:?} on {server} removed; the project now refers to secret://{name}",
                user.as_deref().unwrap_or("?")));
            self.secrets.push(SecretNeeded {
                name: name.clone(),
                description: format!("password for SQL user {:?} on {server}, database {database}", user.as_deref().unwrap_or("?")),
            });
            SecretRef::new(&name).expect("slug is a valid secret name")
        });
        rep.add(Level::Warning, used_by, format!(
            "database server {server:?} is a machine-specific instance; check it exists where the runtime will run"));
        self.list.push(Database { name: name.clone(), provider, server, database, user, password, integrated_auth: integrated });
        Some(name)
    }
}

fn import_data(t: &Tables, dbs: &mut Databases, index: &TagIndex, rep: &mut Report) -> DataConfig {
    let src = "UFUAServer/Server.UFUAServer";
    let mut data = DataConfig::default();

    for h in rows(t, "UFUAHistorianSettings") {
        let Some(name) = h.get("Name") else { continue };
        let used_by = format!("{src} (historian {name})");
        let Some(database) = h.get("ConnectionSettings").and_then(|cs| dbs.get(cs, &used_by, rep)) else {
            rep.add(Level::Problem, src, format!("historian {name:?} has no usable connection; skipped"));
            continue;
        };
        data.historians.push(Historian {
            name: name.to_string(),
            enabled: h.flag("Enabled"),
            database,
            max_age_s: h.int("MaxAge").unwrap_or(2_592_000).max(0) as u64,
        });
    }

    let columns = rows(t, "DataLoggerColumn");
    for l in rows(t, "DataLoggerSettings") {
        let Some(name) = l.get("Name") else { continue };
        let used_by = format!("{src} (data logger {name})");
        // Data-logger connections are "Provider|connection string|…".
        let cs = l.get("ConnectionSettings").map(|s| s.split('|').nth(1).unwrap_or(s));
        let Some(database) = cs.and_then(|cs| dbs.get(cs, &used_by, rep)) else {
            rep.add(Level::Problem, src, format!("data logger {name:?} has no usable connection; skipped"));
            continue;
        };
        let interval = l.int("RecordingTimeInterval").unwrap_or(0);
        let record = if l.flag("RecordOnDataChange") || interval <= 0 {
            Recording::OnChange
        } else {
            Recording::EveryS(interval as u32)
        };
        if l.flag("RecordOnDataChange") && interval > 0 {
            rep.add(Level::Warning, src, format!("data logger {name:?} records on change and every {interval} s; only on-change was kept"));
        }
        let oid = l.get("OID");
        let cols = columns
            .iter()
            .filter(|c| c.get("DataLoggerReference") == oid)
            .filter_map(|c| logger_column(c, name, index, rep))
            .collect();
        data.loggers.push(DataLogger {
            name: name.to_string(),
            enabled: l.flag("Enable"),
            database,
            table: l.get("TableName").unwrap_or(name).to_string(),
            record,
            max_age_s: l.int("MaxAge").unwrap_or(2_592_000).max(0) as u64,
            columns: cols,
        });
    }
    data
}

/// Column tags look like `GUID|2:Tags/2:Name|ns=2;g=GUID|Name (App)`.
fn logger_column(c: &Row, logger: &str, index: &TagIndex, rep: &mut Report) -> Option<LoggerColumn> {
    let name = c.get("ColumnName")?;
    let raw = c.get("ColumnTag").unwrap_or("");
    let path_name = raw.split('|').nth(1).and_then(|p| p.rsplit(':').next()).filter(|s| !s.is_empty());
    let context = format!("data logger {logger:?} column {name:?}");
    let tag = index.resolve(guid_of(raw).as_deref(), path_name, "UFUAServer/Server.UFUAServer", &context, rep)?;
    Some(LoggerColumn { name: name.to_string(), tag })
}

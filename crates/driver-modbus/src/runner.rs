//! The polling loop of one driver: one TCP connection, reads every `poll_ms`,
//! writes in between, reconnects with exponential backoff.

use crate::codec::{decode_bit, decode_registers, encode_bit, encode_registers};
use crate::plan::{DriverPlan, ReadJob};
use crate::stats::DriverStats;
use moviclone_model::{Address, Driver, DriverConfig, ModbusArea, ModbusWrite};
use moviclone_tagdb::{now_ms, Quality, TagId, TagStore, Value, WriteError, WriteRequest};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_modbus::client::{tcp, Context, Reader, Writer};
use tokio_modbus::prelude::{Exception, Slave, SlaveContext};

/// First reconnect delay; doubles after every failed attempt up to [`MAX_BACKOFF`].
pub const MIN_BACKOFF: Duration = Duration::from_millis(250);
pub const MAX_BACKOFF: Duration = Duration::from_secs(10);

/// Why a request failed.
#[derive(Debug)]
enum ReqError {
    /// The device answered with a Modbus exception: the link is fine.
    Exception(Exception),
    /// No answer in time. The connection may still carry the late reply, so it is replaced.
    Timeout,
    /// Socket closed, reset or garbled: the connection is gone.
    Transport(String),
}

impl std::fmt::Display for ReqError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReqError::Exception(e) => write!(f, "device exception: {e}"),
            ReqError::Timeout => f.write_str("request timed out"),
            ReqError::Transport(m) => write!(f, "connection error: {m}"),
        }
    }
}

async fn request<T>(timeout: Duration, fut: impl Future<Output = tokio_modbus::Result<T>>) -> Result<T, ReqError> {
    match tokio::time::timeout(timeout, fut).await {
        Err(_) => Err(ReqError::Timeout),
        Ok(Err(e)) => Err(ReqError::Transport(e.to_string())),
        Ok(Ok(Err(exc))) => Err(ReqError::Exception(exc)),
        Ok(Ok(Ok(v))) => Ok(v),
    }
}

enum Wake {
    Deadline,
    Shutdown,
}

pub(crate) struct Runner {
    name: String,
    host: String,
    port: u16,
    poll: Duration,
    timeout: Duration,
    keep_open: bool,
    plan: DriverPlan,
    store: Arc<TagStore>,
    stats: DriverStats,
    writes: mpsc::Receiver<WriteRequest>,
    shutdown: watch::Receiver<bool>,
    ctx: Option<Context>,
    backoff: Duration,
    /// All tags are already marked bad for the current outage (avoid re-marking every retry).
    outage_marked: bool,
    station_retry_at: HashMap<String, (Instant, Duration)>,
}

impl Runner {
    pub(crate) fn new(
        driver: &Driver,
        store: Arc<TagStore>,
        stats: DriverStats,
        writes: mpsc::Receiver<WriteRequest>,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        let DriverConfig::ModbusTcp(cfg) = &driver.config;
        let plan = DriverPlan::build(driver, &store);
        for (id, why) in &plan.unreadable {
            tracing::warn!(driver = %driver.name, tag = store.name(*id), "tag cannot be polled: {why}");
        }
        Self {
            name: driver.name.clone(),
            host: cfg.host.clone(),
            port: cfg.port,
            poll: Duration::from_millis(u64::from(cfg.poll_ms.max(10))),
            timeout: Duration::from_millis(u64::from(cfg.timeout_ms.max(50))),
            keep_open: cfg.keep_open,
            plan,
            store,
            stats,
            writes,
            shutdown,
            ctx: None,
            backoff: MIN_BACKOFF,
            outage_marked: false,
            station_retry_at: HashMap::new(),
        }
    }

    pub(crate) async fn run(mut self) -> anyhow::Result<()> {
        tracing::info!(driver = %self.name, endpoint = %self.endpoint(), jobs = self.plan.jobs.len(), "modbus driver starting");
        for (id, _) in &self.plan.unreadable {
            self.store.set_quality(*id, Quality::Bad);
        }
        loop {
            if *self.shutdown.borrow() {
                break;
            }
            let cycle_start = Instant::now();
            if self.ctx.is_none() {
                if let Err(e) = self.connect().await {
                    self.connection_lost(&e);
                    let delay = self.backoff;
                    self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
                    match self.serve_writes_until(Instant::now() + delay).await {
                        Wake::Shutdown => break,
                        Wake::Deadline => continue,
                    }
                }
            }
            self.poll_cycle().await;
            if !self.keep_open {
                self.disconnect().await;
            }
            if let Wake::Shutdown = self.serve_writes_until(cycle_start + self.poll).await {
                break;
            }
        }
        self.disconnect().await;
        tracing::info!(driver = %self.name, "modbus driver stopped");
        Ok(())
    }

    fn endpoint(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    async fn connect(&mut self) -> Result<(), String> {
        let endpoint = self.endpoint();
        let addr: SocketAddr = match tokio::time::timeout(self.timeout, tokio::net::lookup_host(&endpoint)).await {
            Ok(Ok(mut addrs)) => addrs.next().ok_or_else(|| format!("{endpoint} did not resolve"))?,
            Ok(Err(e)) => return Err(format!("cannot resolve {endpoint}: {e}")),
            Err(_) => return Err(format!("resolving {endpoint} timed out")),
        };
        let ctx = match tokio::time::timeout(self.timeout, tcp::connect(addr)).await {
            Ok(Ok(ctx)) => ctx,
            Ok(Err(e)) => return Err(format!("cannot connect to {endpoint}: {e}")),
            Err(_) => return Err(format!("connecting to {endpoint} timed out")),
        };
        if self.outage_marked || self.stats.snapshot().connects > 0 {
            tracing::info!(driver = %self.name, %endpoint, "connected");
        } else {
            tracing::debug!(driver = %self.name, %endpoint, "connected");
        }
        self.ctx = Some(ctx);
        self.backoff = MIN_BACKOFF;
        self.outage_marked = false;
        self.stats.connected();
        Ok(())
    }

    async fn disconnect(&mut self) {
        if let Some(mut ctx) = self.ctx.take() {
            // The peer may already be gone; a failed goodbye changes nothing.
            let _ = tokio::time::timeout(Duration::from_millis(200), ctx.disconnect()).await;
        }
    }

    /// The connection is gone: every tag of this driver is bad until it is back.
    fn connection_lost(&mut self, error: &str) {
        self.ctx = None;
        self.stats.disconnected(error);
        if !self.outage_marked {
            tracing::warn!(driver = %self.name, "{error}; retrying with backoff");
            for &id in &self.plan.tags {
                self.store.set_quality(id, Quality::Bad);
            }
            self.outage_marked = true;
        } else {
            tracing::debug!(driver = %self.name, "{error}");
        }
    }

    async fn poll_cycle(&mut self) {
        let now = Instant::now();
        // Stations that timed out in this cycle: their other jobs are skipped,
        // every other station on the connection is still polled.
        let mut silent: HashSet<String> = HashSet::new();
        for i in 0..self.plan.jobs.len() {
            let station = self.plan.jobs[i].station.clone();
            if silent.contains(&station) || self.station_retry_at.get(&station).is_some_and(|(at, _)| *at > now) {
                continue;
            }
            let Some(ctx) = self.ctx.as_mut() else { return };
            let job = &self.plan.jobs[i];
            ctx.set_slave(Slave(job.unit_id));
            let result = match job.area {
                ModbusArea::Coil => request(self.timeout, ctx.read_coils(job.address, job.count)).await.map(Data::Bits),
                ModbusArea::DiscreteInput => {
                    request(self.timeout, ctx.read_discrete_inputs(job.address, job.count)).await.map(Data::Bits)
                }
                ModbusArea::HoldingRegister => {
                    request(self.timeout, ctx.read_holding_registers(job.address, job.count)).await.map(Data::Words)
                }
                ModbusArea::InputRegister => {
                    request(self.timeout, ctx.read_input_registers(job.address, job.count)).await.map(Data::Words)
                }
            };
            match result {
                Ok(data) => {
                    self.stats.request_ok(Some(&station), now_ms());
                    self.station_retry_at.remove(&station);
                    publish(&self.store, &self.plan.jobs[i], &data);
                }
                Err(ReqError::Transport(e)) => {
                    self.stats.request_failed(Some(&station), &e);
                    self.connection_lost(&format!("connection to {} lost: {e}", self.endpoint()));
                    return;
                }
                Err(ReqError::Exception(exc)) => {
                    let msg = format!("station {station}: {}", ReqError::Exception(exc));
                    self.stats.request_failed(Some(&station), &msg);
                    tracing::debug!(driver = %self.name, "{msg}");
                    for slot in &self.plan.jobs[i].slots {
                        self.store.set_quality(slot.id, Quality::Bad);
                    }
                }
                Err(ReqError::Timeout) => {
                    self.station_timed_out(&station);
                    silent.insert(station);
                    // A late reply would be taken for the answer to the next
                    // request: carry on with the other stations on a fresh connection.
                    self.disconnect().await;
                    if let Err(e) = self.connect().await {
                        self.connection_lost(&e);
                        return;
                    }
                }
            }
        }
    }

    /// A silent station degrades to uncertain, then to bad after `max_retries`
    /// misses, and is polled less often while it stays silent.
    fn station_timed_out(&mut self, station: &str) {
        let msg = format!("station {station} did not answer within {} ms", self.timeout.as_millis());
        let failures = self.stats.request_failed(Some(station), &msg);
        let max = self.plan.stations.get(station).map_or(3, |s| s.max_retries.max(1));
        let quality = if failures >= max { Quality::Bad } else { Quality::Uncertain };
        let ids: Vec<TagId> = self.plan.station_tags(station).collect();
        for id in ids {
            self.store.set_quality(id, quality);
        }
        if failures >= max {
            let prev = self.station_retry_at.get(station).map_or(self.poll, |(_, d)| *d);
            let delay = (prev * 2).min(MAX_BACKOFF);
            self.station_retry_at.insert(station.to_string(), (Instant::now() + delay, delay));
            if failures == max {
                tracing::warn!(driver = %self.name, "{msg}; tags marked bad");
            }
        }
    }

    /// Waits for `deadline`, executing writes as they arrive.
    async fn serve_writes_until(&mut self, deadline: Instant) -> Wake {
        loop {
            tokio::select! {
                biased;
                changed = self.shutdown.changed() => {
                    if changed.is_err() || *self.shutdown.borrow() {
                        return Wake::Shutdown;
                    }
                }
                Some(req) = self.writes.recv() => self.write(req).await,
                () = tokio::time::sleep_until(deadline) => return Wake::Deadline,
            }
        }
    }

    async fn write(&mut self, req: WriteRequest) {
        let result = self.connect_and_write(req.id, &req.value).await;
        match &result {
            Ok(()) => {
                self.store.update(req.id, req.value, Quality::Good);
            }
            Err(e) => tracing::info!(driver = %self.name, tag = self.store.name(req.id), "write failed: {e}"),
        }
        // The caller may have given up waiting; nothing to do then.
        let _ = req.reply.send(result);
    }

    /// Writes on the open connection, or on one opened for this write: with
    /// `keep_open = false` (and after a poll timeout) the connection is closed
    /// between poll cycles, which is exactly when writes are served.
    async fn connect_and_write(&mut self, id: TagId, value: &Value) -> Result<(), WriteError> {
        if self.ctx.is_none() {
            if let Err(e) = self.connect().await {
                let endpoint = self.endpoint();
                self.connection_lost(&e);
                return Err(WriteError::Device(format!("not connected to {endpoint}: {e}")));
            }
            let result = self.execute_write(id, value).await;
            if !self.keep_open {
                self.disconnect().await;
            }
            return result;
        }
        self.execute_write(id, value).await
    }

    async fn execute_write(&mut self, id: TagId, value: &Value) -> Result<(), WriteError> {
        let def = self.store.def(id).clone();
        let Some(Address::Modbus(a)) = &def.address else {
            return Err(WriteError::Device(format!("tag {:?} has no Modbus address", def.name)));
        };
        let station = self
            .plan
            .stations
            .get(&a.station)
            .ok_or_else(|| WriteError::Device(format!("unknown station {:?}", a.station)))?;
        let unit_id = station.unit_id;
        let addr = station
            .protocol_address(a.register)
            .ok_or_else(|| WriteError::Device("register 0 does not exist with one-based addressing".into()))?;
        if !a.area.is_writable() {
            return Err(WriteError::ReadOnly(def.name.clone()));
        }
        let endpoint = self.endpoint();
        let timeout = self.timeout;
        let ctx = self
            .ctx
            .as_mut()
            .ok_or_else(|| WriteError::Device(format!("not connected to {endpoint}")))?;
        ctx.set_slave(Slave(unit_id));
        let result = if a.area == ModbusArea::Coil {
            let bit = encode_bit(def.data_type, value).map_err(|e| WriteError::BadValue(e.to_string()))?;
            match a.write {
                ModbusWrite::Multiple => request(timeout, ctx.write_multiple_coils(addr, &[bit])).await,
                ModbusWrite::Single | ModbusWrite::Mask => request(timeout, ctx.write_single_coil(addr, bit)).await,
            }
        } else {
            let words = encode_registers(def.data_type, value).map_err(|e| WriteError::BadValue(e.to_string()))?;
            match (a.write, words.as_slice()) {
                (ModbusWrite::Single, [w]) => request(timeout, ctx.write_single_register(addr, *w)).await,
                // AND mask 0 replaces every bit with the OR mask.
                (ModbusWrite::Mask, [w]) => request(timeout, ctx.masked_write_register(addr, 0, *w)).await,
                // Values wider than one register always need FC16.
                _ => request(timeout, ctx.write_multiple_registers(addr, &words)).await,
            }
        };
        match result {
            Ok(()) => {
                self.stats.request_ok(Some(&a.station), now_ms());
                Ok(())
            }
            Err(ReqError::Exception(exc)) => {
                let msg = exc.to_string();
                self.stats.request_failed(Some(&a.station), &msg);
                Err(WriteError::Device(msg))
            }
            Err(ReqError::Timeout) => {
                self.stats.request_failed(Some(&a.station), "write timed out");
                self.disconnect().await;
                Err(WriteError::Timeout)
            }
            Err(ReqError::Transport(e)) => {
                self.stats.request_failed(Some(&a.station), &e);
                self.connection_lost(&format!("connection to {endpoint} lost: {e}"));
                Err(WriteError::Device(e))
            }
        }
    }
}

enum Data {
    Bits(Vec<bool>),
    Words(Vec<u16>),
}

/// Decodes a read response into the store.
fn publish(store: &TagStore, job: &ReadJob, data: &Data) {
    for slot in &job.slots {
        let offset = slot.offset as usize;
        let decoded = match data {
            Data::Bits(bits) => bits.get(offset).map(|&b| Ok(decode_bit(slot.data_type, b))),
            Data::Words(words) => words.get(offset..).map(|w| decode_registers(slot.data_type, w)),
        };
        match decoded {
            Some(Ok(v)) => {
                store.update(slot.id, v, Quality::Good);
            }
            Some(Err(e)) => {
                tracing::debug!(tag = store.name(slot.id), "cannot decode: {e}");
                store.update(slot.id, Value::Null, Quality::Bad);
            }
            None => {
                tracing::debug!(tag = store.name(slot.id), "response shorter than requested");
                store.set_quality(slot.id, Quality::Bad);
            }
        }
    }
}

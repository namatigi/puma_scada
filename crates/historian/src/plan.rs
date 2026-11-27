//! What to record, resolved once from the project: which tags each enabled
//! historian records, which data loggers exist, and the journal series ids
//! they write to.

use crate::db::Db;
use moviclone_model::{Project, Recording};
use moviclone_tagdb::{TagId, TagStore};
use std::collections::HashMap;
use std::time::Duration;

/// Retention for series the project no longer configures (tag removed, logger
/// columns changed): kept as long as the longest configured retention.
const ORPHAN_DEFAULT_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);

#[derive(Debug, Clone)]
pub struct HistorianPlan {
    pub name: String,
    pub database: String,
    pub max_age: Duration,
    pub tags: Vec<TagId>,
}

#[derive(Debug, Clone)]
pub struct LoggerPlan {
    pub name: String,
    pub database: String,
    pub table: String,
    pub record: Recording,
    pub max_age: Duration,
    /// Column name and tag, in configured order.
    pub columns: Vec<(String, TagId)>,
    pub series: u32,
}

/// What a journal series holds, for decoding entries.
#[derive(Debug, Clone)]
pub enum SeriesInfo {
    Tag { tag: String, historian: usize },
    Logger { logger: usize },
}

#[derive(Debug, Default)]
pub struct Plan {
    pub historians: Vec<HistorianPlan>,
    pub loggers: Vec<LoggerPlan>,
    /// Per tag: (historian index, series id) when the tag is historized.
    pub tag_series: Vec<Option<(usize, u32)>>,
    /// Per tag: on-change loggers that have it as a column.
    pub tag_loggers: Vec<Vec<usize>>,
    pub series: HashMap<u32, SeriesInfo>,
    /// Every series id in the database with its retention, including orphans.
    pub retention: Vec<(u32, Duration)>,
}

impl Plan {
    pub fn build(project: &Project, store: &TagStore, db: &Db) -> anyhow::Result<Plan> {
        let mut plan = Plan {
            tag_series: vec![None; store.len()],
            tag_loggers: vec![Vec::new(); store.len()],
            ..Plan::default()
        };
        let mut hist_index = HashMap::new();
        for h in &project.data.historians {
            if !h.enabled {
                continue;
            }
            hist_index.insert(h.name.as_str(), plan.historians.len());
            plan.historians.push(HistorianPlan {
                name: h.name.clone(),
                database: h.database.clone(),
                max_age: Duration::from_secs(h.max_age_s),
                tags: Vec::new(),
            });
        }

        let mut keys = Vec::new();
        let mut tag_keys = Vec::new();
        for id in store.ids() {
            let def = store.def(id);
            let Some(hname) = &def.historian else { continue };
            match hist_index.get(hname.as_str()) {
                Some(&hi) => {
                    plan.historians[hi].tags.push(id);
                    let key = format!("tag:{}", def.name);
                    keys.push(key.clone());
                    tag_keys.push((id, hi, key));
                }
                None if project.data.historians.iter().any(|h| &h.name == hname) => {}
                None => tracing::warn!(tag = %def.name, historian = %hname, "tag refers to an unknown historian; not recorded"),
            }
        }

        let mut logger_keys = Vec::new();
        for l in &project.data.loggers {
            if !l.enabled {
                continue;
            }
            let mut columns = Vec::new();
            for c in &l.columns {
                match store.id(&c.tag) {
                    Some(id) => columns.push((c.name.clone(), id)),
                    None => tracing::warn!(logger = %l.name, column = %c.name, tag = %c.tag, "logger column refers to an unknown tag; skipped"),
                }
            }
            // The column list is part of the key: rows are stored positionally,
            // so a changed column set starts a new series instead of misreading old rows.
            let sig: Vec<_> = columns.iter().map(|(n, _)| n.as_str()).collect();
            let key = format!("logger:{}:{}", l.name, sig.join(","));
            keys.push(key.clone());
            logger_keys.push(key);
            plan.loggers.push(LoggerPlan {
                name: l.name.clone(),
                database: l.database.clone(),
                table: l.table.clone(),
                record: l.record.clone(),
                max_age: Duration::from_secs(l.max_age_s),
                columns,
                series: 0,
            });
        }

        let ids = db.series_ids(&keys)?;
        let mut retention_by_id = HashMap::new();
        for (id, hi, key) in tag_keys {
            let sid = ids[&key];
            plan.tag_series[id as usize] = Some((hi, sid));
            plan.series.insert(sid, SeriesInfo::Tag { tag: store.name(id).to_string(), historian: hi });
            retention_by_id.insert(sid, plan.historians[hi].max_age);
        }
        for (li, key) in logger_keys.into_iter().enumerate() {
            let sid = ids[&key];
            let logger = &mut plan.loggers[li];
            logger.series = sid;
            plan.series.insert(sid, SeriesInfo::Logger { logger: li });
            retention_by_id.insert(sid, logger.max_age);
            if logger.record == Recording::OnChange {
                for (_, tag) in &logger.columns {
                    plan.tag_loggers[*tag as usize].push(li);
                }
            }
        }

        let orphan_max_age = retention_by_id.values().copied().max().unwrap_or(ORPHAN_DEFAULT_MAX_AGE);
        for (_, sid) in db.all_series()? {
            let age = retention_by_id.get(&sid).copied().unwrap_or(orphan_max_age);
            plan.retention.push((sid, age));
        }
        Ok(plan)
    }

    pub fn historian(&self, name: &str) -> Option<usize> {
        self.historians.iter().position(|h| h.name == name)
    }

    pub fn logger(&self, name: &str) -> Option<usize> {
        self.loggers.iter().position(|l| l.name == name)
    }

    pub fn tag_series(&self, id: TagId) -> Option<u32> {
        self.tag_series.get(id as usize).copied().flatten().map(|(_, s)| s)
    }

    pub fn historized_tags(&self) -> impl Iterator<Item = TagId> + '_ {
        self.historians.iter().flat_map(|h| h.tags.iter().copied())
    }

    /// Database name of the historian or logger a series belongs to.
    pub fn database_of(&self, series: u32) -> Option<&str> {
        match self.series.get(&series)? {
            SeriesInfo::Tag { historian, .. } => Some(&self.historians[*historian].database),
            SeriesInfo::Logger { logger } => Some(&self.loggers[*logger].database),
        }
    }
}

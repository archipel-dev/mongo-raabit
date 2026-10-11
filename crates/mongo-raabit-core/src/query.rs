//! The collection tab's query model: filter / projection / sort / skip / limit,
//! paging, counting, explain and document CRUD.

use crate::connection::Connection;
use crate::error::{RaabitError, Result};
use crate::shell;
use futures_util::TryStreamExt;
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// The query bar as the user typed it (shell syntax).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryText {
    pub filter: String,
    pub projection: String,
    pub sort: String,
    pub skip: String,
    pub limit: String,
}

/// A validated query.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    pub filter: Document,
    pub projection: Option<Document>,
    pub sort: Option<Document>,
    pub skip: u64,
    /// 0 means no limit.
    pub limit: u64,
}

impl QueryText {
    pub fn parse(&self) -> Result<Query> {
        let optional = |text: &str, what: &'static str| -> Result<Option<Document>> {
            let doc = shell::parse_document(text).map_err(|e| RaabitError::parse(what, e))?;
            Ok((!doc.is_empty()).then_some(doc))
        };
        let number = |text: &str, what: &str| -> Result<u64> {
            let text = text.trim();
            if text.is_empty() {
                return Ok(0);
            }
            text.parse::<u64>().map_err(|_| {
                RaabitError::Invalid(format!(
                    "{what} must be a non-negative integer, got '{text}'"
                ))
            })
        };
        Ok(Query {
            filter: shell::parse_document(&self.filter)
                .map_err(|e| RaabitError::parse("filter", e))?,
            projection: optional(&self.projection, "projection")?,
            sort: optional(&self.sort, "sort")?,
            skip: number(&self.skip, "skip")?,
            limit: number(&self.limit, "limit")?,
        })
    }
}

impl Query {
    /// The `find` call in mongo shell syntax, for the "Query Code" view and history.
    pub fn to_shell(&self, collection: &str) -> String {
        let style = shell::ShellStyle::Compact;
        let mut out = format!(
            "db.getCollection(\"{}\").find({}",
            collection.replace('"', "\\\""),
            shell::document_to_shell(&self.filter, style)
        );
        if let Some(projection) = &self.projection {
            out.push_str(", ");
            out.push_str(&shell::document_to_shell(projection, style));
        }
        out.push(')');
        if let Some(sort) = &self.sort {
            out.push_str(&format!(".sort({})", shell::document_to_shell(sort, style)));
        }
        if self.skip > 0 {
            out.push_str(&format!(".skip({})", self.skip));
        }
        if self.limit > 0 {
            out.push_str(&format!(".limit({})", self.limit));
        }
        out
    }

    /// Skip and limit of one result page, honoring the query's own skip/limit.
    /// Returns `None` when the page lies beyond the query limit.
    pub fn page_window(&self, page: u64, page_size: u64) -> Option<(u64, u64)> {
        let page_size = page_size.max(1);
        let offset = page.checked_mul(page_size)?;
        let size = if self.limit > 0 {
            if offset >= self.limit {
                return None;
            }
            page_size.min(self.limit - offset)
        } else {
            page_size
        };
        Some((self.skip.checked_add(offset)?, size))
    }
}

/// One page of results.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub documents: Vec<Document>,
    pub page: u64,
    pub page_size: u64,
    pub has_more: bool,
    pub elapsed: Duration,
}

impl Page {
    /// 1-based index of the first document on this page (0 when empty).
    pub fn first_index(&self) -> u64 {
        if self.documents.is_empty() {
            0
        } else {
            self.page * self.page_size + 1
        }
    }
}

/// Summary of an explain plan, plus the raw server reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Explain {
    pub root: PlanStage,
    pub n_returned: u64,
    pub docs_examined: u64,
    pub keys_examined: u64,
    pub execution_ms: u64,
    pub indexes_used: Vec<String>,
    pub raw: Document,
}

/// One node of the winning plan tree.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanStage {
    pub stage: String,
    pub index_name: Option<String>,
    pub n_returned: Option<u64>,
    pub docs_examined: Option<u64>,
    pub keys_examined: Option<u64>,
    pub time_ms: Option<u64>,
    pub children: Vec<PlanStage>,
}

impl PlanStage {
    fn from_doc(doc: &Document) -> Self {
        let num = |key: &str| match doc.get(key) {
            Some(Bson::Int32(v)) => u64::try_from(*v).ok(),
            Some(Bson::Int64(v)) => u64::try_from(*v).ok(),
            Some(Bson::Double(v)) if *v >= 0.0 => Some(*v as u64),
            _ => None,
        };
        let mut children = Vec::new();
        if let Ok(child) = doc.get_document("inputStage") {
            children.push(Self::from_doc(child));
        }
        for key in ["inputStages", "shards"] {
            if let Ok(items) = doc.get_array(key) {
                for item in items.iter().filter_map(Bson::as_document) {
                    let item = item
                        .get_document("executionStages")
                        .or_else(|_| item.get_document("winningPlan"))
                        .unwrap_or(item);
                    children.push(Self::from_doc(item));
                }
            }
        }
        Self {
            stage: doc
                .get_str("stage")
                .or_else(|_| doc.get_str("shardName"))
                .unwrap_or("?")
                .to_owned(),
            index_name: doc.get_str("indexName").ok().map(str::to_owned),
            n_returned: num("nReturned"),
            docs_examined: num("docsExamined"),
            keys_examined: num("keysExamined"),
            time_ms: num("executionTimeMillisEstimate"),
            children,
        }
    }

    fn collect_indexes(&self, out: &mut Vec<String>) {
        if let Some(name) = &self.index_name
            && !out.contains(name)
        {
            out.push(name.clone());
        }
        for child in &self.children {
            child.collect_indexes(out);
        }
    }

    /// True when any stage scans the whole collection.
    pub fn has_collection_scan(&self) -> bool {
        self.stage == "COLLSCAN" || self.children.iter().any(Self::has_collection_scan)
    }
}

impl Explain {
    pub fn from_reply(raw: Document) -> Self {
        let stats = raw.get_document("executionStats").ok();
        let num = |key: &str| -> u64 {
            match stats.and_then(|s| s.get(key)) {
                Some(Bson::Int32(v)) => u64::try_from(*v).unwrap_or(0),
                Some(Bson::Int64(v)) => u64::try_from(*v).unwrap_or(0),
                Some(Bson::Double(v)) if *v >= 0.0 => *v as u64,
                _ => 0,
            }
        };
        let planner = raw.get_document("queryPlanner").ok();
        let plan_doc = stats
            .and_then(|s| s.get_document("executionStages").ok())
            .or_else(|| {
                planner
                    .and_then(|p| p.get_document("winningPlan").ok())
                    .map(|w| {
                        // 7.0+ wraps the classic plan in `queryPlan` for SBE.
                        w.get_document("queryPlan").unwrap_or(w)
                    })
            });
        let root = plan_doc.map(PlanStage::from_doc).unwrap_or(PlanStage {
            stage: "UNKNOWN".into(),
            index_name: None,
            n_returned: None,
            docs_examined: None,
            keys_examined: None,
            time_ms: None,
            children: Vec::new(),
        });
        let mut indexes_used = Vec::new();
        root.collect_indexes(&mut indexes_used);
        Self {
            n_returned: num("nReturned"),
            docs_examined: num("totalDocsExamined"),
            keys_examined: num("totalKeysExamined"),
            execution_ms: num("executionTimeMillis"),
            indexes_used,
            root,
            raw,
        }
    }
}

/// `_id` of a document, required for edits.
fn id_of(document: &Document) -> Result<Bson> {
    document
        .get("_id")
        .cloned()
        .ok_or_else(|| RaabitError::Invalid("the document has no _id field".into()))
}

impl Connection {
    /// Fetch one page; fetches one extra document to know whether more exist.
    pub async fn find_page(
        &self,
        db: &str,
        collection: &str,
        query: &Query,
        page: u64,
        page_size: u64,
    ) -> Result<Page> {
        let page_size = page_size.clamp(1, 10_000);
        let Some((skip, size)) = query.page_window(page, page_size) else {
            return Ok(Page {
                page,
                page_size,
                ..Page::default()
            });
        };
        let limit_reached = query.limit > 0 && (page + 1) * page_size >= query.limit;
        let fetch = if limit_reached { size } else { size + 1 };
        let start = Instant::now();
        let handle = self.collection(db, collection);
        let mut find = handle
            .find(query.filter.clone())
            .skip(skip)
            .limit(i64::try_from(fetch).unwrap_or(i64::MAX));
        if let Some(projection) = &query.projection {
            find = find.projection(projection.clone());
        }
        if let Some(sort) = &query.sort {
            find = find.sort(sort.clone());
        }
        let mut documents: Vec<Document> = find.await?.try_collect().await?;
        let elapsed = start.elapsed();
        let has_more = documents.len() as u64 > size;
        documents.truncate(size as usize);
        Ok(Page {
            documents,
            page,
            page_size,
            has_more,
            elapsed,
        })
    }

    /// Number of documents the query matches (respecting its skip and limit).
    pub async fn count(&self, db: &str, collection: &str, query: &Query) -> Result<u64> {
        let handle = self.collection(db, collection);
        let mut count = handle.count_documents(query.filter.clone());
        if query.skip > 0 {
            count = count.skip(query.skip);
        }
        if query.limit > 0 {
            count = count.limit(query.limit);
        }
        Ok(count.await?)
    }

    /// Explain the query with `executionStats` verbosity.
    pub async fn explain(&self, db: &str, collection: &str, query: &Query) -> Result<Explain> {
        let mut find = doc! {"find": collection, "filter": query.filter.clone()};
        if let Some(projection) = &query.projection {
            find.insert("projection", projection.clone());
        }
        if let Some(sort) = &query.sort {
            find.insert("sort", sort.clone());
        }
        if query.skip > 0 {
            find.insert("skip", i64::try_from(query.skip).unwrap_or(i64::MAX));
        }
        if query.limit > 0 {
            find.insert("limit", i64::try_from(query.limit).unwrap_or(i64::MAX));
        }
        let raw = self
            .run_command(db, doc! {"explain": find, "verbosity": "executionStats"})
            .await?;
        Ok(Explain::from_reply(raw))
    }

    /// Insert documents, adding `_id`s where missing. Returns the ids in order.
    pub async fn insert_documents(
        &self,
        db: &str,
        collection: &str,
        mut documents: Vec<Document>,
    ) -> Result<Vec<Bson>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        for document in &mut documents {
            if !document.contains_key("_id") {
                // Keep `_id` first, as the shell does.
                let mut with_id = doc! {"_id": ObjectId::new()};
                with_id.extend(std::mem::take(document));
                *document = with_id;
            }
        }
        let ids = documents
            .iter()
            .map(|d| d.get("_id").cloned().unwrap_or(Bson::Null))
            .collect();
        self.collection(db, collection)
            .insert_many(documents)
            .await?;
        Ok(ids)
    }

    /// Replace a whole document (the document editor's Save). The `_id` must not change.
    pub async fn replace_document(
        &self,
        db: &str,
        collection: &str,
        original_id: &Bson,
        replacement: Document,
    ) -> Result<()> {
        if let Some(new_id) = replacement.get("_id")
            && new_id != original_id
        {
            return Err(RaabitError::Invalid(
                "_id cannot be changed; clone the document instead".into(),
            ));
        }
        let result = self
            .collection(db, collection)
            .replace_one(doc! {"_id": original_id.clone()}, replacement)
            .await?;
        if result.matched_count == 0 {
            return Err(RaabitError::Invalid(
                "the document no longer exists (deleted by someone else?)".into(),
            ));
        }
        Ok(())
    }

    /// Set (or with `None`, unset) one field path of a document — inline table edits.
    pub async fn set_field(
        &self,
        db: &str,
        collection: &str,
        id: &Bson,
        path: &str,
        value: Option<Bson>,
    ) -> Result<()> {
        if path.is_empty() || path == "_id" || path.starts_with("_id.") {
            return Err(RaabitError::Invalid("_id cannot be edited in place".into()));
        }
        let update = match value {
            Some(value) => doc! {"$set": {path: value}},
            None => doc! {"$unset": {path: ""}},
        };
        let result = self
            .collection(db, collection)
            .update_one(doc! {"_id": id.clone()}, update)
            .await?;
        if result.matched_count == 0 {
            return Err(RaabitError::Invalid("the document no longer exists".into()));
        }
        Ok(())
    }

    /// Delete documents by `_id`. Returns how many were removed.
    pub async fn delete_documents(&self, db: &str, collection: &str, ids: &[Bson]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let result = self
            .collection(db, collection)
            .delete_many(doc! {"_id": {"$in": ids.to_vec()}})
            .await?;
        Ok(result.deleted_count)
    }

    /// Insert a copy of the document under a fresh `ObjectId`; returns the new id.
    pub async fn clone_document(
        &self,
        db: &str,
        collection: &str,
        document: &Document,
    ) -> Result<Bson> {
        id_of(document)?;
        let mut copy = document.clone();
        copy.remove("_id");
        let ids = self.insert_documents(db, collection, vec![copy]).await?;
        Ok(ids.into_iter().next().unwrap_or(Bson::Null))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_query_bar() {
        let query = QueryText {
            filter: "{age: {$gt: 30}}".into(),
            projection: "{name: 1}".into(),
            sort: "".into(),
            skip: "5".into(),
            limit: " ".into(),
        }
        .parse()
        .unwrap();
        assert_eq!(query.filter, doc! {"age": {"$gt": 30}});
        assert_eq!(query.projection, Some(doc! {"name": 1}));
        assert_eq!(query.sort, None);
        assert_eq!((query.skip, query.limit), (5, 0));
        assert_eq!(
            query.to_shell("people"),
            "db.getCollection(\"people\").find({ age: { $gt: 30 } }, { name: 1 }).skip(5)"
        );
        let bad = QueryText {
            filter: "{age: }".into(),
            ..QueryText::default()
        }
        .parse()
        .unwrap_err();
        assert!(bad.to_string().starts_with("invalid filter"));
        assert!(
            QueryText {
                limit: "-1".into(),
                ..QueryText::default()
            }
            .parse()
            .is_err()
        );
    }

    #[test]
    fn pages_respect_skip_and_limit() {
        let query = Query {
            skip: 10,
            limit: 45,
            ..Query::default()
        };
        assert_eq!(query.page_window(0, 20), Some((10, 20)));
        assert_eq!(query.page_window(2, 20), Some((50, 5)));
        assert_eq!(query.page_window(3, 20), None);
        assert_eq!(Query::default().page_window(4, 50), Some((200, 50)));
    }

    #[test]
    fn summarizes_explain_reply() {
        let raw = doc! {
            "queryPlanner": {"winningPlan": {"stage": "FETCH", "inputStage": {"stage": "IXSCAN", "indexName": "age_1"}}},
            "executionStats": {
                "nReturned": 3, "totalDocsExamined": 3, "totalKeysExamined": 4, "executionTimeMillis": 1,
                "executionStages": {"stage": "FETCH", "nReturned": 3, "inputStage": {"stage": "IXSCAN", "indexName": "age_1", "keysExamined": 4}}
            }
        };
        let explain = Explain::from_reply(raw);
        assert_eq!(explain.indexes_used, ["age_1"]);
        assert_eq!(explain.keys_examined, 4);
        assert_eq!(explain.root.children[0].keys_examined, Some(4));
        assert!(!explain.root.has_collection_scan());
    }
}

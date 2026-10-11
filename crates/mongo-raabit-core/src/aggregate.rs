//! Aggregation Editor model: a pipeline of individually editable, toggleable stages
//! with per-stage input/output previews.

use crate::connection::Connection;
use crate::error::{RaabitError, Result};
use crate::shell::{self, ShellStyle};
use futures_util::TryStreamExt;
use mongodb::bson::{Bson, Document, doc};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// A stage operator with a one-line description and a starter template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageOperator {
    pub name: &'static str,
    pub description: &'static str,
    pub template: &'static str,
    /// Writes data; never executed for previews.
    pub writes: bool,
}

const fn op(
    name: &'static str,
    description: &'static str,
    template: &'static str,
) -> StageOperator {
    StageOperator {
        name,
        description,
        template,
        writes: false,
    }
}

/// Stage operators offered by the editor's operator picker and autocomplete.
pub const STAGE_OPERATORS: &[StageOperator] = &[
    op("$match", "Filter documents", "{\n  field: \"value\"\n}"),
    op(
        "$project",
        "Include, exclude or compute fields",
        "{\n  field: 1\n}",
    ),
    op(
        "$addFields",
        "Add computed fields",
        "{\n  newField: \"$field\"\n}",
    ),
    op(
        "$set",
        "Alias of $addFields",
        "{\n  newField: \"$field\"\n}",
    ),
    op("$unset", "Remove fields", "[\"field\"]"),
    op(
        "$group",
        "Group by a key and accumulate",
        "{\n  _id: \"$field\",\n  count: { $sum: 1 }\n}",
    ),
    op("$sort", "Order documents", "{\n  field: 1\n}"),
    op("$limit", "Keep the first n documents", "10"),
    op("$skip", "Skip the first n documents", "10"),
    op(
        "$unwind",
        "One document per array element",
        "{\n  path: \"$array\",\n  preserveNullAndEmptyArrays: false\n}",
    ),
    op(
        "$lookup",
        "Left outer join with another collection",
        "{\n  from: \"collection\",\n  localField: \"field\",\n  foreignField: \"_id\",\n  as: \"joined\"\n}",
    ),
    op("$count", "Count documents into a field", "\"count\""),
    op(
        "$facet",
        "Run sub-pipelines on the same input",
        "{\n  facetName: [ { $count: \"n\" } ]\n}",
    ),
    op(
        "$bucket",
        "Group into ranges",
        "{\n  groupBy: \"$field\",\n  boundaries: [0, 100, 1000],\n  default: \"other\"\n}",
    ),
    op(
        "$bucketAuto",
        "Group into n even buckets",
        "{\n  groupBy: \"$field\",\n  buckets: 5\n}",
    ),
    op("$sortByCount", "Group, count and sort", "\"$field\""),
    op(
        "$replaceRoot",
        "Promote an embedded document",
        "{\n  newRoot: \"$field\"\n}",
    ),
    op("$replaceWith", "Promote an embedded document", "\"$field\""),
    op("$sample", "Random sample", "{\n  size: 10\n}"),
    op(
        "$graphLookup",
        "Recursive lookup",
        "{\n  from: \"collection\",\n  startWith: \"$field\",\n  connectFromField: \"field\",\n  connectToField: \"_id\",\n  as: \"graph\"\n}",
    ),
    op(
        "$unionWith",
        "Append another collection",
        "{\n  coll: \"collection\",\n  pipeline: []\n}",
    ),
    op(
        "$geoNear",
        "Order by distance (first stage)",
        "{\n  near: { type: \"Point\", coordinates: [0, 0] },\n  distanceField: \"distance\"\n}",
    ),
    op(
        "$redact",
        "Restrict content by expression",
        "{\n  $cond: { if: true, then: \"$$KEEP\", else: \"$$PRUNE\" }\n}",
    ),
    op(
        "$setWindowFields",
        "Window functions",
        "{\n  sortBy: { field: 1 },\n  output: { running: { $sum: \"$field\", window: { documents: [\"unbounded\", \"current\"] } } }\n}",
    ),
    op(
        "$densify",
        "Fill gaps in a sequence",
        "{\n  field: \"field\",\n  range: { step: 1, bounds: \"full\" }\n}",
    ),
    op(
        "$fill",
        "Fill null and missing values",
        "{\n  output: { field: { method: \"locf\" } }\n}",
    ),
    op("$documents", "Literal input documents", "[ { x: 1 } ]"),
    op(
        "$collStats",
        "Collection statistics (first stage)",
        "{\n  storageStats: {}\n}",
    ),
    op("$indexStats", "Index usage (first stage)", "{}"),
    StageOperator {
        name: "$out",
        description: "Write results to a collection (last stage)",
        template: "\"collection\"",
        writes: true,
    },
    StageOperator {
        name: "$merge",
        description: "Merge results into a collection (last stage)",
        template: "{\n  into: \"collection\",\n  whenMatched: \"merge\",\n  whenNotMatched: \"insert\"\n}",
        writes: true,
    },
];

/// Expression and query operators for autocomplete inside stage bodies.
pub const EXPRESSION_OPERATORS: &[&str] = &[
    "$abs",
    "$add",
    "$addToSet",
    "$all",
    "$allElementsTrue",
    "$and",
    "$anyElementTrue",
    "$arrayElemAt",
    "$arrayToObject",
    "$avg",
    "$bottom",
    "$bottomN",
    "$ceil",
    "$cmp",
    "$concat",
    "$concatArrays",
    "$cond",
    "$convert",
    "$count",
    "$dateAdd",
    "$dateDiff",
    "$dateFromString",
    "$dateToString",
    "$dateTrunc",
    "$dayOfMonth",
    "$dayOfWeek",
    "$dayOfYear",
    "$divide",
    "$elemMatch",
    "$eq",
    "$exists",
    "$exp",
    "$expr",
    "$filter",
    "$first",
    "$firstN",
    "$floor",
    "$getField",
    "$gt",
    "$gte",
    "$hour",
    "$ifNull",
    "$in",
    "$indexOfArray",
    "$isArray",
    "$isNumber",
    "$last",
    "$lastN",
    "$let",
    "$literal",
    "$ln",
    "$log10",
    "$lt",
    "$lte",
    "$ltrim",
    "$map",
    "$max",
    "$maxN",
    "$median",
    "$mergeObjects",
    "$min",
    "$minN",
    "$minute",
    "$mod",
    "$month",
    "$multiply",
    "$ne",
    "$nin",
    "$nor",
    "$not",
    "$objectToArray",
    "$or",
    "$percentile",
    "$pow",
    "$push",
    "$range",
    "$reduce",
    "$regex",
    "$regexFind",
    "$regexMatch",
    "$replaceAll",
    "$reverseArray",
    "$round",
    "$rtrim",
    "$second",
    "$setDifference",
    "$setIntersection",
    "$setUnion",
    "$size",
    "$slice",
    "$sortArray",
    "$split",
    "$sqrt",
    "$stdDevPop",
    "$stdDevSamp",
    "$strLenCP",
    "$substrCP",
    "$subtract",
    "$sum",
    "$switch",
    "$toBool",
    "$toDate",
    "$toDecimal",
    "$toDouble",
    "$toInt",
    "$toLong",
    "$toLower",
    "$toObjectId",
    "$toString",
    "$toUpper",
    "$top",
    "$topN",
    "$trim",
    "$trunc",
    "$type",
    "$week",
    "$year",
    "$zip",
];

/// Operator names matching a typed prefix (case-insensitive), stages first.
pub fn complete_operator(prefix: &str) -> Vec<&'static str> {
    let prefix = prefix.to_ascii_lowercase();
    let mut out: Vec<&'static str> = STAGE_OPERATORS
        .iter()
        .map(|o| o.name)
        .filter(|n| n.to_ascii_lowercase().starts_with(&prefix))
        .collect();
    for name in EXPRESSION_OPERATORS {
        if name.to_ascii_lowercase().starts_with(&prefix) && !out.contains(name) {
            out.push(name);
        }
    }
    out
}

pub fn stage_operator(name: &str) -> Option<&'static StageOperator> {
    STAGE_OPERATORS.iter().find(|o| o.name == name)
}

/// One editable stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage {
    pub operator: String,
    /// Stage body in shell syntax (the value of the operator key).
    pub body: String,
    pub enabled: bool,
}

impl Stage {
    pub fn new(operator: &str) -> Self {
        Self {
            operator: operator.to_owned(),
            body: stage_operator(operator)
                .map(|o| o.template.to_owned())
                .unwrap_or_else(|| "{}".into()),
            enabled: true,
        }
    }

    /// The stage as a pipeline document.
    pub fn to_document(&self) -> Result<Document> {
        if !self.operator.starts_with('$') || self.operator.len() < 2 {
            return Err(RaabitError::Invalid(format!(
                "'{}' is not a stage operator",
                self.operator
            )));
        }
        let body = shell::parse_value(&self.body).map_err(|e| RaabitError::parse("stage", e))?;
        Ok(doc! { self.operator.clone(): body })
    }

    pub fn writes(&self) -> bool {
        stage_operator(&self.operator).is_some_and(|o| o.writes)
    }
}

/// An aggregation pipeline under edit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pipeline {
    pub stages: Vec<Stage>,
}

/// Options for running a pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOptions {
    pub allow_disk_use: bool,
    /// Feed only the first n input documents to the pipeline (0 = all), for fast previews.
    pub input_sample: u64,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            allow_disk_use: true,
            input_sample: 0,
        }
    }
}

/// Input and output of one stage, as shown below the stage editor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StagePreview {
    pub input: Vec<Document>,
    pub output: Vec<Document>,
    /// Set when the stage writes data and was therefore not executed.
    pub skipped_reason: Option<String>,
    pub elapsed: Duration,
}

impl Pipeline {
    /// Build stages from pipeline text, e.g. pasted from the shell or a file.
    pub fn from_text(text: &str) -> Result<Self> {
        let docs =
            shell::parse_document_array(text).map_err(|e| RaabitError::parse("pipeline", e))?;
        let stages = docs
            .into_iter()
            .map(|doc| {
                if doc.len() != 1 {
                    return Err(RaabitError::Invalid(format!(
                        "each stage needs exactly one operator, found {} keys",
                        doc.len()
                    )));
                }
                let (operator, body) = doc.into_iter().next().unwrap_or_default();
                Ok(Stage {
                    operator,
                    body: shell::to_shell_string(&body, ShellStyle::Pretty(2)),
                    enabled: true,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { stages })
    }

    /// Enabled stages up to and including `index` (all when `None`), as documents.
    /// Errors name the 1-based stage number.
    pub fn documents(&self, through: Option<usize>) -> Result<Vec<Document>> {
        let end = through.map_or(self.stages.len(), |i| (i + 1).min(self.stages.len()));
        self.stages[..end]
            .iter()
            .enumerate()
            .filter(|(_, s)| s.enabled)
            .map(|(i, s)| {
                s.to_document().map_err(|e| {
                    RaabitError::Invalid(format!("stage {} ({}): {e}", i + 1, s.operator))
                })
            })
            .collect()
    }

    /// Whole pipeline as mongo shell code.
    pub fn to_shell(&self, collection: &str, options: RunOptions) -> Result<String> {
        let stages = self.documents(None)?;
        let body = shell::to_shell_string(
            &Bson::Array(stages.into_iter().map(Bson::Document).collect()),
            ShellStyle::Pretty(2),
        );
        let opts = if options.allow_disk_use {
            ", { allowDiskUse: true }"
        } else {
            ""
        };
        Ok(format!(
            "db.getCollection(\"{}\").aggregate({body}{opts})",
            collection.replace('"', "\\\"")
        ))
    }

    pub fn move_stage(&mut self, from: usize, to: usize) {
        if from < self.stages.len() && to < self.stages.len() {
            let stage = self.stages.remove(from);
            self.stages.insert(to, stage);
        }
    }
}

fn sampled(mut stages: Vec<Document>, options: RunOptions) -> Vec<Document> {
    if options.input_sample > 0 {
        let limit = i64::try_from(options.input_sample).unwrap_or(i64::MAX);
        // Source stages must stay first.
        let first_is_source = stages
            .first()
            .and_then(|d| d.keys().next())
            .is_some_and(|k| {
                matches!(
                    k.as_str(),
                    "$geoNear"
                        | "$collStats"
                        | "$indexStats"
                        | "$documents"
                        | "$search"
                        | "$currentOp"
                )
            });
        let at = usize::from(first_is_source);
        stages.insert(at, doc! {"$limit": limit});
    }
    stages
}

impl Connection {
    /// Run an aggregation and collect at most `max_results` documents.
    pub async fn aggregate(
        &self,
        db: &str,
        collection: &str,
        stages: Vec<Document>,
        options: RunOptions,
        max_results: u64,
    ) -> Result<(Vec<Document>, Duration)> {
        let mut stages = sampled(stages, options);
        let writes = stages
            .last()
            .and_then(|d| d.keys().next())
            .is_some_and(|k| k == "$out" || k == "$merge");
        if max_results > 0 && !writes {
            stages.push(doc! {"$limit": i64::try_from(max_results).unwrap_or(i64::MAX)});
        }
        let start = Instant::now();
        let cursor = self
            .collection(db, collection)
            .aggregate(stages)
            .allow_disk_use(options.allow_disk_use)
            .await?;
        let docs: Vec<Document> = cursor.try_collect().await?;
        Ok((docs, start.elapsed()))
    }

    /// Preview the input and output of stage `index` (at most `limit` documents each).
    /// Disabled stages pass their input through; writing stages are not executed.
    pub async fn preview_stage(
        &self,
        db: &str,
        collection: &str,
        pipeline: &Pipeline,
        index: usize,
        limit: u64,
        options: RunOptions,
    ) -> Result<StagePreview> {
        let stage = pipeline
            .stages
            .get(index)
            .ok_or_else(|| RaabitError::Invalid(format!("no stage {}", index + 1)))?;
        let start = Instant::now();
        let before = if index == 0 {
            Vec::new()
        } else {
            pipeline.documents(Some(index - 1))?
        };
        if before
            .iter()
            .any(|d| d.contains_key("$out") || d.contains_key("$merge"))
        {
            return Err(RaabitError::Invalid(
                "$out and $merge must be the last stage".into(),
            ));
        }
        let (input, _) = self
            .aggregate(db, collection, before.clone(), options, limit)
            .await?;
        if stage.writes() && stage.enabled {
            return Ok(StagePreview {
                output: Vec::new(),
                skipped_reason: Some(format!(
                    "{} writes data; run the full pipeline to execute it",
                    stage.operator
                )),
                input,
                elapsed: start.elapsed(),
            });
        }
        let through = pipeline.documents(Some(index))?;
        let output = if stage.enabled {
            self.aggregate(db, collection, through, options, limit)
                .await?
                .0
        } else {
            input.clone()
        };
        Ok(StagePreview {
            input,
            output,
            skipped_reason: None,
            elapsed: start.elapsed(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_renders_pipelines() {
        let mut pipeline = Pipeline::from_text(
            "[{$match: {a: 1}}, {$group: {_id: '$b', n: {$sum: 1}}}, {$limit: 5}]",
        )
        .unwrap();
        assert_eq!(pipeline.stages.len(), 3);
        assert_eq!(pipeline.stages[2].body, "5");
        pipeline.stages[1].enabled = false;
        let docs = pipeline.documents(None).unwrap();
        assert_eq!(docs, vec![doc! {"$match": {"a": 1}}, doc! {"$limit": 5}]);
        assert_eq!(pipeline.documents(Some(0)).unwrap().len(), 1);
        let code = pipeline.to_shell("orders", RunOptions::default()).unwrap();
        assert!(code.starts_with("db.getCollection(\"orders\").aggregate(["));
        assert!(code.ends_with("{ allowDiskUse: true })"));
        pipeline.move_stage(2, 0);
        assert_eq!(pipeline.stages[0].operator, "$limit");
        assert!(Pipeline::from_text("[{$match: {}, $limit: 1}]").is_err());
    }

    #[test]
    fn stage_errors_name_the_stage() {
        let pipeline = Pipeline {
            stages: vec![
                Stage::new("$match"),
                Stage {
                    operator: "$group".into(),
                    body: "{_id: }".into(),
                    enabled: true,
                },
            ],
        };
        let err = pipeline.documents(None).unwrap_err().to_string();
        assert!(err.starts_with("stage 2 ($group)"), "{err}");
        assert!(
            Stage {
                operator: "match".into(),
                body: "{}".into(),
                enabled: true
            }
            .to_document()
            .is_err()
        );
    }

    #[test]
    fn templates_parse_and_completion_works() {
        for operator in STAGE_OPERATORS {
            Stage::new(operator.name)
                .to_document()
                .unwrap_or_else(|e| panic!("{}: {e}", operator.name));
        }
        assert_eq!(complete_operator("$gro"), ["$group"]);
        assert!(complete_operator("$su").contains(&"$sum"));
        assert_eq!(complete_operator("$out"), ["$out"]);
    }

    #[test]
    fn input_sample_keeps_source_stage_first() {
        let options = RunOptions {
            input_sample: 100,
            ..RunOptions::default()
        };
        let stages = sampled(
            vec![doc! {"$documents": [{"x": 1}]}, doc! {"$match": {}}],
            options,
        );
        assert_eq!(stages[1], doc! {"$limit": 100_i64});
        let stages = sampled(vec![doc! {"$match": {}}], options);
        assert_eq!(stages[0], doc! {"$limit": 100_i64});
    }
}

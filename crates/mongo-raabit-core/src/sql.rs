//! SQL Query: translate a SQL `SELECT` into a MongoDB find or aggregation.
//!
//! Supported: `SELECT [DISTINCT] … FROM coll [alias] [[LEFT] JOIN other [alias] ON a = b]…
//! [WHERE …] [GROUP BY …] [HAVING …] [ORDER BY …] [LIMIT n] [OFFSET n]` with comparison,
//! `AND`/`OR`/`NOT`, `IN`, `BETWEEN`, `LIKE`/`ILIKE`, `IS [NOT] NULL`, arithmetic, common
//! scalar functions and the `COUNT`/`SUM`/`AVG`/`MIN`/`MAX` aggregates. Simple queries
//! become `find`; anything with joins, grouping or computed columns becomes a pipeline.
//! Parsing uses the `sqlparser` crate.

use crate::error::{RaabitError, Result};
use crate::query::Query;
use crate::shell::{self, ShellStyle};
use mongodb::bson::{Bson, DateTime, Document, Regex, doc, oid::ObjectId};
use sqlparser::ast::{
    BinaryOperator, Distinct, DuplicateTreatment, Expr, Function, FunctionArg, FunctionArgExpr,
    FunctionArguments, GroupByExpr, Ident, JoinConstraint, JoinOperator, LimitClause, ObjectName,
    ObjectNamePart, OrderByKind, OrderBySort, Select, SelectItem, SelectItemQualifiedWildcardKind,
    SetExpr, Statement, TableFactor, UnaryOperator, Value,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

/// The MongoDB equivalent of a SQL query.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlTranslation {
    Find {
        collection: String,
        query: Box<Query>,
    },
    Aggregate {
        collection: String,
        pipeline: Vec<Document>,
    },
}

impl SqlTranslation {
    pub fn collection(&self) -> &str {
        match self {
            Self::Find { collection, .. } | Self::Aggregate { collection, .. } => collection,
        }
    }

    /// Equivalent pipeline (a find becomes `$match`/`$sort`/`$skip`/`$limit`/`$project`).
    pub fn pipeline(&self) -> Vec<Document> {
        match self {
            Self::Aggregate { pipeline, .. } => pipeline.clone(),
            Self::Find { query, .. } => {
                let mut stages = Vec::new();
                if !query.filter.is_empty() {
                    stages.push(doc! {"$match": query.filter.clone()});
                }
                if let Some(sort) = &query.sort {
                    stages.push(doc! {"$sort": sort.clone()});
                }
                if query.skip > 0 {
                    stages.push(doc! {"$skip": query.skip as i64});
                }
                if query.limit > 0 {
                    stages.push(doc! {"$limit": query.limit as i64});
                }
                if let Some(projection) = &query.projection {
                    stages.push(doc! {"$project": projection.clone()});
                }
                stages
            }
        }
    }

    /// The generated MongoDB query in shell syntax.
    pub fn to_shell(&self) -> String {
        match self {
            Self::Find { collection, query } => query.to_shell(collection),
            Self::Aggregate {
                collection,
                pipeline,
            } => format!(
                "db.getCollection(\"{}\").aggregate({})",
                collection.replace('"', "\\\""),
                shell::to_shell_string(
                    &Bson::Array(pipeline.iter().cloned().map(Bson::Document).collect()),
                    ShellStyle::Pretty(2)
                )
            ),
        }
    }
}

fn err<T>(message: impl Into<String>) -> Result<T> {
    Err(RaabitError::Invalid(format!("SQL: {}", message.into())))
}

/// Translate one SQL `SELECT` statement.
pub fn translate(sql: &str) -> Result<SqlTranslation> {
    let statements = Parser::parse_sql(&GenericDialect {}, sql)
        .map_err(|e| RaabitError::Invalid(format!("SQL: {e}")))?;
    let [statement] = statements.as_slice() else {
        return err("enter exactly one SELECT statement");
    };
    let Statement::Query(query) = statement else {
        return err("only SELECT statements can be translated");
    };
    if query.with.is_some() {
        return err("WITH clauses are not supported");
    }
    let SetExpr::Select(select) = query.body.as_ref() else {
        return err("UNION, INTERSECT and VALUES are not supported");
    };
    let mut t = Translator::from_select(select)?;

    let order: Vec<(Expr, bool)> = match &query.order_by {
        None => Vec::new(),
        Some(order_by) => match &order_by.kind {
            OrderByKind::Expressions(items) => items
                .iter()
                .map(|item| {
                    let descending = matches!(item.options.sort, Some(OrderBySort::Desc));
                    (item.expr.clone(), descending)
                })
                .collect(),
            OrderByKind::All(_) => return err("ORDER BY ALL is not supported"),
        },
    };
    let (limit, offset) = match &query.limit_clause {
        None => (None, None),
        Some(LimitClause::LimitOffset { limit, offset, .. }) => (
            limit.as_ref().map(integer).transpose()?,
            offset.as_ref().map(|o| integer(&o.value)).transpose()?,
        ),
        Some(LimitClause::OffsetCommaLimit { offset, limit }) => {
            (Some(integer(limit)?), Some(integer(offset)?))
        }
    };
    t.build(select, &order, limit, offset)
}

fn integer(expr: &Expr) -> Result<u64> {
    match expr {
        Expr::Value(v) => match &v.value {
            Value::Number(n, _) => n
                .parse::<u64>()
                .or_else(|_| err(format!("'{n}' is not a non-negative integer"))),
            other => err(format!("expected a number, found {other}")),
        },
        other => err(format!("expected a number, found {other}")),
    }
}

fn object_name(name: &ObjectName) -> String {
    name.0
        .iter()
        .map(|part| match part {
            ObjectNamePart::Identifier(ident) => ident.value.clone(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

struct JoinSpec {
    alias: String,
    collection: String,
    local: String,
    foreign: String,
    left: bool,
}

struct Translator {
    collection: String,
    /// Names that refer to the base collection (its name and alias).
    base_names: Vec<String>,
    joins: Vec<JoinSpec>,
    /// Aggregate expressions (by SQL text) → accumulator field name.
    accumulators: Vec<(String, String, Document)>,
}

fn table(factor: &TableFactor) -> Result<(String, Option<String>)> {
    match factor {
        TableFactor::Table { name, alias, .. } => Ok((
            object_name(name),
            alias.as_ref().map(|a| a.name.value.clone()),
        )),
        _ => err("only plain collection names are supported in FROM/JOIN"),
    }
}

impl Translator {
    fn from_select(select: &Select) -> Result<Self> {
        let [from] = select.from.as_slice() else {
            return if select.from.is_empty() {
                err("FROM clause is missing")
            } else {
                err("use JOIN instead of comma-separated tables")
            };
        };
        let (collection, alias) = table(&from.relation)?;
        let mut t = Self {
            base_names: std::iter::once(collection.clone()).chain(alias).collect(),
            collection,
            joins: Vec::new(),
            accumulators: Vec::new(),
        };
        for join in &from.joins {
            let (collection, alias) = table(&join.relation)?;
            let alias = alias.unwrap_or_else(|| collection.clone());
            let (constraint, left) = match &join.join_operator {
                JoinOperator::Join(c) | JoinOperator::Inner(c) => (c, false),
                JoinOperator::Left(c) | JoinOperator::LeftOuter(c) => (c, true),
                _ => return err("only INNER and LEFT joins map to $lookup"),
            };
            let JoinConstraint::On(on) = constraint else {
                return err("joins need an ON a = b condition");
            };
            let Expr::BinaryOp {
                left: a,
                op: BinaryOperator::Eq,
                right: b,
            } = on
            else {
                return err("join conditions must be a single equality (a = b)");
            };
            // Whichever side names the joined alias is the foreign field.
            let (foreign_expr, local_expr) = if t.qualifier(a).as_deref() == Some(alias.as_str()) {
                (a, b)
            } else {
                (b, a)
            };
            let foreign = match t.qualifier(foreign_expr) {
                Some(q) if q == alias => t.raw_column(foreign_expr)?,
                _ => {
                    return err(format!(
                        "the join condition must reference a field of '{alias}'"
                    ));
                }
            };
            let local = t.field(local_expr)?;
            t.joins.push(JoinSpec {
                alias,
                collection,
                local,
                foreign,
                left,
            });
        }
        Ok(t)
    }

    /// The table qualifier of `t.col`, if the first part names a table.
    fn qualifier(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::CompoundIdentifier(parts) if parts.len() >= 2 => Some(parts[0].value.clone()),
            _ => None,
        }
    }

    /// Column path without its table qualifier.
    fn raw_column(&self, expr: &Expr) -> Result<String> {
        match expr {
            Expr::Identifier(ident) => Ok(ident.value.clone()),
            Expr::CompoundIdentifier(parts) => Ok(parts[1..]
                .iter()
                .map(|p| p.value.as_str())
                .collect::<Vec<_>>()
                .join(".")),
            other => err(format!("expected a column, found {other}")),
        }
    }

    /// Document path of a column reference, accounting for aliases and joins.
    fn field(&self, expr: &Expr) -> Result<String> {
        match expr {
            Expr::Identifier(ident) => Ok(ident.value.clone()),
            Expr::CompoundIdentifier(parts) => {
                let first = &parts[0].value;
                let rest = || {
                    parts[1..]
                        .iter()
                        .map(|p| p.value.as_str())
                        .collect::<Vec<_>>()
                        .join(".")
                };
                if self.base_names.contains(first) {
                    Ok(rest())
                } else if self.joins.iter().any(|j| &j.alias == first) {
                    Ok(format!("{first}.{}", rest()))
                } else {
                    // Not a table: a nested document path such as address.city.
                    Ok(join_idents(parts))
                }
            }
            Expr::Nested(inner) => self.field(inner),
            other => err(format!("expected a column, found {other}")),
        }
    }

    fn is_column(expr: &Expr) -> bool {
        matches!(expr, Expr::Identifier(_) | Expr::CompoundIdentifier(_))
    }

    // ----- WHERE: query language ---------------------------------------------------------

    fn filter(&self, expr: &Expr) -> Result<Document> {
        match expr {
            Expr::Nested(inner) => self.filter(inner),
            Expr::BinaryOp { left, op, right } => match op {
                BinaryOperator::And => {
                    let mut parts = Vec::new();
                    flatten(expr, &BinaryOperator::And, &mut parts);
                    let docs = parts
                        .into_iter()
                        .map(|p| self.filter(p))
                        .collect::<Result<Vec<_>>>()?;
                    Ok(merge_and(docs))
                }
                BinaryOperator::Or => {
                    let mut parts = Vec::new();
                    flatten(expr, &BinaryOperator::Or, &mut parts);
                    let docs = parts
                        .into_iter()
                        .map(|p| self.filter(p).map(Bson::Document))
                        .collect::<Result<Vec<_>>>()?;
                    Ok(doc! {"$or": docs})
                }
                _ => self.comparison(left, op, right),
            },
            Expr::UnaryOp {
                op: UnaryOperator::Not,
                expr,
            } => Ok(doc! {"$nor": [self.filter(expr)?]}),
            Expr::IsNull(e) => Ok(doc! {self.field(e)?: Bson::Null}),
            Expr::IsNotNull(e) => Ok(doc! {self.field(e)?: {"$ne": Bson::Null}}),
            Expr::IsTrue(e) => Ok(doc! {self.field(e)?: true}),
            Expr::IsFalse(e) => Ok(doc! {self.field(e)?: false}),
            Expr::InList {
                expr,
                list,
                negated,
            } => {
                let values = list.iter().map(literal).collect::<Result<Vec<_>>>()?;
                let op = if *negated { "$nin" } else { "$in" };
                Ok(doc! {self.field(expr)?: {op: values}})
            }
            Expr::Between {
                expr,
                negated,
                low,
                high,
            } => {
                let field = self.field(expr)?;
                let (low, high) = (literal(low)?, literal(high)?);
                Ok(if *negated {
                    doc! {"$or": [{field.clone(): {"$lt": low}}, {field: {"$gt": high}}]}
                } else {
                    doc! {field: {"$gte": low, "$lte": high}}
                })
            }
            Expr::Like {
                negated,
                expr,
                pattern,
                ..
            } => self.like(expr, pattern, *negated, false),
            Expr::ILike {
                negated,
                expr,
                pattern,
                ..
            } => self.like(expr, pattern, *negated, true),
            Expr::Identifier(_) | Expr::CompoundIdentifier(_) => Ok(doc! {self.field(expr)?: true}),
            other => Ok(doc! {"$expr": self.expression(other)?}),
        }
    }

    fn like(
        &self,
        expr: &Expr,
        pattern: &Expr,
        negated: bool,
        insensitive: bool,
    ) -> Result<Document> {
        let Bson::String(pattern) = literal(pattern)? else {
            return err("LIKE needs a string pattern");
        };
        let regex = Regex {
            pattern: like_to_regex(&pattern),
            options: if insensitive {
                "i".into()
            } else {
                String::new()
            },
        };
        let field = self.field(expr)?;
        Ok(if negated {
            doc! {field: {"$not": Bson::RegularExpression(regex)}}
        } else {
            doc! {field: Bson::RegularExpression(regex)}
        })
    }

    fn comparison(&self, left: &Expr, op: &BinaryOperator, right: &Expr) -> Result<Document> {
        let mongo_op = |op: &BinaryOperator, flipped: bool| -> Option<&'static str> {
            Some(match (op, flipped) {
                (BinaryOperator::Eq, _) => "$eq",
                (BinaryOperator::NotEq, _) => "$ne",
                (BinaryOperator::Gt, false) | (BinaryOperator::Lt, true) => "$gt",
                (BinaryOperator::GtEq, false) | (BinaryOperator::LtEq, true) => "$gte",
                (BinaryOperator::Lt, false) | (BinaryOperator::Gt, true) => "$lt",
                (BinaryOperator::LtEq, false) | (BinaryOperator::GtEq, true) => "$lte",
                _ => return None,
            })
        };
        let column_literal = match (Self::is_column(left), Self::is_column(right)) {
            (true, false) => literal(right).ok().map(|v| (left, v, false)),
            (false, true) => literal(left).ok().map(|v| (right, v, true)),
            _ => None,
        };
        if let Some((column, value, flipped)) = column_literal
            && let Some(op) = mongo_op(op, flipped)
        {
            let field = self.field(column)?;
            return Ok(if op == "$eq" && !matches!(value, Bson::Document(_)) {
                doc! {field: value}
            } else {
                doc! {field: {op: value}}
            });
        }
        // Column-to-column or computed comparisons use $expr.
        Ok(doc! {"$expr": self.expression(&Expr::BinaryOp {
            left: Box::new(left.clone()),
            op: op.clone(),
            right: Box::new(right.clone()),
        })?})
    }

    // ----- aggregation expressions ---------------------------------------------------------

    fn expression(&self, expr: &Expr) -> Result<Bson> {
        match expr {
            Expr::Identifier(_) | Expr::CompoundIdentifier(_) => {
                Ok(Bson::String(format!("${}", self.field(expr)?)))
            }
            Expr::Nested(inner) => self.expression(inner),
            Expr::Value(_) | Expr::TypedString(_) => {
                let value = literal(expr)?;
                Ok(match value {
                    Bson::String(s) if s.starts_with('$') => Bson::Document(doc! {"$literal": s}),
                    other => other,
                })
            }
            Expr::UnaryOp { op, expr: inner } => match op {
                UnaryOperator::Minus => match literal(expr) {
                    Ok(v) => Ok(v),
                    Err(_) => Ok(Bson::Document(
                        doc! {"$multiply": [-1, self.expression(inner)?]},
                    )),
                },
                UnaryOperator::Plus => self.expression(inner),
                UnaryOperator::Not => Ok(Bson::Document(doc! {"$not": [self.expression(inner)?]})),
                other => err(format!("operator {other} is not supported")),
            },
            Expr::BinaryOp { left, op, right } => {
                let name = match op {
                    BinaryOperator::Plus => "$add",
                    BinaryOperator::Minus => "$subtract",
                    BinaryOperator::Multiply => "$multiply",
                    BinaryOperator::Divide => "$divide",
                    BinaryOperator::Modulo => "$mod",
                    BinaryOperator::StringConcat => "$concat",
                    BinaryOperator::Eq => "$eq",
                    BinaryOperator::NotEq => "$ne",
                    BinaryOperator::Gt => "$gt",
                    BinaryOperator::GtEq => "$gte",
                    BinaryOperator::Lt => "$lt",
                    BinaryOperator::LtEq => "$lte",
                    BinaryOperator::And => "$and",
                    BinaryOperator::Or => "$or",
                    other => return err(format!("operator {other} is not supported")),
                };
                Ok(Bson::Document(
                    doc! {name: [self.expression(left)?, self.expression(right)?]},
                ))
            }
            Expr::IsNull(e) => Ok(Bson::Document(
                doc! {"$eq": [{"$ifNull": [self.expression(e)?, Bson::Null]}, Bson::Null]},
            )),
            Expr::IsNotNull(e) => Ok(Bson::Document(
                doc! {"$ne": [{"$ifNull": [self.expression(e)?, Bson::Null]}, Bson::Null]},
            )),
            Expr::Case {
                operand: None,
                conditions,
                else_result,
                ..
            } => {
                let branches = conditions
                    .iter()
                    .map(|w| {
                        Ok(Bson::Document(doc! {
                            "case": self.expression(&w.condition)?,
                            "then": self.expression(&w.result)?,
                        }))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let default = match else_result {
                    Some(e) => self.expression(e)?,
                    None => Bson::Null,
                };
                Ok(Bson::Document(
                    doc! {"$switch": {"branches": branches, "default": default}},
                ))
            }
            Expr::Function(function) => {
                let key = expr.to_string();
                if let Some((_, name, _)) = self.accumulators.iter().find(|(k, _, _)| *k == key) {
                    return Ok(Bson::String(format!("${name}")));
                }
                self.scalar_function(function)
            }
            other => err(format!("expression '{other}' is not supported")),
        }
    }

    fn function_args<'a>(
        &self,
        function: &'a Function,
    ) -> Result<(Vec<&'a FunctionArgExpr>, bool)> {
        match &function.args {
            FunctionArguments::None => Ok((Vec::new(), false)),
            FunctionArguments::List(list) => Ok((
                list.args
                    .iter()
                    .map(|arg| match arg {
                        FunctionArg::Unnamed(a) => Ok(a),
                        _ => err("named function arguments are not supported"),
                    })
                    .collect::<Result<Vec<_>>>()?,
                matches!(list.duplicate_treatment, Some(DuplicateTreatment::Distinct)),
            )),
            FunctionArguments::Subquery(_) => err("subqueries are not supported"),
        }
    }

    fn scalar_function(&self, function: &Function) -> Result<Bson> {
        let name = object_name(&function.name).to_ascii_uppercase();
        let (args, _) = self.function_args(function)?;
        let args = args
            .into_iter()
            .map(|a| match a {
                FunctionArgExpr::Expr(e) => self.expression(e),
                _ => err(format!("{name}(*) is not supported")),
            })
            .collect::<Result<Vec<_>>>()?;
        let single = |op: &str| -> Result<Bson> {
            match args.as_slice() {
                [one] => Ok(Bson::Document(doc! {op: one.clone()})),
                _ => err(format!("{name} takes one argument")),
            }
        };
        match name.as_str() {
            "UPPER" | "UCASE" => single("$toUpper"),
            "LOWER" | "LCASE" => single("$toLower"),
            "LENGTH" | "CHAR_LENGTH" => single("$strLenCP"),
            "ABS" => single("$abs"),
            "CEIL" | "CEILING" => single("$ceil"),
            "FLOOR" => single("$floor"),
            "SQRT" => single("$sqrt"),
            "TRIM" => Ok(Bson::Document(
                doc! {"$trim": {"input": args.first().cloned().unwrap_or(Bson::Null)}},
            )),
            "ROUND" => Ok(Bson::Document(doc! {"$round": args})),
            "CONCAT" => Ok(Bson::Document(doc! {"$concat": args})),
            "COALESCE" | "IFNULL" => Ok(Bson::Document(doc! {"$ifNull": args})),
            "YEAR" => single("$year"),
            "MONTH" => single("$month"),
            "DAY" | "DAYOFMONTH" => single("$dayOfMonth"),
            "SUBSTRING" | "SUBSTR" => match args.as_slice() {
                [s, start, len] => Ok(Bson::Document(doc! {
                    "$substrCP": [s.clone(), {"$subtract": [start.clone(), 1]}, len.clone()]
                })),
                _ => err("SUBSTRING takes (string, start, length)"),
            },
            "COUNT" | "SUM" | "AVG" | "MIN" | "MAX" => {
                err(format!("{name}() is only allowed in SELECT or HAVING"))
            }
            other => err(format!("function {other}() is not supported")),
        }
    }

    /// If `expr` is an aggregate call, register an accumulator and return its field name.
    fn accumulator(&mut self, expr: &Expr, alias: Option<&str>) -> Result<Option<String>> {
        let Expr::Function(function) = expr else {
            return Ok(None);
        };
        let name = object_name(&function.name).to_ascii_uppercase();
        if !matches!(name.as_str(), "COUNT" | "SUM" | "AVG" | "MIN" | "MAX") {
            return Ok(None);
        }
        let key = expr.to_string();
        if let Some((_, field, _)) = self.accumulators.iter().find(|(k, _, _)| *k == key) {
            return Ok(Some(field.clone()));
        }
        let (args, distinct) = self.function_args(function)?;
        let arg = match args.as_slice() {
            [FunctionArgExpr::Wildcard] if name == "COUNT" => None,
            [FunctionArgExpr::Expr(e)] => Some(self.expression(e)?),
            _ => return err(format!("{name} takes one argument")),
        };
        let accumulator = match (name.as_str(), arg, distinct) {
            ("COUNT", None, _) => doc! {"$sum": 1},
            ("COUNT", Some(arg), true) => doc! {"$addToSet": arg},
            ("COUNT", Some(arg), false) => doc! {"$sum": {"$cond": [
                {"$eq": [{"$ifNull": [arg, Bson::Null]}, Bson::Null]}, 0, 1
            ]}},
            (_, _, true) => return err(format!("{name}(DISTINCT …) is not supported")),
            (op, Some(arg), false) => doc! {format!("${}", op.to_ascii_lowercase()): arg},
            (op, None, _) => return err(format!("{op}(*) is not supported")),
        };
        let field = alias.map(str::to_owned).unwrap_or_else(|| {
            let base = key
                .to_ascii_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect::<String>();
            let base = base.trim_matches('_').replace("__", "_");
            if base == "count" {
                "count".into()
            } else {
                base
            }
        });
        let field = if distinct {
            format!("{field}__set")
        } else {
            field
        };
        self.accumulators.push((key, field.clone(), accumulator));
        Ok(Some(field))
    }

    // ----- assembly ------------------------------------------------------------------------

    fn lookup_stages(&self) -> Vec<Document> {
        let mut stages = Vec::new();
        for join in &self.joins {
            stages.push(doc! {"$lookup": {
                "from": &join.collection,
                "localField": &join.local,
                "foreignField": &join.foreign,
                "as": &join.alias,
            }});
            stages.push(doc! {"$unwind": {
                "path": format!("${}", join.alias),
                "preserveNullAndEmptyArrays": join.left,
            }});
        }
        stages
    }

    fn build(
        &mut self,
        select: &Select,
        order: &[(Expr, bool)],
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<SqlTranslation> {
        let filter = select
            .selection
            .as_ref()
            .map(|e| self.filter(e))
            .transpose()?
            .unwrap_or_default();
        let group_keys: Vec<Expr> = match &select.group_by {
            GroupByExpr::Expressions(keys, _) => keys.clone(),
            GroupByExpr::All(_) => return err("GROUP BY ALL is not supported"),
        };
        let distinct = match &select.distinct {
            None | Some(Distinct::All) => false,
            Some(Distinct::Distinct) => true,
            Some(Distinct::On(_)) => return err("DISTINCT ON is not supported"),
        };
        let has_aggregate = select.projection.iter().any(|item| match item {
            SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => {
                contains_aggregate(e)
            }
            _ => false,
        }) || select.having.is_some();

        if group_keys.is_empty() && !has_aggregate && !distinct {
            self.build_plain(select, filter, order, limit, offset)
        } else {
            self.build_grouped(select, filter, group_keys, distinct, order, limit, offset)
        }
    }

    fn build_plain(
        &mut self,
        select: &Select,
        filter: Document,
        order: &[(Expr, bool)],
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<SqlTranslation> {
        let mut projection = Document::new();
        let mut computed = false;
        let mut wildcard = false;
        let mut has_id = false;
        for item in &select.projection {
            match item {
                SelectItem::Wildcard(_) => wildcard = true,
                SelectItem::QualifiedWildcard(
                    SelectItemQualifiedWildcardKind::ObjectName(name),
                    _,
                ) => {
                    let name = object_name(name);
                    if self.base_names.contains(&name) {
                        wildcard = true;
                    } else {
                        projection.insert(name, 1);
                    }
                }
                SelectItem::UnnamedExpr(expr) if Self::is_column(expr) => {
                    let field = self.field(expr)?;
                    has_id |= field == "_id";
                    projection.insert(field, 1);
                }
                SelectItem::UnnamedExpr(expr) => {
                    computed = true;
                    projection.insert(expr.to_string(), self.expression(expr)?);
                }
                SelectItem::ExprWithAlias { expr, alias } => {
                    computed = true;
                    has_id |= alias.value == "_id";
                    projection.insert(alias.value.clone(), self.expression(expr)?);
                }
                other => return err(format!("select item '{other}' is not supported")),
            }
        }
        if wildcard {
            if projection.is_empty() {
                // `SELECT *` (optionally with joined tables) keeps documents whole.
            } else {
                return err("mixing * with other columns is not supported");
            }
        } else if !has_id && !projection.is_empty() {
            projection.insert("_id", 0);
        }
        let sort = self.sort_document(order, &|t, e| t.field(e))?;
        if self.joins.is_empty() && !computed {
            return Ok(SqlTranslation::Find {
                collection: self.collection.clone(),
                query: Box::new(Query {
                    filter,
                    projection: (!projection.is_empty()).then_some(projection),
                    sort,
                    skip: offset.unwrap_or(0),
                    limit: limit.unwrap_or(0),
                }),
            });
        }
        let mut pipeline = self.lookup_stages();
        if !filter.is_empty() {
            pipeline.push(doc! {"$match": filter});
        }
        if let Some(sort) = sort {
            pipeline.push(doc! {"$sort": sort});
        }
        if let Some(offset) = offset.filter(|o| *o > 0) {
            pipeline.push(doc! {"$skip": offset as i64});
        }
        if let Some(limit) = limit {
            pipeline.push(doc! {"$limit": limit as i64});
        }
        if !projection.is_empty() {
            pipeline.push(doc! {"$project": projection});
        }
        Ok(SqlTranslation::Aggregate {
            collection: self.collection.clone(),
            pipeline,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn build_grouped(
        &mut self,
        select: &Select,
        filter: Document,
        mut group_keys: Vec<Expr>,
        distinct: bool,
        order: &[(Expr, bool)],
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<SqlTranslation> {
        // Output column name → value in the final $project.
        let mut outputs: Vec<(String, Bson)> = Vec::new();
        // SQL text of an expression → output column, for ORDER BY.
        let mut names: Vec<(String, String)> = Vec::new();
        if distinct && group_keys.is_empty() {
            for item in &select.projection {
                match item {
                    SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => {
                        group_keys.push(e.clone());
                    }
                    _ => return err("SELECT DISTINCT * is not supported"),
                }
            }
        }
        let key_name = |expr: &Expr| -> String {
            match expr {
                Expr::Identifier(i) => i.value.clone(),
                Expr::CompoundIdentifier(parts) => {
                    parts.last().map(|p| p.value.clone()).unwrap_or_default()
                }
                other => other.to_string().replace(['.', '$', ' '], "_"),
            }
        };
        let mut id = Document::new();
        for key in &group_keys {
            id.insert(key_name(key), self.expression(key)?);
        }
        let single_key = group_keys.len() == 1;
        let key_ref = |expr: &Expr| -> Option<String> {
            let text = expr.to_string();
            group_keys.iter().find(|k| k.to_string() == text).map(|k| {
                if single_key {
                    "$_id".to_owned()
                } else {
                    format!("$_id.{}", key_name(k))
                }
            })
        };
        for item in &select.projection {
            let (expr, alias) = match item {
                SelectItem::UnnamedExpr(e) => (e, None),
                SelectItem::ExprWithAlias { expr, alias } => (expr, Some(alias.value.as_str())),
                other => {
                    return err(format!(
                        "select item '{other}' is not supported with GROUP BY"
                    ));
                }
            };
            if let Some(reference) = key_ref(expr) {
                let name = alias.map(str::to_owned).unwrap_or_else(|| key_name(expr));
                names.push((expr.to_string(), name.clone()));
                outputs.push((name, Bson::String(reference)));
            } else if let Some(field) = self.accumulator(expr, alias)? {
                let name = alias
                    .map(str::to_owned)
                    .unwrap_or_else(|| field.trim_end_matches("__set").to_owned());
                names.push((expr.to_string(), name.clone()));
                let value = if field.ends_with("__set") {
                    Bson::Document(doc! {"$size": format!("${field}")})
                } else {
                    Bson::String(format!("${field}"))
                };
                outputs.push((name, value));
            } else if contains_aggregate(expr) {
                // e.g. SUM(a) / COUNT(*): register inner aggregates, compute after grouping.
                self.register_nested(expr)?;
                let name = alias.map(str::to_owned).unwrap_or_else(|| expr.to_string());
                names.push((expr.to_string(), name.clone()));
                outputs.push((name, self.expression(expr)?));
            } else {
                return err(format!(
                    "'{expr}' must appear in GROUP BY or be used in an aggregate function"
                ));
            }
        }
        let having = match &select.having {
            Some(having) => {
                self.register_nested(having)?;
                Some(self.post_group_expression(having, &key_ref)?)
            }
            None => None,
        };
        let mut group = doc! {"_id": if group_keys.is_empty() {
            Bson::Null
        } else if single_key {
            id.into_iter().next().map(|(_, v)| v).unwrap_or(Bson::Null)
        } else {
            Bson::Document(id)
        }};
        for (_, field, accumulator) in &self.accumulators {
            group.insert(field.clone(), accumulator.clone());
        }
        let mut pipeline = self.lookup_stages();
        if !filter.is_empty() {
            pipeline.push(doc! {"$match": filter});
        }
        pipeline.push(doc! {"$group": group});
        if let Some(having) = having {
            pipeline.push(doc! {"$match": {"$expr": having}});
        }
        let mut project = doc! {"_id": 0};
        for (name, value) in outputs {
            project.insert(name, value);
        }
        pipeline.push(doc! {"$project": project});
        let sort = self.sort_document(order, &|t, e| {
            let text = e.to_string();
            names
                .iter()
                .find(|(expr, name)| *expr == text || *name == text)
                .map(|(_, name)| name.clone())
                .map(Ok)
                .unwrap_or_else(|| {
                    t.field(e)
                        .or_else(|_| err(format!("ORDER BY '{e}' must be a selected column")))
                })
        })?;
        if let Some(sort) = sort {
            pipeline.push(doc! {"$sort": sort});
        }
        if let Some(offset) = offset.filter(|o| *o > 0) {
            pipeline.push(doc! {"$skip": offset as i64});
        }
        if let Some(limit) = limit {
            pipeline.push(doc! {"$limit": limit as i64});
        }
        Ok(SqlTranslation::Aggregate {
            collection: self.collection.clone(),
            pipeline,
        })
    }

    fn register_nested(&mut self, expr: &Expr) -> Result<()> {
        match expr {
            Expr::Function(_) if contains_aggregate_shallow(expr) => {
                self.accumulator(expr, None)?;
            }
            Expr::BinaryOp { left, right, .. } => {
                self.register_nested(left)?;
                self.register_nested(right)?;
            }
            Expr::Nested(inner) | Expr::UnaryOp { expr: inner, .. } => {
                self.register_nested(inner)?
            }
            _ => {}
        }
        Ok(())
    }

    /// Expression evaluated after `$group`: aggregates refer to accumulators, group keys
    /// to `_id`.
    fn post_group_expression(
        &self,
        expr: &Expr,
        key_ref: &dyn Fn(&Expr) -> Option<String>,
    ) -> Result<Bson> {
        if let Some(reference) = key_ref(expr) {
            return Ok(Bson::String(reference));
        }
        match expr {
            Expr::BinaryOp { left, op, right } => {
                let mut inner = self.expression(&Expr::BinaryOp {
                    left: left.clone(),
                    op: op.clone(),
                    right: right.clone(),
                })?;
                // Re-map operands that are group keys.
                if let Bson::Document(doc) = &mut inner
                    && let Some((_, Bson::Array(items))) = doc.iter_mut().next()
                {
                    for (slot, side) in items.iter_mut().zip([left, right]) {
                        if Self::is_column(side) {
                            if let Some(reference) = key_ref(side) {
                                *slot = Bson::String(reference);
                            }
                        } else if matches!(side.as_ref(), Expr::BinaryOp { .. } | Expr::Nested(_)) {
                            *slot = self.post_group_expression(side, key_ref)?;
                        }
                    }
                }
                Ok(inner)
            }
            Expr::Nested(inner) => self.post_group_expression(inner, key_ref),
            other => self.expression(other),
        }
    }

    fn sort_document(
        &self,
        order: &[(Expr, bool)],
        resolve: &dyn Fn(&Self, &Expr) -> Result<String>,
    ) -> Result<Option<Document>> {
        if order.is_empty() {
            return Ok(None);
        }
        let mut sort = Document::new();
        for (expr, descending) in order {
            sort.insert(resolve(self, expr)?, if *descending { -1 } else { 1 });
        }
        Ok(Some(sort))
    }
}

fn join_idents(parts: &[Ident]) -> String {
    parts
        .iter()
        .map(|p| p.value.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn flatten<'a>(expr: &'a Expr, op: &BinaryOperator, out: &mut Vec<&'a Expr>) {
    match expr {
        Expr::BinaryOp {
            left,
            op: inner,
            right,
        } if inner == op => {
            flatten(left, op, out);
            flatten(right, op, out);
        }
        Expr::Nested(inner) if matches!(inner.as_ref(), Expr::BinaryOp { op: o, .. } if o == op) => {
            flatten(inner, op, out)
        }
        other => out.push(other),
    }
}

/// Merge `AND`ed conditions into one document when their keys do not collide.
fn merge_and(docs: Vec<Document>) -> Document {
    let mut merged = Document::new();
    let mut conflict = false;
    for d in &docs {
        for (key, value) in d {
            match (merged.get_mut(key), value) {
                (None, _) => {
                    merged.insert(key.clone(), value.clone());
                }
                (Some(Bson::Document(existing)), Bson::Document(new))
                    if existing.keys().all(|k| k.starts_with('$'))
                        && new
                            .keys()
                            .all(|k| k.starts_with('$') && !existing.contains_key(k)) =>
                {
                    existing.extend(new.clone());
                }
                _ => conflict = true,
            }
        }
    }
    if conflict {
        doc! {"$and": docs}
    } else {
        merged
    }
}

fn contains_aggregate_shallow(expr: &Expr) -> bool {
    matches!(expr, Expr::Function(f) if matches!(
        object_name(&f.name).to_ascii_uppercase().as_str(),
        "COUNT" | "SUM" | "AVG" | "MIN" | "MAX"
    ))
}

fn contains_aggregate(expr: &Expr) -> bool {
    match expr {
        Expr::Function(_) if contains_aggregate_shallow(expr) => true,
        Expr::BinaryOp { left, right, .. } => contains_aggregate(left) || contains_aggregate(right),
        Expr::Nested(inner) | Expr::UnaryOp { expr: inner, .. } => contains_aggregate(inner),
        _ => false,
    }
}

/// Convert a SQL `LIKE` pattern to an anchored regular expression.
pub fn like_to_regex(pattern: &str) -> String {
    let mut out = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '%' => out.push_str(".*"),
            '_' => out.push('.'),
            '\\' => {
                if let Some(next) = chars.next() {
                    push_escaped(&mut out, next);
                }
            }
            other => push_escaped(&mut out, other),
        }
    }
    out.push('$');
    // `^.*` and `.*$` add nothing.
    let trimmed = out.strip_prefix("^.*").unwrap_or(&out);
    let trimmed = trimmed.strip_suffix(".*$").unwrap_or(trimmed);
    trimmed.to_owned()
}

fn push_escaped(out: &mut String, c: char) {
    if "\\^$.|?*+()[]{}/".contains(c) {
        out.push('\\');
    }
    out.push(c);
}

/// A SQL literal as BSON.
fn literal(expr: &Expr) -> Result<Bson> {
    match expr {
        Expr::Nested(inner) => literal(inner),
        Expr::Value(v) => match &v.value {
            Value::Number(n, _) => {
                if let Ok(i) = n.parse::<i64>() {
                    Ok(i32::try_from(i).map(Bson::Int32).unwrap_or(Bson::Int64(i)))
                } else {
                    n.parse::<f64>()
                        .map(Bson::Double)
                        .or_else(|_| err(format!("invalid number '{n}'")))
                }
            }
            Value::SingleQuotedString(s) | Value::DoubleQuotedString(s) => {
                Ok(Bson::String(s.clone()))
            }
            Value::Boolean(b) => Ok(Bson::Boolean(*b)),
            Value::Null => Ok(Bson::Null),
            other => err(format!("literal {other} is not supported")),
        },
        Expr::UnaryOp {
            op: UnaryOperator::Minus,
            expr,
        } => match literal(expr)? {
            Bson::Int32(v) => Ok(Bson::Int32(-v)),
            Bson::Int64(v) => Ok(Bson::Int64(-v)),
            Bson::Double(v) => Ok(Bson::Double(-v)),
            _ => err("unary minus needs a number"),
        },
        Expr::TypedString(typed) => {
            let text = match &typed.value.value {
                Value::SingleQuotedString(s) | Value::DoubleQuotedString(s) => s.clone(),
                other => other.to_string(),
            };
            shell::parse_date(&text)
                .map(Bson::DateTime)
                .map_err(|e| RaabitError::Invalid(format!("SQL: {e}")))
        }
        Expr::Function(function) => {
            let name = object_name(&function.name).to_ascii_uppercase();
            let arg = match &function.args {
                FunctionArguments::List(list) => match list.args.as_slice() {
                    [FunctionArg::Unnamed(FunctionArgExpr::Expr(e))] => Some(literal(e)?),
                    [] => None,
                    _ => return err(format!("{name}() takes one argument")),
                },
                _ => None,
            };
            match (name.as_str(), arg) {
                ("OBJECTID", Some(Bson::String(hex))) => ObjectId::parse_str(&hex)
                    .map(Bson::ObjectId)
                    .or_else(|_| err(format!("invalid ObjectId '{hex}'"))),
                ("ISODATE" | "DATE" | "TIMESTAMP", Some(Bson::String(text))) => {
                    shell::parse_date(&text)
                        .map(Bson::DateTime)
                        .map_err(|e| RaabitError::Invalid(format!("SQL: {e}")))
                }
                ("NOW" | "CURRENT_TIMESTAMP", None) => Ok(Bson::DateTime(DateTime::now())),
                _ => err(format!("{name}() is not a literal")),
            }
        }
        other => err(format!("expected a literal value, found '{other}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(sql: &str) -> Query {
        match translate(sql).unwrap() {
            SqlTranslation::Find { query, .. } => *query,
            other => panic!("expected find, got {other:?}"),
        }
    }

    fn pipeline(sql: &str) -> Vec<Document> {
        match translate(sql).unwrap() {
            SqlTranslation::Aggregate { pipeline, .. } => pipeline,
            other => panic!("expected aggregate, got {other:?}"),
        }
    }

    #[test]
    fn simple_select_becomes_find() {
        let q = find(
            "SELECT name, address.city FROM customers WHERE age >= 18 AND age < 65 \
             AND tier IN ('pro', 'enterprise') AND name LIKE 'Ad%' ORDER BY name DESC LIMIT 10 OFFSET 20",
        );
        assert_eq!(
            q.filter,
            doc! {
                "age": {"$gte": 18, "$lt": 65},
                "tier": {"$in": ["pro", "enterprise"]},
                "name": Bson::RegularExpression(Regex { pattern: "^Ad".into(), options: String::new() }),
            }
        );
        assert_eq!(
            q.projection,
            Some(doc! {"name": 1, "address.city": 1, "_id": 0})
        );
        assert_eq!(q.sort, Some(doc! {"name": -1}));
        assert_eq!((q.skip, q.limit), (20, 10));
        let all = find("SELECT * FROM c WHERE NOT (a = 1 OR b IS NULL) AND c BETWEEN 1 AND 5");
        assert_eq!(
            all.filter,
            doc! {"$nor": [{"$or": [{"a": 1}, {"b": Bson::Null}]}], "c": {"$gte": 1, "$lte": 5}}
        );
        assert_eq!(all.projection, None);
        let flipped = find("SELECT * FROM c WHERE 5 < x AND x <> y");
        assert_eq!(
            flipped.filter,
            doc! {"x": {"$gt": 5}, "$expr": {"$ne": ["$x", "$y"]}}
        );
    }

    #[test]
    fn group_by_with_having_and_order() {
        let p = pipeline(
            "SELECT status, COUNT(*) AS n, SUM(total) AS revenue, AVG(total) FROM orders \
             WHERE total > 0 GROUP BY status HAVING COUNT(*) > 10 ORDER BY revenue DESC LIMIT 3",
        );
        assert_eq!(
            p,
            vec![
                doc! {"$match": {"total": {"$gt": 0}}},
                doc! {"$group": {"_id": "$status", "n": {"$sum": 1}, "revenue": {"$sum": "$total"}, "avg_total": {"$avg": "$total"}}},
                doc! {"$match": {"$expr": {"$gt": ["$n", 10]}}},
                doc! {"$project": {"_id": 0, "status": "$_id", "n": "$n", "revenue": "$revenue", "avg_total": "$avg_total"}},
                doc! {"$sort": {"revenue": -1}},
                doc! {"$limit": 3_i64},
            ]
        );
        let multi = pipeline("SELECT a, b, COUNT(DISTINCT c) AS k FROM t GROUP BY a, b");
        assert_eq!(
            multi[0],
            doc! {"$group": {"_id": {"a": "$a", "b": "$b"}, "k__set": {"$addToSet": "$c"}}}
        );
        assert_eq!(
            multi[1],
            doc! {"$project": {"_id": 0, "a": "$_id.a", "b": "$_id.b", "k": {"$size": "$k__set"}}}
        );
        assert!(translate("SELECT a, COUNT(*) FROM t GROUP BY b").is_err());
        let total = pipeline("SELECT COUNT(*) FROM t");
        assert_eq!(
            total[0],
            doc! {"$group": {"_id": Bson::Null, "count": {"$sum": 1}}}
        );
    }

    #[test]
    fn joins_become_lookup() {
        let p = pipeline(
            "SELECT o.total, c.name AS customer FROM orders o LEFT JOIN customers c ON o.customerId = c._id \
             WHERE c.tier = 'pro' ORDER BY o.total DESC LIMIT 5",
        );
        assert_eq!(
            p,
            vec![
                doc! {"$lookup": {"from": "customers", "localField": "customerId", "foreignField": "_id", "as": "c"}},
                doc! {"$unwind": {"path": "$c", "preserveNullAndEmptyArrays": true}},
                doc! {"$match": {"c.tier": "pro"}},
                doc! {"$sort": {"total": -1}},
                doc! {"$limit": 5_i64},
                doc! {"$project": {"total": 1, "customer": "$c.name", "_id": 0}},
            ]
        );
        assert!(translate("SELECT * FROM a JOIN b ON a.x > b.y").is_err());
        assert!(translate("SELECT * FROM a, b").is_err());
    }

    #[test]
    fn computed_columns_and_literals() {
        let p = pipeline(
            "SELECT UPPER(name) AS n, price * qty AS line FROM items WHERE created > DATE '2024-01-01'",
        );
        assert_eq!(
            p[1],
            doc! {"$project": {"n": {"$toUpper": "$name"}, "line": {"$multiply": ["$price", "$qty"]}, "_id": 0}}
        );
        assert!(matches!(
            p[0].get_document("$match")
                .unwrap()
                .get_document("created")
                .unwrap()
                .get("$gt"),
            Some(Bson::DateTime(_))
        ));
        let q =
            find("SELECT * FROM t WHERE _id = ObjectId('5f1d7f1e2b3c4d5e6f708192') AND n = -2.5");
        assert!(matches!(q.filter.get("_id"), Some(Bson::ObjectId(_))));
        assert_eq!(q.filter.get_f64("n").unwrap(), -2.5);
        let d = pipeline("SELECT DISTINCT city FROM c");
        assert_eq!(d[0], doc! {"$group": {"_id": "$city"}});
    }

    #[test]
    fn like_patterns() {
        assert_eq!(like_to_regex("abc%"), "^abc");
        assert_eq!(like_to_regex("%a_c%"), "a.c");
        assert_eq!(like_to_regex("a.b"), "^a\\.b$");
        assert_eq!(like_to_regex("100\\%"), "^100%$");
    }

    #[test]
    fn rejects_unsupported_sql() {
        for sql in [
            "DELETE FROM t",
            "SELECT 1; SELECT 2",
            "SELECT * FROM t UNION SELECT * FROM u",
            "SELEC",
            "SELECT FOO(a) FROM t",
        ] {
            let e = translate(sql).unwrap_err().to_string();
            assert!(e.starts_with("SQL:"), "{sql}: {e}");
        }
    }

    #[test]
    fn shell_output() {
        let t = translate("SELECT * FROM orders WHERE total > 5 LIMIT 2").unwrap();
        assert_eq!(
            t.to_shell(),
            "db.getCollection(\"orders\").find({ total: { $gt: 5 } }).limit(2)"
        );
        assert_eq!(
            t.pipeline(),
            vec![
                doc! {"$match": {"total": {"$gt": 5}}},
                doc! {"$limit": 2_i64}
            ]
        );
    }
}

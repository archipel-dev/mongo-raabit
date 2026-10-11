//! Live connections and server metadata (databases, collections, indexes, users, roles,
//! server status and current operations).

use crate::error::{RaabitError, Result};
use crate::profile::{ConnectionProfile, Target};
use crate::secrets::SecretStore;
use crate::tunnel::SshTunnel;
use futures_util::TryStreamExt;
use mongodb::bson::{Bson, Document, doc};
use mongodb::options::{ClientOptions, IndexOptions};
use mongodb::{Client, IndexModel};
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Facts about the server shown after "Test connection" and in the status bar.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ServerInfo {
    pub version: String,
    /// `Standalone`, `Replica set <name>` or `Sharded cluster (mongos)`.
    pub topology: String,
    pub is_writable_primary: bool,
    pub max_wire_version: i64,
    pub round_trip: Duration,
}

/// An open connection. Cloning is cheap and shares the driver pool and SSH tunnel.
#[derive(Clone)]
pub struct Connection {
    client: Client,
    /// Driver URI including credentials and tunnel endpoint (never displayed).
    uri: Arc<str>,
    profile: Arc<ConnectionProfile>,
    server: ServerInfo,
    _tunnel: Option<Arc<SshTunnel>>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("profile", &self.profile.name)
            .field("server", &self.server)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DatabaseInfo {
    pub name: String,
    pub size_on_disk: u64,
    pub empty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CollectionKind {
    Collection,
    View,
    TimeSeries,
    /// A `<bucket>.files` / `<bucket>.chunks` pair, listed once under the bucket name.
    GridFsBucket,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CollectionInfo {
    pub name: String,
    pub kind: CollectionKind,
    /// Source collection of a view.
    pub view_on: Option<String>,
    pub pipeline: Vec<Document>,
    pub options: Document,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CollectionStats {
    pub count: u64,
    pub size: u64,
    pub avg_obj_size: u64,
    pub storage_size: u64,
    pub total_index_size: u64,
    pub index_count: u64,
    pub capped: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IndexInfo {
    pub name: String,
    pub keys: Document,
    pub unique: bool,
    pub sparse: bool,
    pub hidden: bool,
    pub ttl_seconds: Option<i64>,
    pub partial_filter: Option<Document>,
    pub size: Option<u64>,
    /// Operations since the server started tracking (`$indexStats`).
    pub ops: Option<u64>,
    pub raw: Document,
}

/// Options for creating an index from the Index Manager.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewIndex {
    pub keys: Document,
    pub name: Option<String>,
    pub unique: bool,
    pub sparse: bool,
    pub hidden: bool,
    pub ttl_seconds: Option<u64>,
    pub partial_filter: Option<Document>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RoleRef {
    pub role: String,
    pub db: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UserInfo {
    pub user: String,
    pub db: String,
    pub roles: Vec<RoleRef>,
    pub mechanisms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RoleInfo {
    pub role: String,
    pub db: String,
    pub is_builtin: bool,
    pub inherited: Vec<RoleRef>,
    pub privileges: Vec<Document>,
}

/// One entry of `$currentOp`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CurrentOp {
    pub opid: Bson,
    pub op: String,
    pub namespace: String,
    pub client: String,
    pub secs_running: i64,
    pub description: String,
    pub raw: Document,
}

fn as_u64(value: Option<&Bson>) -> u64 {
    match value {
        Some(Bson::Int32(v)) => u64::try_from(*v).unwrap_or(0),
        Some(Bson::Int64(v)) => u64::try_from(*v).unwrap_or(0),
        Some(Bson::Double(v)) if *v >= 0.0 => *v as u64,
        _ => 0,
    }
}

fn as_i64(value: Option<&Bson>) -> Option<i64> {
    match value {
        Some(Bson::Int32(v)) => Some(i64::from(*v)),
        Some(Bson::Int64(v)) => Some(*v),
        Some(Bson::Double(v)) => Some(*v as i64),
        _ => None,
    }
}

fn role_refs(value: Option<&Bson>) -> Vec<RoleRef> {
    let Some(Bson::Array(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Bson::Document(d) => Some(RoleRef {
                role: d.get_str("role").unwrap_or_default().to_owned(),
                db: d.get_str("db").unwrap_or_default().to_owned(),
            }),
            _ => None,
        })
        .collect()
}

fn validate_name(kind: &str, name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(RaabitError::Invalid(format!("{kind} name is empty")));
    }
    if name.contains('\0') {
        return Err(RaabitError::Invalid(format!(
            "{kind} name contains a NUL character"
        )));
    }
    if kind == "database" && name.contains(['/', '\\', '.', ' ', '"', '$']) {
        return Err(RaabitError::Invalid(format!(
            "database name '{name}' contains a character MongoDB does not allow (/\\. \"$)"
        )));
    }
    if kind == "collection" && (name.starts_with("system.") || name.contains('$')) {
        return Err(RaabitError::Invalid(format!(
            "collection name '{name}' is reserved or contains '$'"
        )));
    }
    Ok(())
}

impl Connection {
    /// Open a connection, starting the SSH tunnel first when the profile has one.
    pub async fn connect(profile: &ConnectionProfile, secrets: &dyn SecretStore) -> Result<Self> {
        profile.validate()?;
        let password = if profile.auth.mechanism.needs_password() {
            secrets.get(&profile.password_key())?
        } else {
            None
        };
        let timeout = profile
            .connect_timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_TIMEOUT);
        let tunnel = match &profile.ssh {
            Some(ssh) => {
                let remote = match &profile.target {
                    Target::Standalone { server } => server.clone(),
                    Target::ReplicaSet { members: hosts, .. }
                    | Target::Sharded { routers: hosts } => hosts
                        .first()
                        .cloned()
                        .ok_or_else(|| RaabitError::InvalidProfile("no hosts configured".into()))?,
                    Target::Srv { .. } => {
                        return Err(RaabitError::InvalidProfile(
                            "SSH tunnels need explicit hosts".into(),
                        ));
                    }
                };
                let ssh_password = secrets.get(&profile.ssh_password_key())?;
                let ssh = ssh.clone();
                let tunnel = tokio::task::spawn_blocking(move || {
                    SshTunnel::open(&ssh, &remote, ssh_password.as_deref(), timeout)
                })
                .await
                .map_err(|e| RaabitError::Tunnel(e.to_string()))??;
                Some(Arc::new(tunnel))
            }
            None => None,
        };
        let uri =
            profile.connection_string(password.as_deref(), tunnel.as_ref().map(|t| t.local()))?;
        let mut options = ClientOptions::parse(&uri).await?;
        let full_uri: Arc<str> = Arc::from(uri.as_str());
        if options.server_selection_timeout.is_none() {
            options.server_selection_timeout = Some(timeout);
        }
        if options.connect_timeout.is_none() {
            options.connect_timeout = Some(timeout);
        }
        if options.app_name.is_none() {
            options.app_name = Some("mongo-raabit".into());
        }
        let client = Client::with_options(options)?;
        let server = Self::probe(&client).await?;
        Ok(Self {
            client,
            uri: full_uri,
            profile: Arc::new(profile.clone()),
            server,
            _tunnel: tunnel,
        })
    }

    /// Connect, collect [`ServerInfo`] and disconnect again.
    pub async fn test(
        profile: &ConnectionProfile,
        secrets: &dyn SecretStore,
    ) -> Result<ServerInfo> {
        let connection = Self::connect(profile, secrets).await?;
        let info = connection.server.clone();
        connection.client.shutdown().await;
        Ok(info)
    }

    async fn probe(client: &Client) -> Result<ServerInfo> {
        let admin = client.database("admin");
        let start = Instant::now();
        let hello = admin.run_command(doc! {"hello": 1}).await?;
        let round_trip = start.elapsed();
        let build = admin.run_command(doc! {"buildInfo": 1}).await?;
        let topology = if hello.get_str("msg") == Ok("isdbgrid") {
            "Sharded cluster (mongos)".to_owned()
        } else if let Ok(set) = hello.get_str("setName") {
            format!("Replica set {set}")
        } else {
            "Standalone".to_owned()
        };
        Ok(ServerInfo {
            version: build.get_str("version").unwrap_or("unknown").to_owned(),
            topology,
            is_writable_primary: hello.get_bool("isWritablePrimary").unwrap_or(false),
            max_wire_version: as_i64(hello.get("maxWireVersion")).unwrap_or(0),
            round_trip,
        })
    }

    /// The connection string the driver uses, **including the password** and the SSH
    /// tunnel endpoint. Only hand it to child processes through the environment.
    pub fn connection_uri(&self) -> &str {
        &self.uri
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn profile(&self) -> &ConnectionProfile {
        &self.profile
    }

    pub fn server(&self) -> &ServerInfo {
        &self.server
    }

    pub fn collection(&self, db: &str, name: &str) -> mongodb::Collection<Document> {
        self.client.database(db).collection(name)
    }

    /// Run an arbitrary database command.
    pub async fn run_command(&self, db: &str, command: Document) -> Result<Document> {
        Ok(self.client.database(db).run_command(command).await?)
    }

    // ----- databases -------------------------------------------------------------------

    pub async fn databases(&self) -> Result<Vec<DatabaseInfo>> {
        let specs = self.client.list_databases().await?;
        let mut list: Vec<DatabaseInfo> = specs
            .into_iter()
            .map(|spec| DatabaseInfo {
                name: spec.name,
                size_on_disk: spec.size_on_disk,
                empty: spec.empty,
            })
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    /// MongoDB creates databases lazily; this creates the first collection.
    pub async fn create_database(&self, db: &str, first_collection: &str) -> Result<()> {
        validate_name("database", db)?;
        self.create_collection(db, first_collection, Document::new())
            .await
    }

    pub async fn drop_database(&self, db: &str) -> Result<()> {
        Ok(self.client.database(db).drop().await?)
    }

    // ----- collections -----------------------------------------------------------------

    pub async fn collections(&self, db: &str) -> Result<Vec<CollectionInfo>> {
        let specs: Vec<Document> = self
            .client
            .database(db)
            .run_cursor_command(
                doc! {"listCollections": 1, "nameOnly": false, "authorizedCollections": true},
            )
            .await?
            .try_collect()
            .await?;
        let names: Vec<String> = specs
            .iter()
            .filter_map(|s| s.get_str("name").ok().map(str::to_owned))
            .collect();
        let mut list = Vec::new();
        for spec in specs {
            let name = spec.get_str("name").unwrap_or_default().to_owned();
            let options = spec.get_document("options").cloned().unwrap_or_default();
            let kind = match spec.get_str("type").unwrap_or("collection") {
                "view" => CollectionKind::View,
                "timeseries" => CollectionKind::TimeSeries,
                _ if name.starts_with("system.") => CollectionKind::System,
                _ => CollectionKind::Collection,
            };
            if let Some(bucket) = name.strip_suffix(".files")
                && names.iter().any(|n| *n == format!("{bucket}.chunks"))
            {
                list.push(CollectionInfo {
                    name: bucket.to_owned(),
                    kind: CollectionKind::GridFsBucket,
                    view_on: None,
                    pipeline: Vec::new(),
                    options: Document::new(),
                });
            }
            let pipeline = options
                .get_array("pipeline")
                .map(|stages| {
                    stages
                        .iter()
                        .filter_map(|s| s.as_document().cloned())
                        .collect()
                })
                .unwrap_or_default();
            list.push(CollectionInfo {
                view_on: options.get_str("viewOn").ok().map(str::to_owned),
                name,
                kind,
                pipeline,
                options,
            });
        }
        list.sort_by(|a, b| {
            (a.kind == CollectionKind::System, a.name.to_lowercase())
                .cmp(&(b.kind == CollectionKind::System, b.name.to_lowercase()))
        });
        Ok(list)
    }

    /// Create a collection; `options` is the `create` command body (capped, validator…).
    pub async fn create_collection(&self, db: &str, name: &str, options: Document) -> Result<()> {
        validate_name("database", db)?;
        validate_name("collection", name)?;
        let mut command = doc! {"create": name};
        command.extend(options);
        self.client.database(db).run_command(command).await?;
        Ok(())
    }

    pub async fn create_view(
        &self,
        db: &str,
        name: &str,
        view_on: &str,
        pipeline: Vec<Document>,
    ) -> Result<()> {
        self.create_collection(db, name, doc! {"viewOn": view_on, "pipeline": pipeline})
            .await
    }

    pub async fn drop_collection(&self, db: &str, name: &str) -> Result<()> {
        Ok(self.collection(db, name).drop().await?)
    }

    pub async fn rename_collection(
        &self,
        db: &str,
        from: &str,
        to_db: &str,
        to: &str,
        drop_target: bool,
    ) -> Result<()> {
        validate_name("collection", to)?;
        self.client
            .database("admin")
            .run_command(doc! {
                "renameCollection": format!("{db}.{from}"),
                "to": format!("{to_db}.{to}"),
                "dropTarget": drop_target,
            })
            .await?;
        Ok(())
    }

    pub async fn collection_stats(&self, db: &str, name: &str) -> Result<CollectionStats> {
        let cursor = self
            .collection(db, name)
            .aggregate(vec![doc! {"$collStats": {"storageStats": {}, "count": {}}}])
            .await?;
        let rows: Vec<Document> = cursor.try_collect().await?;
        let mut stats = CollectionStats::default();
        // Sharded collections report one row per shard; sum them.
        for row in rows {
            let storage = row
                .get_document("storageStats")
                .cloned()
                .unwrap_or_default();
            stats.count += as_u64(storage.get("count").or_else(|| row.get("count")));
            stats.size += as_u64(storage.get("size"));
            stats.storage_size += as_u64(storage.get("storageSize"));
            stats.total_index_size += as_u64(storage.get("totalIndexSize"));
            stats.index_count = stats.index_count.max(as_u64(storage.get("nindexes")));
            stats.capped |= storage.get_bool("capped").unwrap_or(false);
        }
        stats.avg_obj_size = stats.size.checked_div(stats.count).unwrap_or(0);
        Ok(stats)
    }

    // ----- indexes ---------------------------------------------------------------------

    pub async fn indexes(&self, db: &str, name: &str) -> Result<Vec<IndexInfo>> {
        let collection = self.collection(db, name);
        let specs: Vec<Document> = self
            .client
            .database(db)
            .run_cursor_command(doc! {"listIndexes": name})
            .await?
            .try_collect()
            .await?;
        let sizes = collection
            .aggregate(vec![doc! {"$collStats": {"storageStats": {}}}])
            .await?
            .try_collect::<Vec<Document>>()
            .await
            .ok()
            .and_then(|rows| rows.into_iter().next())
            .and_then(|row| row.get_document("storageStats").ok().cloned())
            .and_then(|s| s.get_document("indexSizes").ok().cloned())
            .unwrap_or_default();
        let usage: Vec<Document> = match collection.aggregate(vec![doc! {"$indexStats": {}}]).await
        {
            Ok(cursor) => cursor.try_collect().await.unwrap_or_default(),
            Err(_) => Vec::new(), // needs privileges some users lack
        };
        Ok(specs
            .into_iter()
            .map(|spec| {
                let index_name = spec.get_str("name").unwrap_or_default().to_owned();
                let ops = usage
                    .iter()
                    .find(|u| u.get_str("name") == Ok(index_name.as_str()))
                    .and_then(|u| u.get_document("accesses").ok())
                    .map(|a| as_u64(a.get("ops")));
                IndexInfo {
                    keys: spec.get_document("key").cloned().unwrap_or_default(),
                    unique: spec.get_bool("unique").unwrap_or(false),
                    sparse: spec.get_bool("sparse").unwrap_or(false),
                    hidden: spec.get_bool("hidden").unwrap_or(false),
                    ttl_seconds: as_i64(spec.get("expireAfterSeconds")),
                    partial_filter: spec.get_document("partialFilterExpression").ok().cloned(),
                    size: sizes.get(&index_name).map(|s| as_u64(Some(s))),
                    ops,
                    name: index_name,
                    raw: spec,
                }
            })
            .collect())
    }

    pub async fn create_index(&self, db: &str, name: &str, index: NewIndex) -> Result<String> {
        if index.keys.is_empty() {
            return Err(RaabitError::Invalid(
                "an index needs at least one key".into(),
            ));
        }
        let options = IndexOptions::builder()
            .name(index.name.filter(|n| !n.trim().is_empty()))
            .unique(index.unique.then_some(true))
            .sparse(index.sparse.then_some(true))
            .hidden(index.hidden.then_some(true))
            .expire_after(index.ttl_seconds.map(Duration::from_secs))
            .partial_filter_expression(index.partial_filter)
            .build();
        let model = IndexModel::builder()
            .keys(index.keys)
            .options(options)
            .build();
        Ok(self
            .collection(db, name)
            .create_index(model)
            .await?
            .index_name)
    }

    pub async fn drop_index(&self, db: &str, name: &str, index: &str) -> Result<()> {
        if index == "_id_" {
            return Err(RaabitError::Invalid(
                "the _id index cannot be dropped".into(),
            ));
        }
        Ok(self.collection(db, name).drop_index(index).await?)
    }

    pub async fn set_index_hidden(
        &self,
        db: &str,
        name: &str,
        index: &str,
        hidden: bool,
    ) -> Result<()> {
        self.client
            .database(db)
            .run_command(doc! {"collMod": name, "index": {"name": index, "hidden": hidden}})
            .await?;
        Ok(())
    }

    // ----- users and roles -------------------------------------------------------------

    pub async fn users(&self, db: &str) -> Result<Vec<UserInfo>> {
        let reply = self
            .client
            .database(db)
            .run_command(doc! {"usersInfo": 1})
            .await?;
        let users = reply.get_array("users").cloned().unwrap_or_default();
        Ok(users
            .iter()
            .filter_map(|u| u.as_document())
            .map(|u| UserInfo {
                user: u.get_str("user").unwrap_or_default().to_owned(),
                db: u.get_str("db").unwrap_or_default().to_owned(),
                roles: role_refs(u.get("roles")),
                mechanisms: u
                    .get_array("mechanisms")
                    .map(|m| {
                        m.iter()
                            .filter_map(|x| x.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect())
    }

    pub async fn create_user(
        &self,
        db: &str,
        user: &str,
        password: &str,
        roles: &[RoleRef],
    ) -> Result<()> {
        if user.trim().is_empty() {
            return Err(RaabitError::Invalid("user name is empty".into()));
        }
        let roles: Vec<Bson> = roles
            .iter()
            .map(|r| Bson::Document(doc! {"role": &r.role, "db": &r.db}))
            .collect();
        self.client
            .database(db)
            .run_command(doc! {"createUser": user, "pwd": password, "roles": roles})
            .await?;
        Ok(())
    }

    pub async fn drop_user(&self, db: &str, user: &str) -> Result<()> {
        self.client
            .database(db)
            .run_command(doc! {"dropUser": user})
            .await?;
        Ok(())
    }

    pub async fn roles(&self, db: &str, include_builtin: bool) -> Result<Vec<RoleInfo>> {
        let reply = self
            .client
            .database(db)
            .run_command(
                doc! {"rolesInfo": 1, "showBuiltinRoles": include_builtin, "showPrivileges": true},
            )
            .await?;
        let roles = reply.get_array("roles").cloned().unwrap_or_default();
        Ok(roles
            .iter()
            .filter_map(|r| r.as_document())
            .map(|r| RoleInfo {
                role: r.get_str("role").unwrap_or_default().to_owned(),
                db: r.get_str("db").unwrap_or_default().to_owned(),
                is_builtin: r.get_bool("isBuiltin").unwrap_or(false),
                inherited: role_refs(r.get("inheritedRoles")),
                privileges: r
                    .get_array("privileges")
                    .map(|p| p.iter().filter_map(|x| x.as_document().cloned()).collect())
                    .unwrap_or_default(),
            })
            .collect())
    }

    // ----- server ----------------------------------------------------------------------

    pub async fn server_status(&self) -> Result<Document> {
        self.run_command("admin", doc! {"serverStatus": 1}).await
    }

    pub async fn current_ops(&self, include_idle: bool) -> Result<Vec<CurrentOp>> {
        let cursor = self
            .client
            .database("admin")
            .aggregate(vec![
                doc! {"$currentOp": {"allUsers": true, "idleConnections": include_idle}},
                doc! {"$sort": {"secs_running": -1}},
            ])
            .await?;
        let rows: Vec<Document> = cursor.try_collect().await?;
        Ok(rows
            .into_iter()
            .map(|raw| CurrentOp {
                opid: raw.get("opid").cloned().unwrap_or(Bson::Null),
                op: raw.get_str("op").unwrap_or("none").to_owned(),
                namespace: raw.get_str("ns").unwrap_or_default().to_owned(),
                client: raw.get_str("client").unwrap_or_default().to_owned(),
                secs_running: as_i64(raw.get("secs_running")).unwrap_or(0),
                description: raw.get_str("desc").unwrap_or_default().to_owned(),
                raw,
            })
            .collect())
    }

    pub async fn kill_op(&self, opid: Bson) -> Result<()> {
        self.run_command("admin", doc! {"killOp": 1, "op": opid})
            .await?;
        Ok(())
    }

    pub async fn disconnect(self) {
        self.client.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_names() {
        assert!(validate_name("database", "a.b").is_err());
        assert!(validate_name("database", "").is_err());
        assert!(validate_name("collection", "system.x").is_err());
        assert!(validate_name("collection", "orders").is_ok());
    }
}

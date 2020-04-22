//! database contains all struct and enum pertain to arangoDB "database" level.
//!
//! AQL query are all executed in database level, so Database offers AQL query.
use std::{collections::HashMap, fmt::Debug, sync::Arc};

use failure::{format_err, Error};
use log::trace;
use serde::de::DeserializeOwned;
use serde_json::value::Value;
use url::Url;

use maybe_async::maybe_async;

use crate::{
    aql::AqlQuery,
    client::ClientExt,
    collection::{Collection, CollectionDetails, CollectionResponse},
    connection::{Connection, DatabaseDetails, Version},
    response::{serialize, serialize_query_response, serialize_response, Cursor},
};

#[derive(Debug)]
pub struct ReadOnly;

#[derive(Debug)]
pub struct ReadWrite;

#[derive(Debug)]
pub struct Database<'a, C: ClientExt> {
    name: String,
    base_url: Url,
    session: Arc<C>,
    pub(crate) phantom: &'a (),
}

impl<'a, C: ClientExt> Database<'a, C> {
    pub(crate) fn new<T: Into<String>, S>(conn: &'a Connection<S, C>, name: T) -> Database<C> {
        let name = name.into();
        let path = format!("/_db/{}/", name.as_str());
        let url = conn.get_url().join(path.as_str()).unwrap();
        Database {
            name,
            session: conn.get_session(),
            base_url: url,
            phantom: &conn.phantom,
        }
    }
    /// Retrieve all collections of this database.
    #[maybe_async]
    pub async fn accessible_collections(&self) -> Result<Vec<CollectionResponse>, Error> {
        // an invalid arango_url should never running through initialization
        // so we assume arango_url is a valid url
        // When we pass an invalid path, it should panic to eliminate the bug
        // in development.
        let url = self.base_url.join("_api/collection").unwrap();
        trace!(
            "Retrieving collections from {:?}: {}",
            self.name,
            url.as_str()
        );
        let resp = self.session.get(url, "").await?;
        let result: Vec<CollectionResponse> = serialize_response(resp.text())?;
        trace!("Collections retrieved");
        Ok(result)
    }

    pub fn get_url(&self) -> &Url {
        &self.base_url
    }

    pub fn get_session(&self) -> Arc<C> {
        Arc::clone(&self.session)
    }

    /// Get collection object with name.
    #[maybe_async]
    pub async fn collection(&self, name: &str) -> Result<Collection<'_, C>, Error> {
        let collections = self.accessible_collections().await?;
        for collection in &collections {
            if collection.name.eq(name) {
                return Ok(Collection::from_response(self, collection));
            }
        }
        return Err(format_err!("Collection {} not found", name));
    }

    #[maybe_async]
    pub async fn create_edge_collection(&self, _name: &str) -> Collection<'_, C> {
        unimplemented!()
    }

    /// Create a collection via HTTP request and add it into `self.collections`.
    ///
    /// Return a database object if success.
    /// TODO
    #[maybe_async]
    pub async fn create_collection(&mut self, name: &str) -> Result<Collection<'_, C>, Error> {
        let mut map = HashMap::new();
        map.insert("name", name);
        let url = self.base_url.join("_api/collection").unwrap();
        let resp = self
            .session
            .post(url, &serde_json::to_string(&map)?)
            .await?;
        let result: CollectionDetails = serialize(resp.text())?;
        match result.error {
            true => Err(format_err!("Fail to create collection. Reason: {:?}", resp)),
            false => Ok(self.collection(name).await?),
        }
    }

    /// Drops a collection
    /// TODO
    #[maybe_async]
    pub async fn drop_collection(&mut self, name: &str) -> Result<CollectionDetails, Error> {
        let url_path = format!("_api/collection/{}", name);
        let url = self.base_url.join(&url_path).unwrap();

        let resp = self.session.delete(url, "").await?;
        let result: CollectionDetails = serialize(resp.text())?;
        match result.error {
            true => Err(format_err!(
                "Fail to drop collection. Reason: {:?}",
                result.error_message
            )),
            false => Ok(result),
        }
    }

    /// Get the version remote arango database server
    ///
    /// # Note
    /// this function would make a request to arango server.
    #[maybe_async]
    pub async fn arango_version(&self) -> Result<Version, Error> {
        let url = self.base_url.join("_api/version").unwrap();
        let resp = self.session.get(url, "").await?;
        let version: Version = serde_json::from_str(resp.text())?;
        Ok(version)
    }

    /// Get information of current database.
    ///
    /// # Note
    /// this function would make a request to arango server.
    #[maybe_async]
    pub async fn info(&self) -> Result<DatabaseDetails, Error> {
        let url = self.base_url.join("_api/database/current").unwrap();
        let resp = self.session.get(url, "").await?;
        serialize_response(resp.text())
    }

    /// Execute aql query, return a cursor if succeed. The major advantage of
    /// batch query is that cursors contain more information and stats
    /// about the AQL query, and users can fetch results in batch to save memory
    /// resources on clients.
    #[maybe_async]
    pub async fn aql_query_batch<R>(&self, aql: AqlQuery<'_>) -> Result<Cursor<R>, Error>
    where
        R: DeserializeOwned + Debug,
    {
        let url = self.base_url.join("_api/cursor").unwrap();
        let resp = self
            .session
            .post(url, &serde_json::to_string(&aql)?)
            .await?;
        trace!("{:?}", serde_json::to_string(&aql));
        serialize_query_response(resp.text())
    }

    /// Get next batch given the cursor id.
    #[maybe_async]
    pub async fn aql_next_batch<R>(&self, cursor_id: &str) -> Result<Cursor<R>, Error>
    where
        R: DeserializeOwned + Debug,
    {
        let url = self
            .base_url
            .join(&format!("_api/cursor/{}", cursor_id))
            .unwrap();
        let resp = self.session.put(url, "").await?;

        serialize_query_response(resp.text())
    }

    #[maybe_async]
    async fn aql_fetch_all<R>(&self, response: Cursor<R>) -> Result<Vec<R>, Error>
    where
        R: DeserializeOwned + Debug,
    {
        let mut response_cursor = response;
        let mut results: Vec<R> = Vec::new();
        loop {
            if response_cursor.more {
                let id = response_cursor.id.unwrap().clone();
                results.extend(response_cursor.result.into_iter());
                response_cursor = self.aql_next_batch(id.as_str()).await?;
            } else {
                break;
            }
        }
        Ok(results)
    }

    /// Execute AQL query fetch all results.
    ///
    /// DO NOT do this when the count of results is too large that network or
    /// memory resources cannot afford.
    ///
    /// DO NOT set a small batch size, otherwise clients will have to make many
    /// HTTP requests.
    #[maybe_async]
    pub async fn aql_query<R>(&self, aql: AqlQuery<'_>) -> Result<Vec<R>, Error>
    where
        R: DeserializeOwned + Debug,
    {
        let response = self.aql_query_batch(aql).await?;
        trace!("AQL query response: {:?}", response);
        if response.more {
            self.aql_fetch_all(response).await
        } else {
            Ok(response.result)
        }
    }

    /// Similar to `aql_query`, except that this method only accept a string of
    /// AQL query.
    #[maybe_async]
    pub async fn aql_str<R>(&self, query: &str) -> Result<Vec<R>, Error>
    where
        R: DeserializeOwned + Debug,
    {
        let aql = AqlQuery::new(query);
        self.aql_query(aql).await
    }

    /// Similar to `aql_query`, except that this method only accept a string of
    /// AQL query, with additional bind vars.
    #[maybe_async]
    pub async fn aql_bind_vars<R>(
        &self,
        query: &str,
        bind_vars: HashMap<&str, Value>,
    ) -> Result<Vec<R>, Error>
    where
        R: DeserializeOwned + Debug,
    {
        let mut aql = AqlQuery::new(query);
        for (key, value) in bind_vars {
            aql = aql.bind_var(key, value);
        }
        self.aql_query(aql).await
    }
}

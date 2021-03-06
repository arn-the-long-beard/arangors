//! Sync and async version share the exactly same API, except that async API
//! must be awaited.

#![allow(unused_imports)]
#![allow(unused_parens)]

use std::io::Write;

use serde_json::value::Value;

use arangors::{AqlQuery, Connection};

const URL: &str = "http://localhost:8529";

#[cfg_attr(feature = "reqwest_async", tokio::main)]
#[cfg_attr(feature = "surf_async", async_std::main)]
#[cfg_attr(feature = "reqwest_blocking", maybe_async::must_be_sync)]
async fn main() {
    // asynchronous version
    env_logger::init();

    let conn = Connection::establish_jwt(URL, "username", "password")
        .await
        .unwrap();

    let database = conn.db("test_db").await.unwrap();
    let aql = AqlQuery::new("FOR u IN test_collection LIMIT 3 RETURN u");
    println!("{:?}", aql);
    println!("{:?}", serde_json::to_string(&aql).unwrap());

    let resp: Vec<Value> = database.aql_query(aql).await.unwrap();
    println!("{:?}", resp);
}

#[cfg(not(any(
    feature = "reqwest_blocking",
    feature = "reqwest_async",
    feature = "surf_async"
)))]
fn main() {}

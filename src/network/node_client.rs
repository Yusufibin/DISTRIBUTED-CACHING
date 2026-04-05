use crate::{
    error::{CacheError, Result},
    storage::engine::CacheValue,
};
use hyper::{client::HttpConnector, Body, Client, Method, Request};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct GetRequest {
    pub key: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetResponse {
    pub found: bool,
    pub value: Option<Vec<u8>>,
    pub expires_at_unix: Option<u64>,
    pub created_at_unix: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SetRequest {
    pub key: String,
    pub value: Vec<u8>,
    pub ttl_secs: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteRequest {
    pub key: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OperationResponse {
    pub success: bool,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct NodeClient {
    inner: Client<HttpConnector>,
}

impl NodeClient {
    pub fn new(_timeout_secs: u64) -> Self {
        let client = Client::builder().build(HttpConnector::new());
        NodeClient { inner: client }
    }

    async fn post_json<B: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<R> {
        let json = serde_json::to_vec(body)
            .map_err(|e| CacheError::Serialization(e.to_string()))?;

        let req = Request::builder()
            .method(Method::POST)
            .uri(url)
            .header("content-type", "application/json")
            .body(Body::from(json))
            .map_err(|e| CacheError::Upstream(e.to_string()))?;

        let resp = self.inner.request(req).await
            .map_err(|e| CacheError::Upstream(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(CacheError::Upstream(format!("HTTP {}", resp.status())));
        }

        let bytes = hyper::body::to_bytes(resp.into_body()).await
            .map_err(|e| CacheError::Upstream(e.to_string()))?;

        serde_json::from_slice(&bytes).map_err(|e| CacheError::Serialization(e.to_string()))
    }

    pub async fn get(&self, addr: &str, key: &str) -> Result<Option<CacheValue>> {
        let url = format!("http://{}/internal/get", addr);
        let body: GetResponse = self.post_json(&url, &GetRequest { key: key.to_string() }).await?;
        if !body.found {
            return Ok(None);
        }
        let data = body.value.ok_or_else(|| CacheError::Upstream("missing value".into()))?;
        Ok(Some(CacheValue {
            data,
            expires_at_unix: body.expires_at_unix,
            created_at_unix: body.created_at_unix,
        }))
    }

    pub async fn set(&self, addr: &str, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) -> Result<()> {
        let url = format!("http://{}/internal/set", addr);
        let _: OperationResponse = self.post_json(&url, &SetRequest {
            key: key.to_string(),
            value,
            ttl_secs,
        }).await?;
        Ok(())
    }

    pub async fn delete(&self, addr: &str, key: &str) -> Result<bool> {
        let url = format!("http://{}/internal/delete", addr);
        let resp: OperationResponse = self.post_json(&url, &DeleteRequest { key: key.to_string() }).await?;
        Ok(resp.success)
    }

    pub async fn health_check(&self, addr: &str) -> bool {
        let url = format!("http://{}/internal/health", addr);
        let req = Request::builder()
            .method(Method::GET)
            .uri(&url)
            .body(Body::empty())
            .unwrap();
        match self.inner.request(req).await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    pub async fn post_gossip<B: Serialize>(&self, addr: &str, body: &B) -> Result<()> {
        let url = format!("http://{}/internal/gossip", addr);
        let json = serde_json::to_vec(body)
            .map_err(|e| CacheError::Serialization(e.to_string()))?;
        let req = Request::builder()
            .method(Method::POST)
            .uri(&url)
            .header("content-type", "application/json")
            .body(Body::from(json))
            .map_err(|e| CacheError::Upstream(e.to_string()))?;
        let resp = self.inner.request(req).await
            .map_err(|e| CacheError::Upstream(e.to_string()))?;
        if resp.status().is_success() { Ok(()) }
        else { Err(CacheError::Upstream(format!("HTTP {}", resp.status()))) }
    }
}

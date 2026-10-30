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
    pub value: CacheValue,
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
    timeout: std::time::Duration,
}

impl NodeClient {
    pub fn new(timeout_secs: u64) -> Self {
        let client = Client::builder().build(HttpConnector::new());
        NodeClient {
            inner: client,
            timeout: std::time::Duration::from_secs(timeout_secs),
        }
    }

    async fn post_json<B: Serialize, R: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<R> {
        let json =
            serde_json::to_vec(body).map_err(|e| CacheError::Serialization(e.to_string()))?;

        let req = Request::builder()
            .method(Method::POST)
            .uri(url)
            .header("content-type", "application/json")
            .body(Body::from(json))
            .map_err(|e| CacheError::Upstream(e.to_string()))?;

        let deadline = tokio::time::Instant::now() + self.timeout;
        let resp = tokio::time::timeout_at(deadline, self.inner.request(req))
            .await
            .map_err(|_| CacheError::Upstream("request timed out".into()))?
            .map_err(|e| CacheError::Upstream(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(CacheError::Upstream(format!("HTTP {}", resp.status())));
        }

        let bytes = tokio::time::timeout_at(deadline, hyper::body::to_bytes(resp.into_body()))
            .await
            .map_err(|_| CacheError::Upstream("response body timed out".into()))?
            .map_err(|e| CacheError::Upstream(e.to_string()))?;

        serde_json::from_slice(&bytes).map_err(|e| CacheError::Serialization(e.to_string()))
    }

    pub async fn get(&self, addr: &str, key: &str) -> Result<Option<CacheValue>> {
        let url = format!("http://{}/internal/get", addr);
        let body: GetResponse = self
            .post_json(
                &url,
                &GetRequest {
                    key: key.to_string(),
                },
            )
            .await?;
        if !body.found {
            return Ok(None);
        }
        let data = body
            .value
            .ok_or_else(|| CacheError::Upstream("missing value".into()))?;
        Ok(Some(CacheValue {
            data,
            expires_at_unix: body.expires_at_unix,
            created_at_unix: body.created_at_unix,
        }))
    }

    pub async fn set(&self, addr: &str, key: &str, value: CacheValue) -> Result<()> {
        let url = format!("http://{}/internal/set", addr);
        let response: OperationResponse = self
            .post_json(
                &url,
                &SetRequest {
                    key: key.to_string(),
                    value,
                },
            )
            .await?;
        if !response.success {
            return Err(CacheError::Upstream(
                response.message.unwrap_or_else(|| "write rejected".into()),
            ));
        }
        Ok(())
    }

    pub async fn delete(&self, addr: &str, key: &str) -> Result<bool> {
        let url = format!("http://{}/internal/delete", addr);
        let resp: OperationResponse = self
            .post_json(
                &url,
                &DeleteRequest {
                    key: key.to_string(),
                },
            )
            .await?;
        Ok(resp.success)
    }

    pub async fn health_check(&self, addr: &str) -> bool {
        let url = format!("http://{}/internal/health", addr);
        let req = Request::builder()
            .method(Method::GET)
            .uri(&url)
            .body(Body::empty())
            .map_err(|e| CacheError::Upstream(e.to_string()));
        let req = match req {
            Ok(req) => req,
            Err(_) => return false,
        };
        match tokio::time::timeout(self.timeout, self.inner.request(req)).await {
            Ok(Ok(resp)) => resp.status().is_success(),
            _ => false,
        }
    }

    pub async fn post_gossip<B: Serialize>(&self, addr: &str, body: &B) -> Result<()> {
        let url = format!("http://{}/internal/gossip", addr);
        let json =
            serde_json::to_vec(body).map_err(|e| CacheError::Serialization(e.to_string()))?;
        let req = Request::builder()
            .method(Method::POST)
            .uri(&url)
            .header("content-type", "application/json")
            .body(Body::from(json))
            .map_err(|e| CacheError::Upstream(e.to_string()))?;
        let resp = tokio::time::timeout(self.timeout, self.inner.request(req))
            .await
            .map_err(|_| CacheError::Upstream("request timed out".into()))?
            .map_err(|e| CacheError::Upstream(e.to_string()))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(CacheError::Upstream(format!("HTTP {}", resp.status())))
        }
    }
}

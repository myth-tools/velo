use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use autoagents::core::tool::{ToolCallError, ToolInputT, ToolRuntime, ToolT};
use autoagents_derive::tool;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{exec_err, http_client, ToolOutput};

#[derive(Serialize, Deserialize, Debug)]
pub struct HttpRequestArgs {
    pub method: String,
    pub url: String,
    pub headers: Option<HashMap<String, String>>,
    pub body: Option<Value>,
    pub timeout_secs: Option<u64>,
}

impl ToolInputT for HttpRequestArgs {
    fn io_schema() -> &'static str {
        r#"{"type":"object","properties":{"method":{"type":"string","description":"HTTP method: GET (default, read data), POST (create), PUT (update/replace), PATCH (partial update), DELETE (remove), HEAD (headers only), OPTIONS (capabilities)."},"url":{"type":"string","description":"Full request URL including protocol, e.g. 'https://api.example.com/v1/users'."},"headers":{"type":"object","description":"Optional HTTP request headers as key-value pairs, e.g. {'Authorization': 'Bearer ...', 'Content-Type': 'application/json'}.","additionalProperties":{"type":"string"}},"body":{"type":"object","description":"JSON body to send (for POST, PUT, PATCH methods). Automatically sets Content-Type: application/json."},"timeout_secs":{"type":"integer","description":"Request timeout in seconds. Default: 30. Increase for slow APIs."}}}"#
    }
}

#[tool(name = "http_request", description = "Make HTTP requests to APIs and web services. Methods: GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS. Returns status code, response headers, and body. Supports custom headers and JSON body. Timeout: configurable. BEST FOR: calling REST APIs, fetching web content, testing endpoints. Use the shell tool with curl for more advanced HTTP scenarios (custom certificates, multipart, streaming).", input = HttpRequestArgs)]
#[derive(Default, Clone)]
pub struct HttpRequestTool;

#[async_trait]
impl ToolRuntime for HttpRequestTool {
    async fn execute(&self, args: Value) -> Result<Value, ToolCallError> {
        let a: HttpRequestArgs = serde_json::from_value(args)?;
        let client = http_client();

        let method = a.method.to_uppercase();
        let timeout = Duration::from_secs(a.timeout_secs.unwrap_or(30));

        let mut req = match method.as_str() {
            "GET" => client.get(&a.url),
            "POST" => {
                let mut r = client.post(&a.url);
                if let Some(ref body) = a.body {
                    r = r.json(body);
                }
                r
            }
            "PUT" => {
                let mut r = client.put(&a.url);
                if let Some(ref body) = a.body {
                    r = r.json(body);
                }
                r
            }
            "PATCH" => {
                let mut r = client.patch(&a.url);
                if let Some(ref body) = a.body {
                    r = r.json(body);
                }
                r
            }
            "DELETE" => client.delete(&a.url),
            "HEAD" => client.head(&a.url),
            "OPTIONS" => client.request(reqwest::Method::OPTIONS, &a.url),
            other => return Err(exec_err(format!("Unsupported method: {other}"))),
        };

        if let Some(ref headers) = a.headers {
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
        }

        req = req.timeout(timeout);

        let response = req
            .send()
            .await
            .map_err(|e| exec_err(format!("Request failed: {e}")))?;

        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body = response.text().await.unwrap_or_default();
        let body_preview = if body.len() > 4000 {
            format!("{}...[{} total bytes]", &body[..4000], body.len())
        } else {
            body
        };

        Ok(ToolOutput::ok(format!(
            "Status: {status}\nContent-Type: {content_type}\n\n{body_preview}"
        ))
        .into())
    }
}
